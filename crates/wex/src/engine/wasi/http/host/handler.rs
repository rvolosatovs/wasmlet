use core::iter;

use std::sync::Arc;

use anyhow::bail;
use bytes::Bytes;
use futures::StreamExt as _;
use http::header::HOST;
use http::{HeaderValue, Uri};
use http_body_util::{BodyExt as _, BodyStream, StreamBody};
use tokio::sync::oneshot;
use tracing::debug;
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::http::outgoing_handler;
use crate::engine::bindings::wasi::http::types::ErrorCode;
use crate::engine::wasi::http::host::{delete_request, get_fields_inner, push_response};
use crate::engine::wasi::http::{
    empty_body, Body, BodyFrame, Client as _, ContentLength, FutureIncomingResponse,
    OutgoingRequest, OutgoingRequestBody, OutgoingRequestTrailers, Request, RequestOptions,
    Response, WasiHttpImpl, WasiHttpView,
};
use crate::engine::ResourceView as _;
use crate::engine::WithChildren;

impl<T> outgoing_handler::Host for WasiHttpImpl<&mut T>
where
    T: WasiHttpView,
{
    fn handle(
        &mut self,
        request: Resource<OutgoingRequest>,
        options: Option<Resource<WithChildren<RequestOptions>>>,
    ) -> wasmtime::Result<Result<Resource<FutureIncomingResponse>, ErrorCode>> {
        todo!()
    }
}
