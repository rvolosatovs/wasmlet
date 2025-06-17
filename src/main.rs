use core::future::{poll_fn, Future as _};
use core::iter::zip;
use core::net::SocketAddr;
use core::pin::pin;
use core::sync::atomic::Ordering;
use core::task::Poll;
use core::time::Duration;

use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context as _};
use clap::{command, Parser};
use futures::StreamExt as _;
use kube::runtime::WatchStreamExt as _;
use kube::{CustomResourceExt as _, ResourceExt as _};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use quanta::Clock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::signal;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};
use wasmlet::{
    apply_manifest, load_and_apply_manifest, read_and_apply_manifest, Engine, Host, Manifest, Wasi,
    EPOCH_INTERVAL, EPOCH_MONOTONIC_NOW, EPOCH_SYSTEM_NOW,
};

#[derive(
    Clone, Debug, Deserialize, Serialize, Eq, PartialEq, kube::CustomResource, schemars::JsonSchema,
)]
#[kube(
    group = "wasmcloud.dev",
    kind = "WasmPod",
    namespaced,
    status = PodStatus,
    version = "v1alpha1"
)]
pub struct Pod {
    #[serde(flatten)]
    manifest: Manifest,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, schemars::JsonSchema)]
pub struct PodStatus {
    pub error: Box<str>,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub enum Command {
    Run(RunArgs),
    #[clap(subcommand)]
    K8s(KubernetesCommand),
}

#[derive(Parser, Debug)]
pub enum KubernetesCommand {
    Crd,
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    #[clap(
        long = "max-instances",
        env = "WASMX_MAX_INSTANCES",
        default_value_t = 1000
    )]
    /// Maximum number of concurrent instances
    pub max_instances: u32,

    #[clap(long = "http-admin", env = "WASMX_HTTP_ADMIN")]
    /// HTTP administration endpoint address
    pub http_admin: Option<SocketAddr>,

    #[clap(long = "http-proxy", env = "WASMX_HTTP_PROXY")]
    /// HTTP reverse proxy endpoint address
    pub http_proxy: Option<SocketAddr>,

    #[clap(long = "k8s", env = "WASMX_K8S", default_value_t = Box::default())]
    /// Whether to enable Kubernetes CRD support
    pub k8s: Box<str>,
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

fn init_tracing() -> SdkLoggerProvider {
    //let exporter = opentelemetry_stdout::LogExporter::default();
    let provider: SdkLoggerProvider = SdkLoggerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name("wasmlet")
                .build(),
        )
        //.with_simple_exporter(exporter)
        .build();

    let otel_layer =
        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&provider)
            .with_filter(
                EnvFilter::new("info")
                    .add_directive("hyper=off".parse().unwrap())
                    .add_directive("tonic=off".parse().unwrap())
                    .add_directive("h2=off".parse().unwrap())
                    .add_directive("reqwest=off".parse().unwrap()),
            );

    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .without_time()
        .with_thread_names(true)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));

    tracing_subscriber::registry()
        .with(otel_layer)
        .with(fmt_layer)
        .init();
    provider
}

fn main() -> anyhow::Result<()> {
    let RunArgs {
        max_instances,
        http_admin,
        http_proxy,
        k8s,
    } = match Command::parse() {
        Command::K8s(KubernetesCommand::Crd) => {
            let crd =
                serde_yaml::to_string(&WasmPod::crd()).context("failed to encode WasmPod CRD")?;
            print!("{crd}");
            return Ok(());
        }
        Command::Run(args) => args,
    };

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

    let otel = init_tracing();
    debug!(pid = std::process::id(), "wasmlet starting",);

    let engine = wasmlet::wasmtime::new_engine(max_instances)?;

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
            .name("wasmlet-epoch".into())
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
        .name("wasmlet-engine".into())
        .spawn(move || Engine::<Wasi>::new(engine, max_instances).handle_commands(cmds_rx))
        .context("failed to spawn scheduler thread")?;

    let main = tokio::runtime::Builder::new_multi_thread()
        .thread_name("wasmlet-main")
        .enable_io()
        .enable_time()
        .build()
        .context("failed to build main Tokio runtime")?;
    main.block_on(async {
        if std::fs::exists("wasmlet.toml")? {
            read_and_apply_manifest(&cmds_tx, "wasmlet.toml").await?;
        }

        let mut tasks = JoinSet::new();
        let mut host = Host::new(cmds_tx.clone(), max_instances);
        let ready = Arc::default();
        if let Some(addr) = http_admin {
            let task = host.handle_http_admin(addr, Arc::clone(&ready)).await?;
            tasks.spawn(task);
        }
        if let Some(addr) = http_proxy {
            let task = host.handle_http_proxy(addr).await?;
            tasks.spawn(task);
        }
        if !k8s.is_empty() {
            let client = kube::Client::try_default().await?;

            // TODO: Filter
            let pods: kube::Api<WasmPod> = kube::Api::default_namespaced(client.clone());

            let wc = kube::runtime::watcher::Config::default().any_semantic();
            let events = kube::runtime::watcher(pods.clone(), wc).default_backoff();
            let cmds_tx = cmds_tx.clone();
            tasks.spawn(async move {
                let (k8s_namespace, k8s_name) = k8s.split_once(':').unwrap_or(("default", &k8s));
                let mut events = pin!(events);
                while let Some(event) = events.next().await {
                    match event {
                        Ok(
                            kube::runtime::watcher::Event::Apply(pod)
                            | kube::runtime::watcher::Event::InitApply(pod),
                        ) => {
                            let namespace = pod.namespace();
                            let namespace = namespace.as_deref().unwrap_or("default");
                            let name = pod.name_any();
                            if namespace != k8s_namespace || name != k8s_name {
                                debug!(namespace, name, "skipping pod apply event");
                                continue;
                            }
                            info!(?pod.spec.manifest, "pod CRD applied, reloading manifest");

                            let version = pod.resource_version();
                            let status = if let Err(err) =
                                load_and_apply_manifest(&cmds_tx, pod.spec.manifest).await
                            {
                                warn!(?err, "failed to apply manifest");
                                PodStatus {
                                    error: format!("{err:#}").into(),
                                }
                            } else {
                                info!("applied manifest");
                                PodStatus {
                                    error: Box::default(),
                                }
                            };
                            let status = match serde_json::to_vec(&json!({
                                "apiVersion": "wasmcloud.dev/v1alpha1",
                                "kind": "WasmPod",
                                "metadata": {
                                    "name": name,
                                    "resourceVersion": version,
                                },
                                "status": status,
                            })) {
                                Ok(status) => status,
                                Err(err) => {
                                    error!(?err, "failed to encode status to JSON");
                                    continue;
                                }
                            };
                            if let Err(err) = pods
                                .replace_status(&name, &kube::api::PostParams::default(), status)
                                .await
                            {
                                error!(?err, "failed to update pod status")
                            }
                            continue;
                        }
                        Ok(kube::runtime::watcher::Event::Delete(pod)) => {
                            let namespace = pod.namespace();
                            let namespace = namespace.as_deref().unwrap_or("default");
                            let name = pod.name_any();
                            if namespace != k8s_namespace || name != k8s_name {
                                debug!(namespace, name, "skipping pod delete event");
                                continue;
                            }
                            info!("pod deleted, reloading manifest");
                            if let Err(err) = apply_manifest(&cmds_tx, Manifest::default()).await {
                                error!(?err, "failed to apply manifest");
                                continue;
                            }
                            info!("applied manifest");
                        }
                        Ok(event) => {
                            debug!(?event, "skip pod event")
                        }
                        Err(err) => {
                            error!(?err, "failed to receive pod event");
                        }
                    };
                }
            });
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
                        if let Err(err) = read_and_apply_manifest(&cmds_tx, "wasmlet.toml").await {
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
        ready.store(true, Ordering::Relaxed);
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
        ready.store(false, Ordering::Relaxed);
        drop(host);
        drop(cmds_tx);
        tasks.abort_all();
        debug!("joining engine thread");
        engine
            .join()
            .map_err(|_| anyhow!("engine thread panicked"))?
            .context("engine thread failed")?;
        epoch.join().map_err(|_| anyhow!("epoch thread panicked"))?;
        otel.shutdown().context("failed to shutdown OTEL")
    })
}
