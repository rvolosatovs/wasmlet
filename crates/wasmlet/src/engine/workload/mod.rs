use anyhow::Context as _;
use tracing::{debug, instrument, trace};
use wasmtime::component::{ComponentExportIndex, Instance, Val};
use wasmtime::Store;

use crate::engine::{wasi, ResourceView};

pub mod value;

#[instrument(level = "trace", skip(store, instance))]
pub async fn handle_dynamic<T: ResourceView + Send>(
    mut store: &mut Store<T>,
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
pub async fn handle_http<T: ResourceView + Send>(
    mut store: &mut Store<T>,
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
