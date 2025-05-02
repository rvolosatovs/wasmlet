#![allow(unused)] // TODO: Remove

use core::ffi::c_char;
use core::fmt::Debug;
use core::iter::zip;
use core::mem;
use core::num::NonZeroUsize;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Waker;
use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::thread::{self, ScopedJoinHandle};

use anyhow::{anyhow, bail, ensure, Context as _};
use async_ffi::FfiFuture;
use bindings::exports::wasi::cli::{Command, CommandPre};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use libloading::{Library, Symbol};
use quanta::Clock;
use tokio::net::TcpStream;
use tokio::sync::{
    broadcast, mpsc, oneshot, watch, Notify, OwnedSemaphorePermit, Semaphore, SemaphorePermit,
};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{debug, debug_span, error, info, info_span, instrument, warn, Instrument as _, Span};
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER,
    WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{
    types, Component, ComponentExportIndex, Instance, InstancePre, Linker, LinkerInstance,
    ResourceAny, ResourceType, Val,
};
use wasmtime::{AsContextMut, Store, UpdateDeadline};
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder};
use wasmtime_wasi_http::body::HyperOutgoingBody;

pub mod bindings;
pub mod wasi;
mod workload;

use crate::{config, Manifest, EPOCH_MONOTONIC_NOW};

use self::wasi::cli::I32Exit;
use self::wasi::http::{ErrorCode, IncomingBody, WasiHttpCtx};
pub use self::workload::Ctx;
use self::workload::{handle_dynamic, handle_http};

pub mod resource_types {
    use wasmtime::component::ResourceType;

    use crate::engine::wasi;

    pub type InputStream = wasi::io::InputStream;
    pub type IoError = wasi::io::Error;
    pub type OutputStream = wasi::io::OutputStream;
    pub type Pollable = wasi::io::Pollable;

    pub type TcpSocket = wasi::sockets::tcp::TcpSocket;

    pub fn input_stream() -> ResourceType {
        ResourceType::host::<InputStream>()
    }

    pub fn io_error() -> ResourceType {
        ResourceType::host::<IoError>()
    }

    pub fn output_stream() -> ResourceType {
        ResourceType::host::<OutputStream>()
    }

    pub fn pollable() -> ResourceType {
        ResourceType::host::<Pollable>()
    }

    pub fn tcp_socket() -> ResourceType {
        ResourceType::host::<TcpSocket>()
    }
}

pub trait ResourceView {
    fn table(&mut self) -> &mut ResourceTable;
}

impl<T: ResourceView> ResourceView for &mut T {
    fn table(&mut self) -> &mut ResourceTable {
        (**self).table()
    }
}

fn is_host_resource_type(ty: ResourceType) -> bool {
    ty == resource_types::pollable()
        || ty == resource_types::input_stream()
        || ty == resource_types::output_stream()
        || ty == resource_types::io_error()
        || ty == resource_types::tcp_socket()
    // TODO: extend
}

pub enum Cmd {
    ApplyManifest {
        manifest: Manifest<Bytes>,
        deadline: u64,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
    Invoke {
        name: Box<str>,
        invocation: WorkloadInvocation,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
}

#[derive(Debug)]
pub struct WorkloadInvocation {
    pub span: tracing::Span,
    pub payload: WorkloadInvocationPayload,
}

#[derive(Debug)]
pub struct DynamicWorkloadInvocationResult {
    pub store: Store<Ctx>,
    pub params: Vec<Val>,
    pub results: Vec<Val>,
    pub tx: oneshot::Sender<Store<Ctx>>,
}

#[derive(Debug)]
pub struct DynamicWorkloadInvocation {
    pub idx: ::wasmtime::component::ComponentExportIndex,
    pub store: Store<Ctx>,
    pub params: Vec<Val>,
    pub results: Vec<Val>,
    pub result: oneshot::Sender<anyhow::Result<DynamicWorkloadInvocationResult>>,
}

#[derive(Debug)]
pub enum WorkloadInvocationPayload {
    Dynamic(oneshot::Sender<(Store<Ctx>, oneshot::Sender<DynamicWorkloadInvocation>)>),
    WasiHttpHandler {
        request: wasi::http::IncomingRequest,
        response: wasi::http::ResponseOutparam,
        result: oneshot::Sender<anyhow::Result<()>>,
    },
}

#[derive(Debug)]
pub struct PluginInvocation {
    span: tracing::Span,
    payload: PluginInvocationPayload,
}

#[derive(Debug)]
pub enum PluginInvocationPayload {
    Dynamic {
        workload: Arc<str>,
        instance: Arc<str>,
        name: Arc<str>,
        params: Vec<Val>,
        results: Vec<Val>,
        ty: ::wasmtime::component::types::ComponentFunc,
        result: oneshot::Sender<anyhow::Result<(Vec<Val>, Vec<Val>)>>,
    },
}

struct Workload<'scope> {
    thread: ScopedJoinHandle<'scope, ()>,
    invocations: mpsc::Sender<WorkloadInvocation>,
}

struct Service<'scope> {
    thread: ScopedJoinHandle<'scope, ()>,
    permit: SemaphorePermit<'scope>,
    frozen: AtomicBool,
}

type PluginCallFn = unsafe fn(
    workload: *const c_char,
    instance: *const c_char,
    name: *const c_char,
    ty: *const types::ComponentFunc,
    params: *const Val,
    results: *mut Val,
) -> bool;

type AddToLinkerFn = unsafe fn(
    engine: *const wasmtime::Engine,
    linker: *mut LinkerInstance<Ctx>,
    workload: *const c_char,
    instance: *const c_char,
    ty: *const types::ComponentInstance,
) -> bool;

extern "C" fn func_new(
    linker: *mut LinkerInstance<Ctx>,
    name: *const c_char,
    f: unsafe extern "C" fn(
        param_ptr: *const Val,
        param_len: usize,
        result_ptr: *mut Val,
        result_len: usize,
    ) -> bool,
) -> bool {
    false
}

extern "C" fn func_new_async(
    linker: *mut LinkerInstance<Ctx>,
    name: *const c_char,
    f: unsafe extern "C" fn(
        param_ptr: *const Val,
        param_len: usize,
        result_ptr: *mut Val,
        result_len: usize,
    ) -> FfiFuture<bool>,
) -> bool {
    false
}

struct Plugin<'scope> {
    thread: ScopedJoinHandle<'scope, ()>,
    invocations: mpsc::Sender<PluginInvocation>,
    add_to_linker: AddToLinkerFn,
}

async fn handle_workload_import(
    mut store: impl AsContextMut<Data = Ctx>,
    params: &[Val],
    results: &mut [Val],
    idx: ComponentExportIndex,
    invocations: mpsc::Sender<WorkloadInvocation>,
) -> anyhow::Result<()> {
    let (tx, rx) = oneshot::channel();
    invocations
        .send(WorkloadInvocation {
            span: Span::current(),
            payload: WorkloadInvocationPayload::Dynamic(tx),
        })
        .await
        .context("failed to send dynamic invocation")?;

    let (mut target_store, tx) = rx.await.context("sender channel closed")?;
    let mut params_buf = Vec::with_capacity(params.len());
    for v in params {
        let v = workload::value::lower(&mut store, &mut target_store, v)?;
        params_buf.push(v);
    }

    let results_buf = vec![Val::Bool(false); results.len()];
    let (result_tx, result_rx) = oneshot::channel();
    if let Err(_) = tx.send(DynamicWorkloadInvocation {
        idx,
        store: target_store,
        params: params_buf,
        results: results_buf,
        result: result_tx,
    }) {
        bail!("dynamic workload invocation receiver dropped")
    }
    let result = result_rx.await.context("sender channel closed")?;
    let DynamicWorkloadInvocationResult {
        store: mut target_store,
        params: _,
        results: results_buf,
        tx,
    } = result.context("failed to invoke function")?;
    for (v, result) in results_buf.into_iter().zip(results) {
        *result = workload::value::lift(&mut store, &mut target_store, v)?;
    }
    if let Err(..) = tx.send(target_store) {
        debug!("dynamic workload invocation table receiver dropped")
    }
    Ok(())
}

fn link_workload_import(
    linker: &mut LinkerInstance<'_, Ctx>,
    name: impl Into<Arc<str>>,
    target: impl Into<Arc<str>>,
    idx: ComponentExportIndex,
    invocations: mpsc::Sender<WorkloadInvocation>,
) -> anyhow::Result<()> {
    let name = name.into();
    let target = target.into();
    linker.func_new_async(&Arc::clone(&name), move |store, params, results| {
        let span = debug_span!(parent: Span::current(), "workload_import", "target" = ?target, "name" = ?name);
        Box::new(handle_workload_import(store, params, results, idx, invocations.clone()).instrument(span))
    })
}

fn compile_component(
    engine: &wasmtime::Engine,
    wasm: &[u8],
    adapter: &[u8],
) -> anyhow::Result<Component> {
    if wasmparser::Parser::is_core_wasm(&wasm) {
        let enc = wit_component::ComponentEncoder::default()
            .validate(true)
            .module(wasm)
            .context("failed to set core component module")?;
        let mut enc = enc
            .adapter(WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, adapter)
            .context("failed to add WASI adapter")?;
        let wasm = enc.encode().context("failed to encode a component")?;
        Component::new(engine, wasm)
    } else {
        Component::new(engine, wasm)
    }
    .context("failed to compile component")
}

struct CompiledWorkload {
    span: tracing::Span,
    component: Component,
    linker: Linker<Ctx>,
    runtime: tokio::runtime::Runtime,
    thread_builder: thread::Builder,
    pool_size: usize,
    max_instances: usize,
    execution_time_ms: Option<u64>,
}

impl CompiledWorkload {
    #[instrument(level = "debug", skip_all)]
    fn resolve_workload_import(
        &mut self,
        instance_name: &str,
        import_ty: types::ComponentInstance,
        target: impl Into<Arc<str>>,
        ResolvedWorkload {
            component,
            ty,
            invocations,
        }: &ResolvedWorkload,
    ) -> anyhow::Result<()> {
        let target = target.into();
        let engine = self.component.engine();
        let mut linker = self
            .linker
            .instance(instance_name)
            .with_context(|| format!("failed to instantiate `{instance_name}` in the linker"))?;
        let types::ComponentItem::ComponentInstance(ty) = ty
            .get_export(component.engine(), instance_name)
            .with_context(|| format!("export `{instance_name}` not found on component type"))?
        else {
            bail!("export `{instance_name}` type is not an instance")
        };
        let (_, instance_idx) = component
            .export_index(None, instance_name)
            .with_context(|| format!("export `{instance_name}` not found on component"))?;
        for (name, import_ty) in import_ty.exports(engine) {
            match import_ty {
                types::ComponentItem::ComponentFunc(..) => {
                    let ty = ty
                        .get_export(component.engine(), name)
                        .with_context(|| format!("export `{instance_name}.{name}` not found"))?;
                    let types::ComponentItem::ComponentFunc(ty) = ty else {
                        bail!("export `{instance_name}#{name}` is not a function");
                    };
                    let (_, func_idx) = component
                        .export_index(Some(&instance_idx), name)
                        .with_context(|| {
                            format!("export `{instance_name}.{name}` not found on component")
                        })?;
                    let invocations = invocations.clone();
                    link_workload_import(
                        &mut linker,
                        name,
                        Arc::clone(&target),
                        func_idx,
                        invocations,
                    )
                    .with_context(|| {
                        format!("failed to define `{instance_name}#{name}` function import")
                    })?;
                }
                types::ComponentItem::Resource(..) => {
                    let ty = ty
                        .get_export(component.engine(), name)
                        .with_context(|| format!("export `{instance_name}.{name}` not found"))?;
                    let types::ComponentItem::Resource(ty) = ty else {
                        bail!("export `{instance_name}.{name}` is not a resource");
                    };
                    if !is_host_resource_type(ty) {
                        linker
                            .resource(name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                            .with_context(|| {
                                format!("failed to define `{instance_name}.{name}` resource import")
                            })?;
                    }
                }
                types::ComponentItem::CoreFunc(..)
                | types::ComponentItem::Module(..)
                | types::ComponentItem::Component(..)
                | types::ComponentItem::ComponentInstance(..)
                | types::ComponentItem::Type(..) => {}
            }
        }
        Ok(())
    }

    #[instrument(skip(self, workloads, plugins, imports, names))]
    fn resolve_imports(
        &mut self,
        name: &str,
        workloads: &[WorkloadPre],
        plugins: &HashMap<Box<str>, Plugin>,
        imports: BTreeMap<Box<str>, config::component::Import>,
        names: &[Box<str>],
    ) -> anyhow::Result<Vec<UnresolvedImport>> {
        let mut unresolved = Vec::with_capacity(imports.len());
        for (instance_name, import) in imports {
            let Some(types::ComponentItem::ComponentInstance(ty)) = self
                .component
                .component_type()
                .get_import(self.component.engine(), &instance_name)
            else {
                info!(instance_name, "skip unused import configuration");
                continue;
            };
            match import {
                config::component::Import::Workload { target } => {
                    let target_idx = names
                        .binary_search(&target)
                        .map_err(|_| anyhow!("import target component `{target}` not found"))?;
                    match &workloads[target_idx] {
                        WorkloadPre::Compiled { .. } | WorkloadPre::Unresolved { .. } => {
                            unresolved.push(UnresolvedImport {
                                name: instance_name,
                                ty,
                                target,
                                target_idx,
                            });
                            continue;
                        }
                        WorkloadPre::Resolved(resolved) => {
                            self.resolve_workload_import(&instance_name, ty, target, resolved)?
                        }
                        WorkloadPre::Taken => bail!("cycle in workload resolution"),
                    }
                }
                config::component::Import::Plugin { target } => {
                    let Plugin { invocations, .. } = plugins
                        .get(&target)
                        .with_context(|| format!("plugin `{target}` not found"))?;
                    //let mut linker = self.linker.instance(&instance_name).with_context(|| {
                    //    format!("failed to instantiate `{instance_name}` in the linker")
                    //})?;

                    ////linker
                    ////    .resource("database", ResourceType::host::<()>(), |_, _| Ok(()))
                    ////    .unwrap();

                    //let instance_name_c =
                    //    CString::new(&*instance_name).context("failed to construct C string")?;
                    //let name_c = CString::new(name).context("failed to construct C string")?;
                    //if !unsafe {
                    //    add_to_linker(
                    //        self.component.engine(),
                    //        &mut linker,
                    //        &ty,
                    //        instance_name_c.as_ptr(),
                    //        name_c.as_ptr(),
                    //    )
                    //} {
                    //    bail!("plugin `{target}` failed to link `{instance_name}` for workload `{name}`")
                    //}
                    //eprintln!("done add to linker");
                    // TODO: get plugin
                    // TODO: `call`
                    //bail!("TODO: connect plugin {target:?}");
                }
            }
        }
        Ok(unresolved)
    }
}

// TODO
#[allow(unused)]
async fn handle_plugin(
    rt: &tokio::runtime::Handle,
    call: &Symbol<'_, PluginCallFn>,
    mut invocations: mpsc::Receiver<PluginInvocation>,
) {
    let Some(PluginInvocation { span, payload }) = invocations.recv().await else {
        debug!("invocation channel closed, plugin thread exiting");
        return;
    };
    let _span = span.enter();
}

struct ResolvedWorkload {
    component: Component,
    ty: types::Component,
    invocations: mpsc::Sender<WorkloadInvocation>,
}

#[derive(Debug)]
struct UnresolvedImport {
    name: Box<str>,
    ty: types::ComponentInstance,
    target: Box<str>,
    target_idx: usize,
}

#[derive(Default)]
enum WorkloadPre {
    #[default]
    Taken,
    Compiled(CompiledWorkload),
    Unresolved {
        workload: CompiledWorkload,
        imports: Vec<UnresolvedImport>,
    },
    Resolved(ResolvedWorkload),
}

pub struct Engine {
    engine: wasmtime::Engine,
    max_instances: usize,
    instance_permits: Arc<Semaphore>,
}

struct EngineState<'scope> {
    plugins: HashMap<Box<str>, Plugin<'scope>>,
    services: HashMap<Box<str>, Service<'scope>>,
    workloads: HashMap<Box<str>, Workload<'scope>>,
    shutdown: watch::Sender<u64>,
}

impl Default for EngineState<'_> {
    fn default() -> Self {
        let (tx, _) = watch::channel(0);
        Self {
            plugins: HashMap::default(),
            services: HashMap::default(),
            workloads: HashMap::default(),
            shutdown: tx,
        }
    }
}

impl Engine {
    pub fn new(engine: wasmtime::Engine, max_instances: usize) -> Self {
        Self {
            engine,
            max_instances,
            instance_permits: Arc::new(Semaphore::new(max_instances)),
        }
    }

    async fn handle_workload(
        &self,
        pre: InstancePre<Ctx>,
        mut invocations: mpsc::Receiver<WorkloadInvocation>,
        max_instances: usize,
        pool_size: usize,
        execution_time_ms: Option<u64>,
        shutdown: watch::Receiver<u64>,
    ) {
        let instance_permits = Arc::new(Semaphore::new(max_instances));
        let (instance_tx, mut instance_rx) = NonZeroUsize::new(pool_size)
            .map(|n| mpsc::channel(n.into()))
            .unzip();
        let mut tasks = JoinSet::new();
        loop {
            enum PooledInstance {
                Pre {
                    pre: InstancePre<Ctx>,
                    store: Store<Ctx>,
                },
                Instance {
                    instance: Instance,
                    store: Store<Ctx>,
                },
            }
            while let Some(res) = tasks.try_join_next() {
                if let Err(err) = res {
                    error!(?err, "workload task panicked");
                }
            }

            debug!("awaiting next invocation");
            let Some(WorkloadInvocation { span, payload }) = invocations.recv().await else {
                debug!("invocation channel closed, workload thread exiting");
                return;
            };
            let _span = span.enter();

            debug!("acquiring workload semaphore");
            let Ok(workload_permit) = Arc::clone(&instance_permits).acquire_owned().await else {
                debug!("workload semaphore closed, workload thread exiting");
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "workload task panicked");
                    }
                }
                return;
            };

            debug!("acquiring engine semaphore");
            let Ok(engine_permit) = Arc::clone(&self.instance_permits).acquire_owned().await else {
                debug!("engine semaphore closed, workload thread exiting");
                while let Some(res) = tasks.join_next().await {
                    if let Err(err) = res {
                        error!(?err, "workload task panicked");
                    }
                }
                return;
            };

            let instance = instance_rx
                .as_mut()
                .and_then(|rx| {
                    rx.try_recv()
                        .ok()
                        .map(|(instance, store)| PooledInstance::Instance { instance, store })
                })
                .unwrap_or_else(|| {
                    debug!("initializing a new instance");
                    let mut store = self.new_store(execution_time_ms, shutdown.clone());
                    PooledInstance::Pre {
                        pre: pre.clone(),
                        store,
                    }
                });
            let instance_tx = instance_tx.clone();
            tasks.spawn(
                async move {
                    let (instance, mut store) = match instance {
                        PooledInstance::Pre { pre, mut store } => {
                            let instance = pre
                                .instantiate_async(&mut store)
                                .await
                                .context("failed to instantiate component");
                            (instance, store)
                        }
                        PooledInstance::Instance { instance, store } => (Ok(instance), store),
                    };
                    match payload {
                        WorkloadInvocationPayload::Dynamic(tx) => {
                            let (invocation_tx, invocation_rx) = oneshot::channel();
                            if let Err((store, ..)) = tx.send((store, invocation_tx)) {
                                debug!("store receiver closed");
                                if let Ok(instance) = instance {
                                    instance_tx.map(|tx| tx.try_send((instance, store)));
                                }
                                return;
                            }
                            let Ok(DynamicWorkloadInvocation {
                                idx,
                                mut store,
                                params,
                                results,
                                result,
                            }) = invocation_rx.await
                            else {
                                debug!("invocation sender closed");
                                return;
                            };

                            let (store_tx, store_rx) = oneshot::channel();
                            match instance {
                                Ok(instance) => {
                                    match handle_dynamic(
                                        &mut store, &instance, idx, params, results,
                                    )
                                    .await
                                    {
                                        Ok((params, results)) => {
                                            if let Err(..) =
                                                result.send(Ok(DynamicWorkloadInvocationResult {
                                                    store,
                                                    params,
                                                    results,
                                                    tx: store_tx,
                                                }))
                                            {
                                                debug!("result receiver closed");
                                            }
                                            let Ok(store) = store_rx.await else {
                                                debug!("store sender closed");
                                                return;
                                            };
                                            instance_tx.map(|tx| tx.try_send((instance, store)));
                                        }
                                        Err(err) => {
                                            instance_tx.map(|tx| tx.try_send((instance, store)));
                                            if let Err(..) = result.send(Err(err)) {
                                                debug!("result receiver closed");
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    if let Err(..) = result.send(Err(err)) {
                                        debug!("result receiver closed");
                                    }
                                }
                            }
                        }
                        WorkloadInvocationPayload::WasiHttpHandler {
                            request,
                            response,
                            result,
                        } => {
                            if let Err(..) = result.send({
                                match instance {
                                    Ok(instance) => {
                                        let res =
                                            handle_http(&mut store, &instance, request, response)
                                                .await;
                                        if let Ok(()) = res {
                                            instance_tx.map(|tx| tx.try_send((instance, store)));
                                        }
                                        res
                                    }
                                    Err(err) => Err(err),
                                }
                            }) {
                                debug!("response receiver channel closed");
                            }
                        }
                    }
                    drop(engine_permit);
                    drop(workload_permit);
                }
                .in_current_span(),
            );
        }
    }

    #[instrument(level = "debug", skip_all, fields(name))]
    fn instantiate_workload<'scope, 'env>(
        &'env self,
        s: &'scope thread::Scope<'scope, 'env>,
        state: &mut EngineState<'scope>,
        name: &Box<str>,
        CompiledWorkload {
            span,
            component,
            linker,
            runtime,
            thread_builder,
            pool_size,
            max_instances,
            execution_time_ms,
        }: CompiledWorkload,
    ) -> anyhow::Result<ResolvedWorkload> {
        debug!("pre-instantiating component");
        let ty = linker
            .substituted_component_type(&component)
            .context("failed to derive component type")?;
        let pre = linker
            .instantiate_pre(&component)
            .context("failed to pre-instantiate component")?;

        let (invocations_tx, invocations_rx) = mpsc::channel(max_instances);
        let shutdown = state.shutdown.subscribe();
        let thread = thread_builder
            .spawn_scoped(&s, {
                move || {
                    runtime.block_on(
                        self.handle_workload(
                            pre,
                            invocations_rx,
                            max_instances,
                            pool_size,
                            execution_time_ms,
                            shutdown,
                        )
                        .instrument(span),
                    )
                }
            })
            .context("failed to spawn thread")?;
        state.workloads.insert(
            name.clone(),
            Workload {
                thread,
                invocations: invocations_tx.clone(),
            },
        );
        Ok(ResolvedWorkload {
            component,
            ty,
            invocations: invocations_tx,
        })
    }

    #[instrument(level = "debug", skip_all)]
    fn new_store(&self, mut budget: Option<u64>, shutdown: watch::Receiver<u64>) -> Store<Ctx> {
        let now = EPOCH_MONOTONIC_NOW.load(Ordering::Relaxed);
        let deadline = budget.map(|n| now.saturating_add(n)).unwrap_or_default();
        let mut store = Store::new(&self.engine, Ctx::new(deadline, shutdown.clone()));

        // every 100ms
        let mut n = 100;
        if let Some(budget) = budget.as_mut() {
            if let Some(next) = budget.checked_sub(100) {
                *budget = next;
            } else {
                n = *budget;
                *budget = 0;
            }
        };
        store.set_epoch_deadline(n);
        store.epoch_deadline_callback(move |_| {
            let n = if let Some(budget) = budget.as_mut() {
                if *budget == 0 {
                    bail!("execution time budget exhausted")
                }
                if let Some(next) = budget.checked_sub(100) {
                    *budget = next;
                    100
                } else {
                    let n = *budget;
                    *budget = 0;
                    n
                }
            } else {
                100
            };
            match shutdown.has_changed() {
                Ok(true) => {
                    let deadline = shutdown.borrow();
                    debug!(?deadline, "shutdown requested");
                    let d = deadline.saturating_sub(EPOCH_MONOTONIC_NOW.load(Ordering::Relaxed));
                    if d == 0 {
                        bail!("shutdown time budget exhausted in the guest")
                    }
                    if let Some(next) = d.checked_sub(100) {
                        budget = Some(next);
                        Ok(UpdateDeadline::Yield(100))
                    } else {
                        budget = Some(0);
                        Ok(UpdateDeadline::Yield(d))
                    }
                }
                Ok(false) => Ok(UpdateDeadline::Yield(n)),
                Err(..) => {
                    debug!("shutdown channel dropped, forcing shutdown");
                    bail!("forced shutdown in the guest")
                }
            }
        });
        store
    }

    async fn handle_service(
        &self,
        shutdown: watch::Receiver<u64>,
        mut store: Store<Ctx>,
        mut cmd: Command,
        pre: CommandPre<Ctx>,
    ) {
        let should_exit = || {
            if let Ok(false) = shutdown.has_changed() {
                false
            } else {
                debug!("shutdown requested, service thread exiting");
                true
            }
        };
        loop {
            if should_exit() {
                return;
            }
            match cmd.wasi_cli_run().call_run(&mut store).await {
                Ok(Ok(())) => info!("service returned success"),
                Ok(Err(())) => error!("service returned an error"),
                Err(err) => {
                    if let Some(I32Exit(code)) = err.downcast_ref() {
                        if *code != 0 {
                            warn!(?code, "service exited with non-zero code")
                        } else {
                            info!("service exited with zero code")
                        }
                    } else {
                        error!(?err, "failed to run service")
                    }
                }
                Err(err) => {
                    error!(?err, "failed to call `wasi:cli/run#run` on the service")
                }
            }

            if should_exit() {
                return;
            }
            store = self.new_store(None, shutdown.clone());
            cmd = match pre.instantiate_async(&mut store).await {
                Ok(cmd) => cmd,
                Err(err) => {
                    error!(
                        ?err,
                        "failed to instantiate service component, thread exiting"
                    );
                    return;
                }
            };

            if should_exit() {
                return;
            }
            debug!("restarting service in 10 seconds");
            sleep(Duration::from_secs(10)).await;
        }
    }

    #[instrument(level = "debug", skip_all, fields(name))]
    fn instantiate_service<'scope, 'env>(
        &'env self,
        s: &'scope thread::Scope<'scope, 'env>,
        state: &mut EngineState<'scope>,
        name: Box<str>,
        config::Service {
            component: config::Component { src: wasm, imports },
            ..
        }: config::Service<Bytes>,
    ) -> anyhow::Result<()> {
        let component =
            compile_component(&self.engine, &wasm, WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER)?;

        let mut linker = Linker::<Ctx>::new(&self.engine);

        bindings::wasi::io::error::add_to_linker(&mut linker, |cx| cx)?;
        bindings::wasi::io::poll::add_to_linker(&mut linker, |cx| cx)?;
        bindings::wasi::io::streams::add_to_linker(&mut linker, |cx| cx)?;

        wasi::add_to_linker(&mut linker)?;

        // TODO: Resolve imports

        let pre = linker
            .instantiate_pre(&component)
            .context("failed to pre-instantiate component")?;
        let pre = CommandPre::new(pre).context("failed to pre-instantiate `wasi:cli/command`")?;

        let thread_name = format!("wex-service-{name}");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name(&thread_name)
            .build()
            .context("failed to build service Tokio runtime")?;
        let permit = runtime
            .block_on(async { self.instance_permits.acquire().await })
            .context("failed to acquire service instance semaphore permit")?;
        let shutdown = state.shutdown.subscribe();
        let mut store = self.new_store(None, shutdown.clone());
        let cmd = runtime
            .block_on(pre.instantiate_async(&mut store))
            .context("failed to instantiate component")?;

        let span = info_span!("handle_service", name);
        let thread = thread::Builder::new()
            .name(thread_name)
            .spawn_scoped(&s, {
                move || {
                    runtime.block_on(
                        self.handle_service(shutdown, store, cmd, pre)
                            .instrument(span),
                    )
                }
            })
            .context("failed to spawn thread")?;
        state.services.insert(
            name,
            Service {
                thread,
                permit,
                frozen: AtomicBool::new(false),
            },
        );
        Ok(())
    }

    fn apply_manifest<'scope, 'env>(
        &'env self,
        s: &'scope thread::Scope<'scope, 'env>,
        state: &mut EngineState<'scope>,
        deadline: u64,
        Manifest {
            plugins,
            workloads,
            services,
        }: Manifest<Bytes>,
    ) -> anyhow::Result<()> {
        let workload_count = workloads.len();
        let service_count = services.len();
        let (shutdown_tx, shutdown_rx) = watch::channel(0);
        let mut next = EngineState {
            plugins: HashMap::with_capacity(plugins.len()),
            services: HashMap::with_capacity(service_count),
            workloads: HashMap::with_capacity(workload_count),
            shutdown: shutdown_tx,
        };
        let plugins = plugins
            .into_iter()
            .map(|(name, conf)| match conf {
                config::Plugin { src } => {
                    let path = Path::new(&*src);
                    let lib = if path.has_root()
                        || path.extension().is_some()
                        || path.parent() != Some(Path::new(""))
                    {
                        unsafe { Library::new(path) }
                    } else {
                        unsafe { Library::new(libloading::library_filename(&*src)) }
                    }
                    .with_context(|| format!("failed to load dynamic library by `{src}`"))?;

                    let lib = Arc::new(lib);
                    let add_to_linker: Symbol<AddToLinkerFn> = unsafe {
                        lib.get(b"wex_plugin_add_to_linker").with_context(|| {
                            format!("failed to lookup `wex_plugin_add_to_linker` in dynamic library `{src}`")
                        })?
                    };
                    let add_to_linker = *add_to_linker;
                    let thread_name = format!("wex-plugin-{name}");
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .max_blocking_threads(64)
                        .enable_io()
                        .enable_time()
                        .thread_name(&thread_name)
                        .build()
                        .context("failed to build plugin Tokio runtime")?;
                    let thread_builder = thread::Builder::new().name(thread_name);
                    let (invocations_tx, mut invocations_rx) = mpsc::channel(64);
                    let span = info_span!("handle_plugin", name);
                    let thread = thread_builder
                        .spawn_scoped(&s, {
                            move || {
                    //            let handle = runtime.handle();
                    //            handle.block_on(
                    //                async move {
                    //                    while let Some(PluginInvocation {
                    //                        span,
                    //                        payload:
                    //                            PluginInvocationPayload::Dynamic {
                    //                                workload,
                    //                                instance,
                    //                                name,
                    //                                params,
                    //                                mut results,
                    //                                ty,
                    //                                result,
                    //                            },
                    //                    }) = invocations_rx.recv().await
                    //                    {
                    //                        let _span = span.enter();
                    //                        let lib = Arc::clone(&lib);
                    //                        handle.spawn_blocking(move || unsafe {
                    //                            call(
                    //                                c"workload".as_ptr(),
                    //                                c"instance".as_ptr(),
                    //                                c"name".as_ptr(),
                    //                                &ty,
                    //                                params.as_ptr(),
                    //                                results.as_mut_ptr(),
                    //                            );
                    //                            drop(lib);
                    //                        });
                    //                    }
                    //                    debug!("invocation channel closed, plugin thread exiting");
                    //                }
                    //                .instrument(span),
                    //            );
                    //            runtime.shutdown_background();
                            }
                        })
                        .context("failed to spawn thread")?;
                    Ok((
                        name,
                        Plugin {
                            thread,
                            invocations: invocations_tx,
                            add_to_linker,
                        },
                    ))
                }
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        let mut workload_names = Vec::with_capacity(workload_count);
        let mut workload_pre_imports = Vec::with_capacity(workload_count);
        let mut workload_pres = Vec::with_capacity(workload_count);
        for (
            name,
            config::Workload {
                component: config::Component { src: wasm, imports },
                pool,
                limits:
                    config::workload::Limits {
                        instances: max_instances,
                        execution_time_ms,
                    },
                ..
            },
        ) in workloads
        {
            let component =
                compile_component(&self.engine, &wasm, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER)?;

            let mut linker = Linker::<Ctx>::new(&self.engine);

            bindings::wasi::io::error::add_to_linker(&mut linker, |cx| cx)?;
            bindings::wasi::io::poll::add_to_linker(&mut linker, |cx| cx)?;
            bindings::wasi::io::streams::add_to_linker(&mut linker, |cx| cx)?;

            wasi::add_to_linker(&mut linker)?;

            let thread_name = format!("wex-workload-{name}");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .thread_name(&thread_name)
                .build()
                .context("failed to build workload Tokio runtime")?;

            let max_instances = max_instances
                .map(|max| max.min(self.max_instances))
                .unwrap_or(self.max_instances);
            let thread_builder = thread::Builder::new().name(thread_name);

            let span = info_span!("handle_workload", name);
            workload_names.push(name);
            workload_pre_imports.push(imports);
            workload_pres.push(WorkloadPre::Compiled(CompiledWorkload {
                span,
                component,
                linker,
                runtime,
                thread_builder,
                pool_size: pool.min(max_instances),
                max_instances,
                execution_time_ms,
            }));
        }

        let mut unresolved = Vec::default();
        for (idx, imports) in workload_pre_imports.into_iter().enumerate() {
            let name = &workload_names[idx];

            let WorkloadPre::Compiled(mut workload) = mem::take(&mut workload_pres[idx]) else {
                bail!("corrupted workload list")
            };
            let unresolved_imports = workload
                .resolve_imports(
                    name,
                    &workload_pres,
                    &next.plugins,
                    imports,
                    &workload_names,
                )
                .with_context(|| format!("failed to resolve imports of workload `{name}`"))?;
            if !unresolved_imports.is_empty() {
                unresolved.push(idx);
                workload_pres[idx] = WorkloadPre::Unresolved {
                    workload,
                    imports: unresolved_imports,
                };
                continue;
            }
            let resolved = self
                .instantiate_workload(s, &mut next, name, workload)
                .with_context(|| format!("failed to instantiate workload `{name}`"))?;
            workload_pres[idx] = WorkloadPre::Resolved(resolved);
        }
        while !unresolved.is_empty() {
            let mut unresolved_next = Vec::with_capacity(unresolved.len());
            for idx in &unresolved {
                let WorkloadPre::Unresolved {
                    mut workload,
                    imports,
                } = mem::take(&mut workload_pres[*idx])
                else {
                    bail!("corrupted unresolved component list")
                };
                let mut unresolved_imports = Vec::with_capacity(imports.len());
                for UnresolvedImport {
                    name,
                    ty,
                    target,
                    target_idx,
                } in imports
                {
                    match &workload_pres[target_idx] {
                        WorkloadPre::Compiled { .. } | WorkloadPre::Unresolved { .. } => {
                            unresolved_imports.push(UnresolvedImport {
                                name,
                                ty,
                                target,
                                target_idx,
                            });
                            continue;
                        }
                        WorkloadPre::Resolved(resolved) => {
                            workload.resolve_workload_import(&name, ty, target, &resolved)?
                        }
                        WorkloadPre::Taken => bail!("cycle in workload resolution"),
                    }
                }
                if !unresolved_imports.is_empty() {
                    unresolved_next.push(*idx);
                    workload_pres[*idx] = WorkloadPre::Unresolved {
                        workload,
                        imports: unresolved_imports,
                    };
                    continue;
                }
                let name = &workload_names[*idx];
                let resolved = self
                    .instantiate_workload(s, &mut next, name, workload)
                    .with_context(|| format!("failed to instantiate workload `{name}`"))?;
                workload_pres[*idx] = WorkloadPre::Resolved(resolved);
            }
            ensure!(
                unresolved != unresolved_next,
                "cycle in workload resolution"
            );
            unresolved = unresolved_next;
        }
        for (name, conf) in services {
            self.instantiate_service(s, &mut next, name, conf)?;
        }

        // Manifest applied, initiate shutdown
        state.shutdown.send_replace(deadline);

        let mut service_threads = Vec::with_capacity(state.services.len());
        for (name, Service { thread, .. }) in state.services.drain() {
            service_threads.push((name, thread))
        }
        let mut workload_threads = Vec::with_capacity(state.workloads.len());
        for (
            name,
            Workload {
                thread,
                invocations,
            },
        ) in state.workloads.drain()
        {
            drop(invocations);
            workload_threads.push((name, thread))
        }

        for (name, thread) in service_threads {
            debug!(name, "joining service thread");
            if let Err(err) = thread.join() {
                error!(?err, name, "service thread panicked")
            }
        }
        for (name, thread) in workload_threads {
            debug!(name, "joining workload thread");
            if let Err(err) = thread.join() {
                error!(?err, name, "workload thread panicked")
            }
        }
        debug!("updating engine state");
        *state = next;
        Ok(())
    }

    #[instrument(level = "debug", parent = span, skip(self, span, scheduled, payload))]
    fn invoke<'scope>(
        &self,
        scheduled: &HashMap<Box<str>, Workload<'scope>>,
        name: &str,
        WorkloadInvocation { span, payload }: WorkloadInvocation,
    ) -> anyhow::Result<()> {
        let workload = scheduled.get(name).context("workload not found")?;
        match workload.invocations.try_send(WorkloadInvocation {
            span: Span::current(),
            payload,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(..)) => bail!("workload queue full"),
            Err(mpsc::error::TrySendError::Closed(..)) => bail!("workload thread exited"),
        }
    }

    pub fn handle_commands(&self, mut cmds: mpsc::Receiver<Cmd>) -> anyhow::Result<()> {
        thread::scope(|s| {
            let mut state = EngineState::default();
            let mut buf = vec![];
            while cmds.blocking_recv_many(&mut buf, cmds.max_capacity()) > 0 {
                for cmd in buf.drain(..) {
                    match cmd {
                        Cmd::ApplyManifest {
                            manifest,
                            deadline,
                            result,
                        } => result
                            .send(self.apply_manifest(s, &mut state, deadline, manifest))
                            .map_err(|_| ()),
                        Cmd::Invoke {
                            name,
                            invocation,
                            result,
                        } => result
                            .send(self.invoke(&state.workloads, &name, invocation))
                            .map_err(|_| ()),
                    }
                    .map_err(|()| anyhow!("main thread exited"))?;
                }
            }
            anyhow::Ok(())
        })
    }
}

#[derive(Debug)]
pub enum WithChildren<T> {
    Parent(Arc<std::sync::RwLock<T>>),
    Child(Arc<std::sync::RwLock<T>>),
}

impl<T: Default> Default for WithChildren<T> {
    fn default() -> Self {
        Self::Parent(Arc::default())
    }
}

impl<T> WithChildren<T> {
    pub fn new(v: T) -> Self {
        Self::Parent(Arc::new(std::sync::RwLock::new(v)))
    }

    fn as_arc(&self) -> &Arc<std::sync::RwLock<T>> {
        match self {
            Self::Parent(v) | Self::Child(v) => v,
        }
    }

    fn into_arc(self) -> Arc<std::sync::RwLock<T>> {
        match self {
            Self::Parent(v) | Self::Child(v) => v,
        }
    }

    /// Returns a new child referencing the same value as `self`.
    pub fn child(&self) -> Self {
        Self::Child(Arc::clone(self.as_arc()))
    }

    /// Clone `T` and return the clone as a parent reference.
    /// Fails if the inner lock is poisoned.
    pub fn clone(&self) -> wasmtime::Result<Self>
    where
        T: Clone,
    {
        if let Ok(v) = self.as_arc().read() {
            Ok(Self::Parent(Arc::new(std::sync::RwLock::new(v.clone()))))
        } else {
            bail!("lock poisoned")
        }
    }

    /// If this is the only reference to `T` then unwrap it.
    /// Otherwise, clone `T` and return the clone.
    /// Fails if the inner lock is poisoned.
    pub fn unwrap_or_clone(self) -> wasmtime::Result<T>
    where
        T: Clone,
    {
        match Arc::try_unwrap(self.into_arc()) {
            Ok(v) => v.into_inner().map_err(|_| anyhow!("lock poisoned")),
            Err(v) => {
                if let Ok(v) = v.read() {
                    Ok(v.clone())
                } else {
                    bail!("lock poisoned")
                }
            }
        }
    }

    pub fn get(&self) -> wasmtime::Result<impl Deref<Target = T> + '_> {
        self.as_arc().read().map_err(|_| anyhow!("lock poisoned"))
    }

    pub fn get_mut(&mut self) -> wasmtime::Result<Option<impl DerefMut<Target = T> + '_>> {
        match self {
            Self::Parent(v) => {
                if let Ok(v) = v.write() {
                    Ok(Some(v))
                } else {
                    bail!("lock poisoned")
                }
            }
            Self::Child(..) => Ok(None),
        }
    }
}
