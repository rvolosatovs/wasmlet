use anyhow::Context as _;
use tracing::instrument;
use wasmtime::AsContextMut;

use crate::engine::wasi::http::{IncomingRequest, ResponseOutparam};
use crate::engine::ResourceView;

impl crate::engine::bindings::exports::wasi::http::Proxy {
    /// Call `handle` on [Proxy] getting a [Future] back.
    #[instrument(level = "debug", skip_all, ret)]
    pub async fn handle<T>(
        &self,
        mut store: impl AsContextMut<Data = T>,
        request: impl Into<IncomingRequest>,
        response: ResponseOutparam,
    ) -> wasmtime::Result<()>
    where
        T: ResourceView + Send,
    {
        let mut store = store.as_context_mut();
        let table = store.data_mut().table();
        let request = table
            .push(request.into())
            .context("failed to push request resource")?;
        let response = table
            .push(response)
            .context("failed to push outparam resource")?;
        self.wasi_http_incoming_handler()
            .call_handle(store, request, response)
            .await
    }
}
