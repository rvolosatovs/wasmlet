use core::fmt::Debug;
use core::future::Future;
use core::net::Ipv4Addr;
use core::net::SocketAddr;
use core::pin::pin;
use core::str::FromStr;
use core::time::Duration;

use std::env::{self, VarError};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use clap::Parser;
use futures::{future::try_join_all, stream, StreamExt as _};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::fs;
use tokio::net::{TcpSocket, TcpStream};
use tokio::select;
use tokio::sync::{oneshot, Notify, OnceCell};
use tokio::task::{JoinError, JoinSet};
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::{debug, error, info, instrument, trace, warn};
use url::Url;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER,
    WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Engine, Store};
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiImpl, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::wasi::messaging;

pub mod config;
pub mod wasi;

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "imports",
        async: true,
        tracing: true,
        trappable_imports: true,
        with: {
            "wasi:io": wasmtime_wasi::bindings::io,
            "wasi:messaging/request-reply/request-options": crate::wasi::messaging::RequestOptions,
            "wasi:messaging/types/client": crate::wasi::messaging::Client,
            "wasi:messaging/types/message": crate::wasi::messaging::Message,
            "wasi:sockets/ip-name-lookup/resolve-address-stream": crate::wasi::sockets::ResolveAddressStream,
            "wasi:sockets/network/network": crate::wasi::sockets::Network,
            "wasi:sockets/tcp/tcp-socket": crate::wasi::sockets::TcpSocket,
            "wasi:sockets/udp/incoming-datagram-stream": crate::wasi::sockets::IncomingDatagramStream,
            "wasi:sockets/udp/outgoing-datagram-stream": crate::wasi::sockets::OutgoingDatagramStream,
            "wasi:sockets/udp/udp-socket": crate::wasi::sockets::UdpSocket,
        },
    });
}

#[derive(Parser, Debug)]
pub struct Args {
    #[clap(
        long = "shutdown-timeout",
        env = "WEX_SHUTDOWN_TIMEOUT",
        default_value = "10s"
    )]
    /// Graceful shutdown timeout
    pub shutdown_timeout: humantime::Duration,

    #[clap(long = "http-admin", env = "WEX_HTTP_ADMIN")]
    /// HTTP administration endpoint address
    pub http_admin: Option<SocketAddr>,
}

pub enum Workload {
    Url(Url),
    Binary(Vec<u8>),
}

pub struct Ctx {
    pub table: ResourceTable,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub network: config::component::Network,
    pub nats_addr: Arc<str>,
    pub nats_once: OnceCell<async_nats::Client>,
    pub ipv4_tun: Option<tun::Device>,
    pub ipv4_tun_addr: Ipv4Addr,
}

fn nats_connect_options() -> async_nats::ConnectOptions {
    async_nats::ConnectOptions::new().retry_on_initial_connect()
}

pub async fn nats_client(
    once: &OnceCell<async_nats::Client>,
    addr: impl async_nats::ToServerAddrs,
) -> anyhow::Result<&async_nats::Client> {
    once.get_or_try_init(|| nats_connect_options().connect(addr))
        .await
        .context("failed to connect to NATS.io")
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

fn getenv<T>(key: &str) -> Option<T>
where
    T: FromStr,
    T::Err: Debug,
{
    match env::var(key).as_deref().map(FromStr::from_str) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(err)) => {
            warn!(?err, "failed to parse `{key}` value, ignoring");
            None
        }
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(..)) => {
            warn!("`{key}` value is not valid UTF-8, ignoring");
            None
        }
    }
}

fn new_pooling_config(instances: u32) -> PoolingAllocationConfig {
    let mut config = PoolingAllocationConfig::default();
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_UNUSED_WASM_SLOTS") {
        config.max_unused_warm_slots(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_DECOMMIT_BATCH_SIZE") {
        config.decommit_batch_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_ASYNC_STACK_ZEROING") {
        config.async_stack_zeroing(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT") {
        config.async_stack_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT") {
        config.linear_memory_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TABLE_KEEP_RESIDENT") {
        config.table_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES") {
        config.total_component_instances(v);
    } else {
        config.total_component_instances(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_COMPONENT_INSTANCE_SIZE") {
        config.max_component_instance_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_CORE_INSTANCES_PER_COMPONENT") {
        config.max_core_instances_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORIES_PER_COMPONENT") {
        config.max_memories_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_TABLES_PER_COMPONENT") {
        config.max_tables_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_MEMORIES") {
        config.total_memories(v);
    } else {
        config.total_memories(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_TABLES") {
        config.total_tables(v);
    } else {
        config.total_tables(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_STACKS") {
        config.total_stacks(v);
    } else {
        config.total_stacks(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_CORE_INSTANCES") {
        config.total_core_instances(v);
    } else {
        config.total_core_instances(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_CORE_INSTANCE_SIZE") {
        config.max_core_instance_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_TABLES_PER_MODULE") {
        config.max_tables_per_module(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TABLE_ELEMENTS") {
        config.table_elements(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORIES_PER_MODULE") {
        config.max_memories_per_module(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORY_SIZE") {
        config.max_memory_size(v);
    }
    // TODO: Add memory protection key support
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_GC_HEAPS") {
        config.total_gc_heaps(v);
    } else {
        config.total_gc_heaps(instances);
    }
    config
}

// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default() -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    if let Some(v) = getenv("WEX_WASMTIME_POOLING") {
        return Ok(v);
    }
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.static_memory_maximum_size(1 << BITS_TO_TEST);
    let engine = Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}

fn type_annotate<T: WasiView, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasiImpl<&mut T>,
{
    val
}

pub fn new_engine(max_instances: u32) -> anyhow::Result<Engine> {
    let mut config = wasmtime::Config::default();
    config.wasm_component_model(true);
    config.async_support(true);
    if let Ok(true) = use_pooling_allocator_by_default() {
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(new_pooling_config(
            max_instances,
        )));
    } else {
        config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
    }
    if let Some(v) = getenv("WEX_WASMTIME_DEBUG_INFO") {
        config.debug_info(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_MAX_WASM_STACK") {
        config.max_wasm_stack(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_ASYNC_STACK_SIZE") {
        config.async_stack_size(v);
    }
    match Engine::new(&config).context("failed to construct engine") {
        Ok(engine) => Ok(engine),
        Err(err) => {
            warn!(
                ?err,
                "failed to construct engine, fallback to on-demand allocator"
            );
            config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
            Engine::new(&config).context("failed to construct engine")
        }
    }
}

#[instrument(skip(engine, adapter))]
pub async fn instantiate_pre(
    engine: &Engine,
    adapter: &[u8],
    workload: &str,
) -> anyhow::Result<InstancePre<Ctx>> {
    let wasm = if workload.starts_with('.') || workload.starts_with('/') {
        fs::read(&workload)
            .await
            .with_context(|| format!("failed to read relative path to workload `{workload}`"))
            .map(Workload::Binary)
    } else {
        Url::parse(workload)
            .with_context(|| format!("failed to parse Wasm URL `{workload}`"))
            .map(Workload::Url)
    }?;
    let wasm = match wasm {
        Workload::Url(wasm) => match wasm.scheme() {
            "file" => {
                let wasm = wasm
                    .to_file_path()
                    .map_err(|()| anyhow!("failed to convert Wasm URL to file path"))?;
                fs::read(wasm)
                    .await
                    .context("failed to read Wasm from file URL")?
            }
            "http" | "https" => {
                let wasm = reqwest::get(wasm).await.context("failed to GET Wasm URL")?;
                let wasm = wasm.bytes().await.context("failed fetch Wasm from URL")?;
                wasm.to_vec()
            }
            scheme => bail!("URL scheme `{scheme}` not supported"),
        },
        Workload::Binary(wasm) => wasm,
    };
    let wasm = if wasmparser::Parser::is_core_wasm(&wasm) {
        wit_component::ComponentEncoder::default()
            .validate(true)
            .module(&wasm)
            .context("failed to set core component module")?
            .adapter(WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, adapter)
            .context("failed to add WASI adapter")?
            .encode()
            .context("failed to encode a component")?
    } else {
        wasm
    };

    let component = Component::new(&engine, wasm).context("failed to compile component")?;

    let mut linker = Linker::<Ctx>::new(&engine);
    let closure = type_annotate(|ctx| WasiImpl(ctx));

    wasmtime_wasi::bindings::clocks::wall_clock::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::clocks::monotonic_clock::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::filesystem::types::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::filesystem::preopens::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::io::error::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::io::poll::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::io::streams::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::random::random::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::random::insecure::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::random::insecure_seed::add_to_linker_get_host(&mut linker, closure)?;

    let opts = wasmtime_wasi::bindings::cli::exit::LinkOptions::default();
    wasmtime_wasi::bindings::cli::exit::add_to_linker_get_host(&mut linker, &opts, closure)?;
    wasmtime_wasi::bindings::cli::environment::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::stdin::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::stdout::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::stderr::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::terminal_input::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::terminal_output::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::terminal_stdin::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::terminal_stdout::add_to_linker_get_host(&mut linker, closure)?;
    wasmtime_wasi::bindings::cli::terminal_stderr::add_to_linker_get_host(&mut linker, closure)?;

    wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
        .context("failed to link `wasi:http`")?;

    bindings::wasi::sockets::tcp::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/tcp`")?;
    bindings::wasi::sockets::tcp_create_socket::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/tcp-create-socket`")?;
    bindings::wasi::sockets::udp::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/udp`")?;
    bindings::wasi::sockets::udp_create_socket::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/udp-create-socket`")?;
    bindings::wasi::sockets::instance_network::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/instance-network`")?;
    let opts = bindings::wasi::sockets::network::LinkOptions::default();
    bindings::wasi::sockets::network::add_to_linker(&mut linker, &opts, |ctx| ctx)
        .context("failed to link `wasi:sockets/network`")?;
    bindings::wasi::sockets::ip_name_lookup::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:sockets/ip-name-lookup`")?;

    bindings::wasi::keyvalue::atomics::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:keyvalue/atomics`")?;
    bindings::wasi::keyvalue::batch::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:keyvalue/batch`")?;
    bindings::wasi::keyvalue::store::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:keyvalue/store`")?;

    bindings::wasi::messaging::types::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:messaging/types`")?;
    bindings::wasi::messaging::producer::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:messaging/producer`")?;
    bindings::wasi::messaging::request_reply::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `wasi:messaging/request_reply`")?;

    linker
        .instantiate_pre(&component)
        .context("failed to pre-instantiate component")
}

pub fn handle_message<Fut>(
    tasks: &mut JoinSet<anyhow::Result<()>>,
    new_store: impl FnOnce() -> Fut + Send + 'static,
    pre: messaging::bindings::MessagingGuestPre<Ctx>,
    msg: messaging::Message,
) where
    Fut: Future<Output = anyhow::Result<Store<Ctx>>> + Send + 'static,
{
    tasks.spawn(async move {
        let mut store = new_store().await?;
        let component = pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate `wasi:messaging/incoming-handler`")?;
        let msg = store
            .data_mut()
            .table
            .push(msg)
            .context("failed to push message to table")?;
        let res = component
            .wasi_messaging_incoming_handler()
            .call_handle(&mut store, msg)
            .await
            .context("failed to invoke component")?;
        res.context("failed to handle NATS.io message")
    });
}

pub fn handle_http<Fut>(
    tasks: &mut JoinSet<anyhow::Result<()>>,
    new_store: impl FnOnce() -> Fut + Send + Clone + 'static,
    pre: wasmtime_wasi_http::bindings::ProxyPre<Ctx>,
    stream: TcpStream,
) where
    Fut: Future<Output = anyhow::Result<Store<Ctx>>> + Send + 'static,
{
    tasks.spawn(async move {
        hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
            .serve_connection(
                TokioIo::new(stream),
                hyper::service::service_fn(move |req| {
                    let new_store = new_store.clone();
                    let pre = pre.clone();
                    async move {
                        let mut store = new_store().await?;
                        trace!("instantiating `wasi:http/incoming-handler`");
                        let component = pre
                            .instantiate_async(&mut store)
                            .await
                            .context("failed to instantiate `wasi:http/incoming-handler`")?;
                        let req = store
                            .data_mut()
                            .new_incoming_request(
                                wasmtime_wasi_http::bindings::http::types::Scheme::Http,
                                req,
                            )
                            .context("failed to create a new HTTP request resource")?;
                        let (tx, rx) = oneshot::channel();
                        let out = store
                            .data_mut()
                            .new_response_outparam(tx)
                            .context("failed to create response outparam resource")?;
                        debug!("invoking `wasi:http/incoming-handler.handle`");
                        let () = component
                            .wasi_http_incoming_handler()
                            .call_handle(&mut store, req, out)
                            .await
                            .context("failed to invoke component")?;
                        debug!("awaiting `wasi:http/incoming-handler.handle` response");
                        match rx.await {
                            Ok(Ok(res)) => {
                                debug!(
                                "successful `wasi:http/incoming-handler.handle` response received"
                            );
                                Ok(res)
                            }
                            Ok(Err(err)) => {
                                debug!(
                            ?err,
                            "unsuccessful `wasi:http/incoming-handler.handle` response received"
                        );
                                Err(err.into())
                            }
                            Err(_) => {
                                debug!(
                                    "`wasi:http/incoming-handler.handle` response sender dropped"
                                );
                                bail!("component did not call `response-outparam::set`")
                            }
                        }
                    }
                }),
            )
            .await
            .map_err(|err| anyhow!(err).context("failed to serve HTTP connection"))
    });
}

pub fn handle_join_result(res: Result<anyhow::Result<()>, JoinError>) {
    match res {
        Ok(Ok(())) => debug!("successfully joined task"),
        Ok(Err(err)) => warn!(?err, "task failed"),
        Err(err) => error!(?err, "failed to join task"),
    }
}

pub async fn call_run(
    store: &mut Store<Ctx>,
    cmd: wasmtime_wasi::bindings::CommandPre<Ctx>,
) -> anyhow::Result<()> {
    let cmd = cmd
        .instantiate_async(&mut *store)
        .await
        .context("failed to instantiate `wasi:cli/run`")?;
    cmd.wasi_cli_run()
        .call_run(store)
        .await
        .context("failed to call `wasi:cli/run`")?
        .map_err(|()| anyhow!("`wasi:cli/run` failed"))?;
    anyhow::Ok(())
}

#[instrument(skip_all, fields(
    component = name.as_ref(),
    composition = composition.as_ref(),
))]
pub async fn handle_component(
    shutdown_timeout: Duration,
    engine: Engine,
    nats_addr: Arc<str>,
    nats_once: OnceCell<async_nats::Client>,
    shutdown: Arc<Notify>,
    composition: Arc<str>,
    name: Arc<str>,
    config::Component {
        src,
        cli,
        trigger,
        network,
        ..
    }: config::Component,
) -> anyhow::Result<()> {
    let pre = instantiate_pre(
        &engine,
        if cli.run.unwrap_or_default() {
            WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER
        } else {
            WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER
        },
        &src,
    )
    .await?;
    let new_store = {
        let nats_addr = Arc::clone(&nats_addr);
        let nats_once = nats_once.clone();
        move || {
            let mut ipv4_tun_addr = Ipv4Addr::UNSPECIFIED;
            let ipv4_tun = match network.tcp.ipv4 {
                config::component::network::Network::None {
                    loopback: config::component::network::none::Loopback::None,
                }
                | config::component::network::Network::Host(
                    config::component::network::host::Config {
                        loopback:
                            config::component::network::host::Loopback::None
                            | config::component::network::host::Loopback::Host,
                        ..
                    },
                ) => None,
                config::component::network::Network::None {
                    loopback: config::component::network::none::Loopback::Tun,
                }
                | config::component::network::Network::Host(
                    config::component::network::host::Config {
                        loopback: config::component::network::host::Loopback::Tun,
                        ..
                    },
                ) => {
                    debug!("creating TUN...");
                    ipv4_tun_addr = Ipv4Addr::new(169, 254, 0, 1);
                    let tun = tun::create(
                        tun::Configuration::default()
                            .mtu(65520)
                            .address(ipv4_tun_addr)
                            .netmask((255, 255, 255, 255))
                            .destination(ipv4_tun_addr)
                            .up(),
                    )
                    .context("failed to create TUN")?;
                    Some(tun)
                }
                config::component::network::Network::None {
                    loopback: config::component::network::none::Loopback::Composition { .. },
                }
                | config::component::network::Network::Host(
                    config::component::network::host::Config {
                        loopback: config::component::network::host::Loopback::Composition { .. },
                        ..
                    },
                ) => {
                    bail!("composition-scoped localhost not supported yet")
                }
            };
            Ok(Store::new(
                &engine,
                Ctx {
                    wasi: WasiCtxBuilder::new()
                        .inherit_env()
                        .inherit_stdio()
                        .args(&["workload.wasm"])
                        .build(),
                    http: WasiHttpCtx::new(),
                    network: network.clone(),
                    table: ResourceTable::new(),
                    nats_addr: Arc::clone(&nats_addr),
                    nats_once: nats_once.clone(),
                    ipv4_tun,
                    ipv4_tun_addr,
                },
            ))
        }
    };
    let mut tasks = JoinSet::new();
    match cli.run {
        None => {
            if let Ok(cmd) = wasmtime_wasi::bindings::CommandPre::new(pre.clone()) {
                let mut store = new_store()?;
                tasks.spawn(async move { call_run(&mut store, cmd).await });
            }
        }
        Some(true) => {
            let cmd = wasmtime_wasi::bindings::CommandPre::new(pre.clone())
                .context("failed to pre-instantiate `wasi:cli/run`")?;
            let mut store = new_store()?;
            tasks.spawn(async move { call_run(&mut store, cmd).await });
        }
        _ => {}
    }
    let http_pre = (!trigger.http.is_empty())
        .then(|| wasmtime_wasi_http::bindings::ProxyPre::new(pre.clone()))
        .transpose()
        .context("failed to pre-instantiate `wasi:http/incoming-handler`")?;
    let http_conns = if !trigger.http.is_empty() {
        let conns = Box::into_iter(trigger.http).map(
            |config::component::HttpTrigger { address }| async move {
                debug!(?address, "binding TCP socket...");
                let sock = match address {
                    SocketAddr::V4(..) => TcpSocket::new_v4(),
                    SocketAddr::V6(..) => TcpSocket::new_v6(),
                }
                .context("failed to create socket")?;
                // Conditionally enable `SO_REUSEADDR` depending on the current
                // platform. On Unix we want this to be able to rebind an address in
                // the `TIME_WAIT` state which can happen then a server is killed with
                // active TCP connections and then restarted. On Windows though if
                // `SO_REUSEADDR` is specified then it enables multiple applications to
                // bind the port at the same time which is not something we want. Hence
                // this is conditionally set based on the platform (and deviates from
                // Tokio's default from always-on).
                sock.set_reuseaddr(!cfg!(windows))?;
                sock.bind(address)
                    .with_context(|| format!("failed to bind on `{address}`"))?;
                let sock = sock.listen(1024).context("failed to listen on socket")?;
                let address = sock.local_addr().unwrap_or(address);
                info!(?address, "bound TCP socket");
                anyhow::Ok(TcpListenerStream::new(sock))
            },
        );
        try_join_all(conns).await?
    } else {
        Vec::default()
    };

    let messaging_pre = (!trigger.nats.is_empty())
        .then(|| messaging::bindings::MessagingGuestPre::new(pre.clone()))
        .transpose()
        .context("failed to pre-instantiate `wasi:messaging/incoming-handler`")?;
    let nats_msgs = if !trigger.nats.is_empty() {
        let subs = Box::into_iter(trigger.nats).map(
            |config::component::NatsTrigger { subject, group }| {
                let nats_addr = Arc::clone(&nats_addr);
                let nats_once = nats_once.clone();
                async move {
                    let nats = nats_client(&nats_once, nats_addr.as_ref()).await?;
                    let subject = async_nats::Subject::from(subject.into_string());
                    if !group.is_empty() {
                        nats.queue_subscribe(subject, group.into_string()).await
                    } else {
                        nats.subscribe(subject).await
                    }
                    .context("failed to subscribe")
                }
            },
        );
        try_join_all(subs).await?
    } else {
        Vec::default()
    };

    let new_store_init = {
        let pre = pre.clone();
        move || async move {
            match cli.run {
                None => {
                    if let Ok(cmd) = wasmtime_wasi::bindings::CommandPre::new(pre.clone()) {
                        let mut store = new_store()?;
                        call_run(&mut store, cmd).await?;
                        Ok(store)
                    } else {
                        new_store()
                    }
                }
                Some(true) => {
                    let cmd = wasmtime_wasi::bindings::CommandPre::new(pre.clone())
                        .context("failed to pre-instantiate `wasi:cli/run`")?;
                    let mut store = new_store()?;
                    call_run(&mut store, cmd).await?;
                    Ok(store)
                }
                _ => new_store(),
            }
        }
    };

    let mut nats_msgs = stream::select_all(nats_msgs);
    let mut http_conns = stream::select_all(http_conns);
    let shutdown = shutdown.notified();
    let mut shutdown = pin!(shutdown);
    loop {
        select! {
            Some(conn) = http_conns.next() => {
                match conn {
                    Ok(conn) => {
                        let new_store_init = new_store_init.clone();
                        handle_http(
                            &mut tasks,
                            new_store_init,
                            http_pre.clone().unwrap(),
                            conn,
                        );
                    }
                    Err(err) => {
                        error!(?err, "failed to accept TCP connection");
                    }
                };
            },
            Some(msg) = nats_msgs.next() => {
                let new_store_init = new_store_init.clone();
                handle_message(
                    &mut tasks,
                    new_store_init,
                    messaging_pre.clone().unwrap(),
                    messaging::Message::Nats(msg),
                );
            },
            Some(res) = tasks.join_next() => {
                handle_join_result(res)
            },
            _ = &mut shutdown => {
                // wait for all invocations to complete
                let deadline = sleep(shutdown_timeout);
                let mut deadline = pin!(deadline);
                loop {
                    select! {
                        res = tasks.join_next() => {
                            if let Some(res) = res {
                                handle_join_result(res);
                            } else {
                                return Ok(())
                            }
                        }
                        _ = &mut deadline => {
                            tasks.abort_all();
                            while let Some(res) = tasks.join_next().await {
                                handle_join_result(res);
                            }
                            return Ok(())
                        }
                    }
                }
            },
        }
    }
}
