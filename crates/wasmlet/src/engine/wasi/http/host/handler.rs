use wasmtime::component::Resource;

use crate::engine::bindings::wasi::http::outgoing_handler;
use crate::engine::bindings::wasi::http::types::ErrorCode;
use crate::engine::wasi::http::{FutureIncomingResponse, OutgoingRequest, RequestOptions};
use crate::engine::WithChildren;
use crate::Ctx;

impl outgoing_handler::Host for Ctx {
    fn handle(
        &mut self,
        _request: Resource<OutgoingRequest>,
        _options: Option<Resource<WithChildren<RequestOptions>>>,
    ) -> wasmtime::Result<Result<Resource<FutureIncomingResponse>, ErrorCode>> {
        todo!()
    }
}
