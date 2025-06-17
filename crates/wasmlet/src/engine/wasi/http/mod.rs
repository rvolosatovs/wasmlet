//! # Wasmlet's WASI HTTP Implementation

#![allow(unused)] // TODO: remove

mod body;
mod client;
mod conv;
mod host;
mod proxy;
mod request;
mod response;

pub use body::*;
pub use client::*;
pub use request::*;
pub use response::*;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use tokio::sync::oneshot;
use wasmtime::component::Linker;

pub use crate::engine::bindings::wasi::http::types::ErrorCode;

use crate::Ctx;

#[derive(Debug)]
pub struct ResponseOutparam {
    pub response: oneshot::Sender<Result<http::Response<()>, ErrorCode>>,
    pub body: OutgoingBodySender,
}

pub struct FutureIncomingResponse;

pub enum FutureTrailers {
    Body(BoxBody<Bytes, ErrorCode>),
    Ready(http::HeaderMap),
}

/// Default byte buffer capacity to use
const DEFAULT_BUFFER_CAPACITY: usize = 1 << 13;

/// Set of [http::header::HeaderName], that are forbidden by default
/// for requests and responses originating in the guest.
pub const DEFAULT_FORBIDDEN_HEADERS: [http::header::HeaderName; 10] = [
    http::header::CONNECTION,
    http::header::HeaderName::from_static("keep-alive"),
    http::header::PROXY_AUTHENTICATE,
    http::header::PROXY_AUTHORIZATION,
    http::header::HeaderName::from_static("proxy-connection"),
    http::header::TE,
    http::header::TRANSFER_ENCODING,
    http::header::UPGRADE,
    http::header::HOST,
    http::header::HeaderName::from_static("http2-settings"),
];

fn is_forbidden_header(name: &http::header::HeaderName) -> bool {
    DEFAULT_FORBIDDEN_HEADERS.contains(name)
}

/// Capture the state necessary for use in the wasi-http API implementation.
#[derive(Clone, Debug, Default)]
pub struct WasiHttpCtx<C = DefaultClient>
where
    C: Client,
{
    /// HTTP client
    pub client: C,
}

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: Send>(
    l: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> anyhow::Result<()> {
    // TODO: Allow config

    use crate::engine::bindings::wasi::http;
    http::outgoing_handler::add_to_linker(l, get)?;
    http::types::add_to_linker(l, &http::types::LinkOptions::default(), get)?;
    Ok(())
}
