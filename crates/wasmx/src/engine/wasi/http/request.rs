use core::time::Duration;

use http::uri::{Authority, PathAndQuery, Scheme};
use http::{HeaderMap, Method};

use crate::engine::wasi::http::{IncomingBody, OutgoingBody};
use crate::engine::WithChildren;

pub type IncomingRequest = Request<IncomingBody>;
pub type OutgoingRequest = Request<OutgoingBody>;

/// The concrete type behind a `wasi:http/types/request-options` resource.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    /// How long to wait for a connection to be established.
    pub connect_timeout: Option<Duration>,
    /// How long to wait for the first byte of the response body.
    pub first_byte_timeout: Option<Duration>,
    /// How long to wait between frames of the response body.
    pub between_bytes_timeout: Option<Duration>,
}

/// The concrete type behind a `wasi:http/types/request` resource.
#[derive(Debug)]
pub struct Request<T> {
    /// The method of the request.
    pub method: Method,
    /// The scheme of the request.
    pub scheme: Option<Scheme>,
    /// The authority of the request.
    pub authority: Option<Authority>,
    /// The path and query of the request.
    pub path_with_query: Option<PathAndQuery>,
    /// The request headers.
    pub headers: WithChildren<HeaderMap>,
    /// The request body.
    pub(crate) body: Option<T>,
}

impl Request<IncomingBody> {
    /// Construct a new [Request]
    pub fn new(
        method: Method,
        scheme: Option<Scheme>,
        authority: Option<Authority>,
        path_with_query: Option<PathAndQuery>,
        headers: HeaderMap,
        body: impl Into<IncomingBody>,
    ) -> Self {
        Self {
            method,
            scheme,
            authority,
            path_with_query,
            headers: WithChildren::new(headers),
            body: Some(body.into()),
        }
    }
}
