use core::future::{poll_fn, Future as _};
use core::iter::zip;
use core::net::SocketAddr;
use core::pin::pin;
use core::sync::atomic::Ordering;
use core::task::Poll;
use core::time::Duration;

use std::thread;
use std::time::SystemTime;
use std::{alloc::System, time::UNIX_EPOCH};

use anyhow::{anyhow, Context as _};
use clap::Parser;
use quanta::Clock;
use tokio::signal;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use wex::{
    load_and_apply_manifest, Engine, Host, EPOCH_INTERVAL, EPOCH_MONOTONIC_NOW, EPOCH_SYSTEM_NOW,
};

// Explicitly select system allocator to ensure dylib compatiblity
#[global_allocator]
static ALLOC: System = System;

#[derive(Parser, Debug)]
pub struct Args {
    #[clap(
        long = "shutdown-timeout",
        env = "WEX_SHUTDOWN_TIMEOUT",
        default_value = "10s"
    )]
    /// Graceful shutdown timeout
    pub shutdown_timeout: humantime::Duration,

    #[clap(
        long = "max-instances",
        env = "WEX_MAX_INSTANCES",
        default_value_t = 1000
    )]
    /// Maximum number of concurrent instances
    pub max_instances: u32,

    #[clap(long = "http-admin", env = "WEX_HTTP_ADMIN")]
    /// HTTP administration endpoint address
    pub http_admin: Option<SocketAddr>,

    #[clap(long = "http-proxy", env = "WEX_HTTP_PROXY")]
    /// HTTP reverse proxy endpoint address
    pub http_proxy: Option<SocketAddr>,
}

/// Computes appropriate resolution of the clock by taking N samples
fn sample_clock_resolution<const N: usize>(clock: &Clock) -> Duration {
    let mut instants = [clock.now(); N];
    for t in &mut instants {
        *t = clock.now();
    }
    let mut resolution = Duration::from_nanos(1);
    for (i, j) in zip(0.., 1..N) {
        let a = instants[i];
        let b = instants[j];
        resolution = b.saturating_duration_since(a).max(resolution);
    }
    resolution
}

fn main() -> anyhow::Result<()> {
    let clock = Clock::new();

    let resolution = sample_clock_resolution::<10000>(&clock);
    if resolution > EPOCH_INTERVAL {
        warn!(
            ?resolution,
            ?EPOCH_INTERVAL,
            "observed clock resolution is greater that epoch interval used"
        );
    } else {
        debug!(?resolution, "completed sampling clock resolution");
    }

    let Args {
        http_proxy,
        max_instances,
        ..
    } = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    debug!(pid = std::process::id(), "wex starting",);

    let engine = wex::wasmtime::new_engine(max_instances)?;

    let init_at = clock.now();
    debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));
    let system_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before Unix epoch")?;
    let system_now_millis = system_now
        .as_millis()
        .try_into()
        .context("milliseconds since epoch do not fit in u64")?;
    EPOCH_MONOTONIC_NOW.store(0, Ordering::Relaxed);
    EPOCH_SYSTEM_NOW.store(system_now_millis, Ordering::Relaxed);
    let epoch = {
        let engine = engine.weak();
        thread::Builder::new()
            .name("wex-epoch".into())
            .spawn(move || loop {
                thread::sleep(EPOCH_INTERVAL);

                let monotonic_now = clock.now().saturating_duration_since(init_at);
                let monotonic_now = monotonic_now.as_millis().try_into().unwrap_or(u64::MAX);
                EPOCH_MONOTONIC_NOW.store(monotonic_now, Ordering::Relaxed);

                if let Ok(system_now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                    let system_now_millis = system_now.as_millis().try_into().unwrap_or(u64::MAX);
                    EPOCH_SYSTEM_NOW.store(system_now_millis, Ordering::Relaxed);
                }

                let Some(engine) = engine.upgrade() else {
                    debug!("engine was dropped, epoch thread exiting");
                    return;
                };
                engine.increment_epoch();
            })
            .context("failed to spawn epoch thread")?
    };
    let max_instances = max_instances.try_into().unwrap_or(usize::MAX);

    let (cmds_tx, cmds_rx) = mpsc::channel(max_instances.max(1024));

    let engine = thread::Builder::new()
        .name("wex-engine".into())
        .spawn(move || Engine::new(engine, max_instances).handle_commands(cmds_rx))
        .context("failed to spawn scheduler thread")?;

    let main = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .thread_name("wex-main")
        .build()
        .context("failed to build main Tokio runtime")?;
    main.block_on(async {
        load_and_apply_manifest(&cmds_tx, "wex.toml").await?;

        let mut tasks = JoinSet::new();
        let mut host = Host::new(cmds_tx.clone(), max_instances);
        if let Some(addr) = http_proxy {
            let task = host.handle_http_proxy(addr).await?;
            tasks.spawn(task);
        }
        #[cfg(unix)]
        let mut sighup = signal::unix::signal(signal::unix::SignalKind::hangup())
            .context("failed to listen for SIGHUP")?;
        #[cfg(unix)]
        tasks.spawn({
            let cmds_tx = cmds_tx.clone();
            async move {
                loop {
                    while let Some(()) = sighup.recv().await {
                        info!("SIGHUP received, reloading manifest");
                        if let Err(err) = load_and_apply_manifest(&cmds_tx, "wex.toml").await {
                            error!(?err, "failed to load and apply manifest");
                        }
                        info!("reloaded manifest");
                    }
                    info!("manifest reload task exiting");
                }
            }
        });
        let ctrl_c = signal::ctrl_c();
        let mut ctrl_c = pin!(ctrl_c);
        poll_fn(|cx| {
            loop {
                match tasks.poll_join_next(cx) {
                    Poll::Ready(Some(Ok(()))) => debug!("successfully joined host task"),
                    Poll::Ready(Some(Err(err))) => error!(?err, "failed to join host task"),
                    Poll::Ready(None) => {
                        info!("no host tasks left, shutting down");
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Pending => break,
                }
            }
            match ctrl_c.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => {
                    info!("^C received, shutting down");
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Err(err)) => {
                    warn!(?err, "failed to listen for ^C, shutting down");
                    Poll::Ready(Err(err))
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await?;
        drop(host);
        drop(cmds_tx);
        tasks.abort_all();
        debug!("joining engine thread");
        engine
            .join()
            .map_err(|_| anyhow!("engine thread panicked"))?
            .context("engine thread failed")?;
        epoch.join().map_err(|_| anyhow!("epoch thread panicked"))
    })
}
