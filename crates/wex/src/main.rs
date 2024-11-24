use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use bytes::Bytes;
use clap::Parser;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, OnceCell};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio::{select, signal};
use tracing::{debug, error, info, info_span, Instrument as _};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use wex::config::{self, Config};
use wex::{handle_component, handle_join_result, new_engine, Args};

fn main() -> anyhow::Result<()> {
    let Args {
        http_admin,
        shutdown_timeout,
    } = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    #[cfg(target_os = "linux")]
    {
        //use nix::sched::{unshare, CloneFlags};
        use nix::sys::stat::{makedev, mknod, Mode, SFlag};

        match std::fs::exists("/dev/net/tun") {
            Ok(true) => {}
            Ok(false) => {
                if let Err(err) = std::fs::create_dir_all("/dev/net") {
                    error!(?err, "failed to create `/dev/net` directory")
                }
                if let Err(err) = mknod(
                    "/dev/net/tun",
                    SFlag::S_IFCHR,
                    Mode::S_IWOTH | Mode::S_IWGRP | Mode::S_IWUSR,
                    makedev(10, 200),
                ) {
                    error!(?err, "failed to create `/dev/net/tun`")
                }
            }
            Err(err) => {
                error!(?err, "failed to check if `/dev/net/tun` exists")
            }
        }
        //unshare(CloneFlags::CLONE_NEWUSER).context("failed to unshare user namespace")?;
    }

    let Config {
        compositions, nats, ..
    } = match std::fs::read("wex.toml") {
        Ok(buf) => {
            toml::from_str(&String::from_utf8_lossy(&buf)).context("failed to parse `wex.toml`")?
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            debug!("`wex.toml` not found, using defaults");
            Default::default()
        }
        Err(err) => {
            bail!(anyhow!(err).context("failed to read `wex.toml`"))
        }
    };

    let rt = Runtime::new().context("failed to build Tokio runtime")?;
    rt.block_on(async {
        let shutdown: Arc<Notify> = Arc::default();
        let mut tasks = JoinSet::new();

        let ready = Arc::<AtomicBool>::default();
        if let Some(addr) = http_admin {
            let socket = TcpListener::bind(addr)
                .await
                .context("failed to bind HTTP administration endpoint")?;
            let addr = socket.local_addr().unwrap_or(addr);
            let ready = Arc::clone(&ready);
            let svc = hyper::service::service_fn(move |req| {
                const OK: &str = r#"{"status":"ok"}"#;
                const FAIL: &str = r#"{"status":"failure"}"#;
                let ready = Arc::clone(&ready);
                async move {
                    let (http::request::Parts { method, uri, .. }, _) = req.into_parts();
                    match (method.as_str(), uri.path_and_query().map(|pq| pq.as_str())) {
                        ("GET", Some("/livez")) => Ok(http::Response::new(
                            http_body_util::Full::new(Bytes::from(OK)),
                        )),
                        ("GET", Some("/readyz")) => {
                            if ready.load(Ordering::Relaxed) {
                                Ok(http::Response::new(http_body_util::Full::new(Bytes::from(
                                    OK,
                                ))))
                            } else {
                                Ok(http::Response::new(http_body_util::Full::new(Bytes::from(
                                    FAIL,
                                ))))
                            }
                        }
                        (method, Some(path)) => {
                            Err(format!("method `{method}` not supported for path `{path}`"))
                        }
                        (method, None) => Err(format!("method `{method}` not supported")),
                    }
                }
            });
            let srv = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            tasks.spawn(
                async move {
                    info!(?addr, "HTTP administration endpoint started");
                    loop {
                        let stream = match socket.accept().await {
                            Ok((stream, addr)) => {
                                info!(?addr, "accepted HTTP administration connection");
                                stream
                            }
                            Err(err) => {
                                error!(
                                    ?err,
                                    "failed to accept HTTP administration endpoint connection"
                                );
                                continue;
                            }
                        };
                        let svc = svc.clone();
                        if let Err(err) = srv.serve_connection(TokioIo::new(stream), svc).await {
                            error!(
                                ?err,
                                "failed to serve HTTP administration endpoint connection"
                            );
                        }
                    }
                }
                .instrument(info_span!("handle_http_admin")),
            );
        }

        let engine = new_engine(1000)?;
        let nats_addr = Arc::from(nats.address);
        let nats_once = OnceCell::new();
        for (composition, config::Composition { components, .. }) in compositions {
            let composition = Arc::<str>::from(composition);
            for (name, conf) in components {
                let composition = Arc::clone(&composition);
                let engine = engine.clone();
                let name = Arc::<str>::from(name);
                let nats_addr = Arc::clone(&nats_addr);
                let nats_once = nats_once.clone();
                let shutdown = Arc::clone(&shutdown);
                tasks.spawn(async move {
                    handle_component(
                        *shutdown_timeout,
                        engine,
                        nats_addr,
                        nats_once,
                        shutdown,
                        Arc::clone(&composition),
                        Arc::clone(&name),
                        conf,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to handle component `{name}` from composition `{composition}`"
                        )
                    })
                });
            }
        }
        ready.store(true, Ordering::Relaxed);
        let ctrl_c = signal::ctrl_c();
        let mut ctrl_c = pin!(ctrl_c);
        loop {
            select! {
                res = tasks.join_next() => {
                    if let Some(res) = res {
                        handle_join_result(res);
                    } else {
                        info!("no tasks left, exiting");
                        return Ok(())
                    }
                },
                ctrl_c = &mut ctrl_c => {
                    info!(shutdown_timeout = ?*shutdown_timeout, "^C received, shutting down");
                    ready.store(false, Ordering::Relaxed);
                    let ctrl_c = ctrl_c.context("failed to listen for ^C");
                    shutdown.notify_waiters();
                    let deadline = sleep(*shutdown_timeout);
                    let mut deadline = pin!(deadline);
                    loop {
                        select! {
                            res = tasks.join_next() => {
                                if let Some(res) = res {
                                    handle_join_result(res);
                                } else {
                                    return ctrl_c
                                }
                            }
                            _ = &mut deadline => {
                                tasks.abort_all();
                                while let Some(res) = tasks.join_next().await {
                                    handle_join_result(res);
                                }
                                return ctrl_c
                            }
                        }
                    }
                },
            }
        }
    })
}
