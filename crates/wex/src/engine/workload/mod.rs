use core::fmt::{self, Debug};
use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt as _};
use tokio::sync::{mpsc::UnboundedSender, oneshot, watch, Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::{debug, instrument, trace};
use wasmtime::component::{ComponentExportIndex, Instance, ResourceTable, Val};
use wasmtime::Store;

use crate::engine::wasi::cli::{WasiCliCtx, WasiCliView};
use crate::engine::wasi::clocks::{WasiClocksCtx, WasiClocksView};
use crate::engine::wasi::filesystem::{WasiFilesystemCtx, WasiFilesystemView};
use crate::engine::wasi::http::empty_body;
use crate::engine::wasi::http::{WasiHttpCtx, WasiHttpView};
use crate::engine::wasi::random::{WasiRandomCtx, WasiRandomView};
use crate::engine::wasi::sockets::{WasiSocketsCtx, WasiSocketsView};
use crate::engine::{wasi, ResourceView};

pub mod value;

pub struct Ctx {
    pub table: ResourceTable,
    pub http: WasiHttpCtx,
    pub cli: WasiCliCtx,
    pub clocks: WasiClocksCtx,
    pub filesystem: WasiFilesystemCtx,
    pub random: WasiRandomCtx,
    pub sockets: WasiSocketsCtx,
    pub shutdown: watch::Receiver<u64>,
    pub deadline: u64,
}

impl Ctx {
    pub fn new(deadline: u64, shutdown: watch::Receiver<u64>) -> Self {
        Self {
            table: ResourceTable::default(),
            http: WasiHttpCtx::default(),
            cli: WasiCliCtx::default(),
            clocks: WasiClocksCtx::default(),
            filesystem: WasiFilesystemCtx::default(),
            random: WasiRandomCtx::default(),
            sockets: WasiSocketsCtx::default(),
            shutdown,
            deadline,
        }
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Ctx")
            .field(&self.table)
            .field(&self.http)
            // TODO: Add fields
            .finish_non_exhaustive()
    }
}

impl ResourceView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiCliView for Ctx {
    fn cli(&mut self) -> &WasiCliCtx {
        &self.cli
    }
}

impl WasiClocksView for Ctx {
    fn clocks(&mut self) -> &mut WasiClocksCtx {
        &mut self.clocks
    }
}

impl WasiFilesystemView for Ctx {
    fn filesystem(&self) -> &WasiFilesystemCtx {
        &self.filesystem
    }
}

impl WasiRandomView for Ctx {
    fn random(&mut self) -> &mut WasiRandomCtx {
        &mut self.random
    }
}

impl WasiSocketsView for Ctx {
    fn sockets(&self) -> &WasiSocketsCtx {
        &self.sockets
    }
}

impl WasiHttpView for Ctx {
    type Client = crate::engine::wasi::http::DefaultClient;

    fn http(&self) -> &WasiHttpCtx<Self::Client> {
        &self.http
    }
}

#[instrument(level = "trace", skip(store, instance))]
pub async fn handle_dynamic(
    mut store: &mut Store<Ctx>,
    instance: &Instance,
    idx: ComponentExportIndex,
    params: Vec<Val>,
    mut results: Vec<Val>,
) -> anyhow::Result<(Vec<Val>, Vec<Val>)> {
    let func = instance
        .get_func(&mut store, idx)
        .context("function not found")?;
    debug!(?params, "invoking dynamic export");
    func.call_async(&mut store, &params, &mut results)
        .await
        .context("failed to call function")?;
    debug!(?results, "invoked dynamic export");
    func.post_return_async(&mut store)
        .await
        .context("failed to execute post-return")?;
    Ok((params, results))
}

#[instrument(level = "trace", skip_all)]
pub async fn handle_http(
    mut store: &mut Store<Ctx>,
    instance: &Instance,
    request: impl Into<wasi::http::IncomingRequest>,
    response: wasi::http::ResponseOutparam,
) -> anyhow::Result<()> {
    trace!("instantiating `wasi:http/incoming-handler`");
    let proxy = crate::engine::bindings::exports::wasi::http::Proxy::new(&mut store, instance)
        .context("failed to instantiate `wasi:http/incoming-handler`")?;
    debug!("invoking `wasi:http/incoming-handler.handle`");
    proxy.handle(store, request, response).await
}
