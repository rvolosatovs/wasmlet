use core::mem;
use core::ops::{Deref, DerefMut};
use core::str;

use std::sync::{Arc, TryLockError};

use anyhow::{bail, Context as _};
use http::header::CONTENT_LENGTH;
use tokio::sync::{oneshot, Mutex};
use tracing::debug;
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::bindings::wasi::clocks::monotonic_clock::Duration;
use crate::engine::bindings::wasi::http::types::{
    ErrorCode, FieldName, FieldValue, HeaderError, Host, HostFields, HostFutureIncomingResponse,
    HostFutureTrailers, HostIncomingBody, HostIncomingRequest, HostIncomingResponse,
    HostOutgoingBody, HostOutgoingRequest, HostOutgoingResponse, HostRequestOptions,
    HostResponseOutparam, Method, Scheme, StatusCode, Trailers,
};
use crate::engine::wasi::http::host::{
    delete_fields, delete_request, delete_response, get_fields, get_fields_inner,
    get_fields_inner_mut, get_request, get_request_mut, get_response, get_response_mut,
    push_fields, push_fields_child, push_response,
};
use crate::engine::wasi::http::is_forbidden_header;
use crate::engine::wasi::http::{
    FutureIncomingResponse, FutureTrailers, IncomingBody, IncomingRequest, IncomingResponse,
    OutgoingBody, OutgoingBodyContentSender, OutgoingBodySender, OutgoingRequest, OutgoingResponse,
    Request, RequestOptions, ResponseOutparam,
};
use crate::engine::wasi::io::{HttpInputStream, InputStream, OutputStream, Pollable};
use crate::engine::{wasi, ResourceView as _, WithChildren};
use crate::Ctx;

use super::{delete_body, get_body, push_body};

fn get_request_options<'a>(
    table: &'a ResourceTable,
    opts: &Resource<WithChildren<RequestOptions>>,
) -> wasmtime::Result<&'a WithChildren<RequestOptions>> {
    table
        .get(opts)
        .context("failed to get request options from table")
}

fn get_request_options_inner<'a>(
    table: &'a ResourceTable,
    opts: &Resource<WithChildren<RequestOptions>>,
) -> wasmtime::Result<impl Deref<Target = RequestOptions> + use<'a>> {
    let opts = get_request_options(table, opts)?;
    opts.get()
}

fn get_request_options_mut<'a>(
    table: &'a mut ResourceTable,
    opts: &Resource<WithChildren<RequestOptions>>,
) -> wasmtime::Result<&'a mut WithChildren<RequestOptions>> {
    table
        .get_mut(opts)
        .context("failed to get request options from table")
}

fn get_request_options_inner_mut<'a>(
    table: &'a mut ResourceTable,
    opts: &Resource<WithChildren<RequestOptions>>,
) -> wasmtime::Result<Option<impl DerefMut<Target = RequestOptions> + use<'a>>> {
    let opts = get_request_options_mut(table, opts)?;
    opts.get_mut()
}

fn push_request_options(
    table: &mut ResourceTable,
    fields: WithChildren<RequestOptions>,
) -> wasmtime::Result<Resource<WithChildren<RequestOptions>>> {
    table
        .push(fields)
        .context("failed to push request options to table")
}

fn delete_request_options(
    table: &mut ResourceTable,
    opts: Resource<WithChildren<RequestOptions>>,
) -> wasmtime::Result<WithChildren<RequestOptions>> {
    table
        .delete(opts)
        .context("failed to delete request options from table")
}

fn clone_trailer_result(
    res: &Result<Option<Resource<Trailers>>, ErrorCode>,
) -> Result<Option<Resource<Trailers>>, ErrorCode> {
    match res {
        Ok(None) => Ok(None),
        Ok(Some(trailers)) => Ok(Some(Resource::new_own(trailers.rep()))),
        Err(err) => Err(err.clone()),
    }
}

/// Extract the `Content-Length` header value from a [`http::HeaderMap`], returning `None` if it's not
/// present. This function will return `Err` if it's not possible to parse the `Content-Length`
/// header.
fn get_content_length(headers: &http::HeaderMap) -> wasmtime::Result<Option<u64>> {
    let Some(v) = headers.get(CONTENT_LENGTH) else {
        return Ok(None);
    };
    let v = v.to_str()?;
    let v = v.parse()?;
    Ok(Some(v))
}

fn parse_header_value(
    name: &http::HeaderName,
    value: impl AsRef<[u8]>,
) -> Result<http::HeaderValue, HeaderError> {
    if name == CONTENT_LENGTH {
        let s = str::from_utf8(value.as_ref()).or(Err(HeaderError::InvalidSyntax))?;
        let v: u64 = s.parse().or(Err(HeaderError::InvalidSyntax))?;
        Ok(v.into())
    } else {
        http::header::HeaderValue::from_bytes(value.as_ref()).or(Err(HeaderError::InvalidSyntax))
    }
}

impl Host for Ctx {
    fn http_error_code(
        &mut self,
        err: Resource<wasi::io::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        todo!()
    }
}

impl HostFutureIncomingResponse for Ctx {
    fn subscribe(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn get(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> wasmtime::Result<Option<Result<Result<Resource<IncomingResponse>, ErrorCode>, ()>>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<FutureIncomingResponse>) -> wasmtime::Result<()> {
        todo!()
    }
}

impl HostIncomingBody for Ctx {
    fn stream(
        &mut self,
        body: Resource<IncomingBody>,
    ) -> wasmtime::Result<Result<Resource<InputStream>, ()>> {
        let stream = self
            .table
            .get_mut(&body)
            .context("failed to get body resource")?;
        let prev = mem::replace(stream, IncomingBody::Streaming);
        let IncomingBody::Body(stream) = prev else {
            *stream = prev;
            return Ok(Err(()));
        };
        let stream = InputStream::Http(Arc::new(Mutex::new(HttpInputStream::new(stream))));
        let stream = self
            .table
            .push_child(stream, &body)
            .context("failed to push stream resource")?;
        Ok(Ok(stream))
    }

    fn finish(
        &mut self,
        body: Resource<IncomingBody>,
    ) -> wasmtime::Result<Resource<FutureTrailers>> {
        let body = delete_body(&mut self.table, body)?;
        match body {
            IncomingBody::Body(body) => {
                todo!("consume")
            }
            IncomingBody::Streaming => bail!("stream child is still alive"),
            IncomingBody::Trailers(trailers) => {
                todo!("report trailers")
            }
        }
    }

    fn drop(&mut self, body: Resource<IncomingBody>) -> wasmtime::Result<()> {
        delete_body(&mut self.table, body)?;
        Ok(())
    }
}

impl HostOutgoingBody for Ctx {
    fn write(
        &mut self,
        body: Resource<Arc<std::sync::Mutex<OutgoingBody>>>,
    ) -> wasmtime::Result<Result<Resource<OutputStream>, ()>> {
        let body = get_body(&mut self.table, &body)?;
        let mut body = match body.try_lock() {
            Ok(body) => body,
            Err(TryLockError::WouldBlock) => bail!("outgoing body lock contended"),
            Err(TryLockError::Poisoned(..)) => bail!("outgoing body lock poisoned"),
        };
        let prev = mem::replace(&mut *body, OutgoingBody::Corrupted);
        let (limit, body_tx) = match prev {
            OutgoingBody::Created { limit, body } => (limit, body),
            OutgoingBody::Pending(..) | OutgoingBody::Trailers { .. } => {
                *body = prev;
                return Ok(Err(()));
            }
            OutgoingBody::Dropped => bail!("outgoing body dropped"),
            OutgoingBody::Finished => bail!("outgoing body finished"),
            OutgoingBody::Corrupted => bail!("outgoing body corrupted"),
        };
        let mut stream = if let Some(OutgoingBodySender {
            conn,
            permits,
            data,
            trailers,
        }) = body_tx
        {
            *body = OutgoingBody::Trailers {
                conn: Arc::clone(&conn),
                tx: trailers,
            };
            OutputStream::HttpWriting(OutgoingBodyContentSender {
                conn,
                permits,
                data,
            })
        } else {
            let (tx, rx) = oneshot::channel();
            *body = OutgoingBody::Pending(tx);
            OutputStream::HttpPending(rx)
        };
        drop(body);

        if let Some(limit) = limit {
            stream = OutputStream::Limited {
                budget: limit,
                stream: Box::new(stream),
            };
        }
        let stream = self
            .table
            .push(stream)
            .context("failed to push output stream resource")?;
        Ok(Ok(stream))
    }

    fn finish(
        &mut self,
        body: Resource<Arc<std::sync::Mutex<OutgoingBody>>>,
        trailers: Option<Resource<Trailers>>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let trailers = trailers
            .map(|trailers| self.table.delete(trailers))
            .transpose()?;
        let body = delete_body(&mut self.table, body)?;
        let mut body = match body.try_lock() {
            Ok(body) => body,
            Err(TryLockError::WouldBlock) => bail!("outgoing body lock contended"),
            Err(TryLockError::Poisoned(..)) => bail!("outgoing body lock poisoned"),
        };
        match mem::replace(&mut *body, OutgoingBody::Finished) {
            OutgoingBody::Created { body: None, .. } | OutgoingBody::Pending(..) => Ok(Err(
                ErrorCode::InternalError(Some("cannot finish an unsent body".into())),
            )),
            OutgoingBody::Created {
                body:
                    Some(OutgoingBodySender {
                        conn, trailers: tx, ..
                    }),
                ..
            }
            | OutgoingBody::Trailers { conn, tx } => {
                let trailers = trailers
                    .map(WithChildren::unwrap_or_clone)
                    .transpose()
                    .context("failed to get trailers")?;
                if let Err(..) = tx.send(trailers) {
                    return Ok(Err(ErrorCode::InternalError(Some(
                        "trailer receiver dropped".into(),
                    ))));
                }
                let mut conn = match conn.try_lock() {
                    Ok(conn) => conn,
                    Err(..) => bail!("connection lock contended"),
                };
                if let Ok(err) = conn.try_recv() {
                    Ok(Err(err))
                } else {
                    Ok(Ok(()))
                }
            }
            OutgoingBody::Dropped => bail!("outgoing body dropped"),
            OutgoingBody::Finished => bail!("outgoing body finished"),
            OutgoingBody::Corrupted => bail!("outgoing body corrupted"),
        }
    }

    fn drop(
        &mut self,
        body: Resource<Arc<std::sync::Mutex<OutgoingBody>>>,
    ) -> wasmtime::Result<()> {
        let body = delete_body(&mut self.table, body)?;
        let mut body = match body.try_lock() {
            Ok(body) => body,
            Err(TryLockError::WouldBlock) => bail!("outgoing body lock contended"),
            Err(TryLockError::Poisoned(..)) => bail!("outgoing body lock poisoned"),
        };
        match mem::replace(&mut *body, OutgoingBody::Dropped) {
            OutgoingBody::Dropped => bail!("outgoing body dropped"),
            OutgoingBody::Finished => bail!("outgoing body finished"),
            OutgoingBody::Corrupted => bail!("outgoing body corrupted"),
            _ => Ok(()),
        }
    }
}

impl HostFutureTrailers for Ctx {
    fn subscribe(
        &mut self,
        trailers: Resource<FutureTrailers>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn get(
        &mut self,
        trailers: Resource<FutureTrailers>,
    ) -> wasmtime::Result<Option<Result<Result<Option<Resource<Trailers>>, ErrorCode>, ()>>> {
        todo!()
    }

    fn drop(&mut self, trailers: Resource<FutureTrailers>) -> wasmtime::Result<()> {
        self.table
            .delete(trailers)
            .context("failed to delete trailer resource")?;
        Ok(())
    }
}

impl HostResponseOutparam for Ctx {
    fn send_informational(
        &mut self,
        self_: Resource<ResponseOutparam>,
        status: u16,
        headers: Resource<WithChildren<http::HeaderMap>>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        Ok(Err(ErrorCode::InternalError(Some(
            "informational responses not supported".into(),
        ))))
    }

    fn set(
        &mut self,
        param: Resource<ResponseOutparam>,
        response: Result<Resource<OutgoingResponse>, ErrorCode>,
    ) -> wasmtime::Result<()> {
        let ResponseOutparam {
            response: response_tx,
            body: body_tx,
        } = self
            .table
            .delete(param)
            .context("failed to delete response outparam resource")?;
        let response = match response {
            Ok(response) => {
                let OutgoingResponse {
                    status,
                    headers,
                    body,
                } = self
                    .table
                    .delete(response)
                    .context("failed to delete outgoing response resource")?;
                let mut response = http::Response::builder().status(status);
                *response.headers_mut().unwrap() =
                    headers.unwrap_or_clone().context("failed to get headers")?;
                let response = response.body(()).context("failed to build response")?;
                if let Some(body) = body {
                    let mut body = match body.try_lock() {
                        Ok(body) => body,
                        Err(TryLockError::WouldBlock) => bail!("outgoing body lock contended"),
                        Err(TryLockError::Poisoned(..)) => bail!("outgoing body lock poisoned"),
                    };
                    match mem::replace(&mut *body, OutgoingBody::Corrupted) {
                        OutgoingBody::Created { body: Some(..), .. }
                        | OutgoingBody::Trailers { .. } => bail!("outgoing body already set"),
                        OutgoingBody::Created { limit, body: None } => {
                            *body = OutgoingBody::Created {
                                limit,
                                body: Some(body_tx),
                            }
                        }
                        OutgoingBody::Pending(tx) => {
                            let OutgoingBodySender {
                                conn,
                                permits,
                                data,
                                trailers,
                            } = body_tx;
                            _ = tx.send(OutgoingBodyContentSender {
                                conn: Arc::clone(&conn),
                                permits,
                                data,
                            });
                            *body = OutgoingBody::Trailers { conn, tx: trailers }
                        }
                        OutgoingBody::Dropped => {}
                        OutgoingBody::Finished => {}
                        OutgoingBody::Corrupted => bail!("outgoing body corrupted"),
                    };
                }
                Ok(response)
            }
            Err(err) => Err(err),
        };
        if let Err(..) = response_tx.send(response) {
            debug!("response receiver closed, dropping response");
        }
        Ok(())
    }

    fn drop(&mut self, param: Resource<ResponseOutparam>) -> wasmtime::Result<()> {
        self.table
            .delete(param)
            .context("failed to delete response outparam resource")?;
        Ok(())
    }
}

impl HostFields for Ctx {
    fn new(&mut self) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        push_fields(&mut self.table, WithChildren::default())
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldName, FieldValue)>,
    ) -> wasmtime::Result<Result<Resource<WithChildren<http::HeaderMap>>, HeaderError>> {
        let mut fields = http::HeaderMap::new();

        for (name, value) in entries {
            let Ok(name) = name.parse() else {
                return Ok(Err(HeaderError::InvalidSyntax));
            };
            if is_forbidden_header(&name) {
                return Ok(Err(HeaderError::Forbidden));
            }
            match parse_header_value(&name, value) {
                Ok(value) => {
                    fields.append(name, value);
                }
                Err(err) => return Ok(Err(err)),
            }
        }
        let fields = push_fields(&mut self.table, WithChildren::new(fields))?;
        Ok(Ok(fields))
    }

    fn get(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
        name: FieldName,
    ) -> wasmtime::Result<Vec<FieldValue>> {
        let fields = get_fields_inner(&mut self.table, &fields)?;
        Ok(fields
            .get_all(name)
            .into_iter()
            .map(|val| val.as_bytes().into())
            .collect())
    }

    fn has(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
        name: FieldName,
    ) -> wasmtime::Result<bool> {
        let fields = get_fields_inner(&mut self.table, &fields)?;
        Ok(fields.contains_key(name))
    }

    fn set(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
        name: FieldName,
        value: Vec<FieldValue>,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        let Ok(name) = name.parse() else {
            return Ok(Err(HeaderError::InvalidSyntax));
        };
        if is_forbidden_header(&name) {
            return Ok(Err(HeaderError::Forbidden));
        }
        let mut values = Vec::with_capacity(value.len());
        for value in value {
            match parse_header_value(&name, value) {
                Ok(value) => {
                    values.push(value);
                }
                Err(err) => return Ok(Err(err)),
            }
        }
        let Some(mut fields) = get_fields_inner_mut(&mut self.table, &fields)? else {
            return Ok(Err(HeaderError::Immutable));
        };
        fields.remove(&name);
        for value in values {
            fields.append(&name, value);
        }
        Ok(Ok(()))
    }

    fn delete(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
        name: FieldName,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        let header = match http::header::HeaderName::from_bytes(name.as_bytes()) {
            Ok(header) => header,
            Err(_) => return Ok(Err(HeaderError::InvalidSyntax)),
        };
        if is_forbidden_header(&header) {
            return Ok(Err(HeaderError::Forbidden));
        }
        let Some(mut fields) = get_fields_inner_mut(&mut self.table, &fields)? else {
            return Ok(Err(HeaderError::Immutable));
        };
        fields.remove(&name);
        Ok(Ok(()))
    }

    fn append(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
        name: FieldName,
        value: FieldValue,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        let Ok(name) = name.parse() else {
            return Ok(Err(HeaderError::InvalidSyntax));
        };
        if is_forbidden_header(&name) {
            return Ok(Err(HeaderError::Forbidden));
        }
        let value = match parse_header_value(&name, value) {
            Ok(value) => value,
            Err(err) => return Ok(Err(err)),
        };
        let Some(mut fields) = get_fields_inner_mut(&mut self.table, &fields)? else {
            return Ok(Err(HeaderError::Immutable));
        };
        fields.append(name, value);
        Ok(Ok(()))
    }

    fn entries(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
    ) -> wasmtime::Result<Vec<(FieldName, FieldValue)>> {
        let fields = get_fields_inner(&self.table, &fields)?;
        let fields = fields
            .iter()
            .map(|(name, value)| (name.as_str().into(), value.as_bytes().into()))
            .collect();
        Ok(fields)
    }

    fn clone(
        &mut self,
        fields: Resource<WithChildren<http::HeaderMap>>,
    ) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        let fields = get_fields(&self.table, &fields)?;
        let fields = fields.clone()?;
        push_fields(&mut self.table, fields)
    }

    fn drop(&mut self, fields: Resource<WithChildren<http::HeaderMap>>) -> wasmtime::Result<()> {
        delete_fields(&mut self.table, fields)?;
        Ok(())
    }
}

impl HostOutgoingRequest for Ctx {
    fn new(
        &mut self,
        headers: Resource<WithChildren<http::HeaderMap>>,
    ) -> wasmtime::Result<Resource<OutgoingRequest>> {
        todo!()
    }

    fn body(
        &mut self,
        self_: Resource<OutgoingRequest>,
    ) -> wasmtime::Result<Result<Resource<Arc<std::sync::Mutex<OutgoingBody>>>, ()>> {
        todo!()
    }

    //fn new(
    //    &mut self,
    //    headers: Resource<WithChildren<http::HeaderMap>>,
    //    contents: Option<HostStream<u8>>,
    //    trailers: TrailerFuture,
    //    options: Option<Resource<WithChildren<RequestOptions>>>,
    //) -> wasmtime::Result<(Resource<OutgoingRequest>, HostFuture<Result<(), ErrorCode>>)> {
    //    store.with(|mut view| {
    //        let instance = view.instance();
    //        let (res_tx, res_rx) = instance
    //            .future(&mut view)
    //            .context("failed to create future")?;
    //        let contents = contents.map(|contents| {
    //            contents
    //                .into_reader(&mut view)
    //                .read(BytesMut::with_capacity(DEFAULT_BUFFER_CAPACITY))
    //        });
    //        let trailers = trailers.into_reader(&mut view).read();
    //        let table = view.table;
    //        let headers = delete_fields(table, headers)?;
    //        let headers = headers.unwrap_or_clone()?;
    //        let content_length = get_content_length(&headers)?;
    //        let options = options
    //            .map(|options| {
    //                let options = delete_request_options(table, options)?;
    //                options.unwrap_or_clone()
    //            })
    //            .transpose()?;
    //        let body = Body::Guest {
    //            contents: contents.map(|v| Box::pin(v) as _),
    //            trailers: Some(Box::pin(trailers)),
    //            buffer: None,
    //            tx: res_tx,
    //            content_length: content_length.map(ContentLength::new),
    //        };
    //        let req = push_request(
    //            table,
    //            Request::new(http::Method::GET, None, None, None, headers, body, options),
    //        )?;
    //        Ok((req, res_rx.into()))
    //    })
    //}
    fn method(&mut self, req: Resource<OutgoingRequest>) -> wasmtime::Result<Method> {
        let Request { method, .. } = get_request(&mut self.table, &req)?;
        Ok(method.into())
    }

    fn set_method(
        &mut self,
        req: Resource<OutgoingRequest>,
        method: Method,
    ) -> wasmtime::Result<Result<(), ()>> {
        let req = get_request_mut(&mut self.table, &req)?;
        let Ok(method) = method.try_into() else {
            return Ok(Err(()));
        };
        req.method = method;
        Ok(Ok(()))
    }

    fn path_with_query(
        &mut self,
        req: Resource<OutgoingRequest>,
    ) -> wasmtime::Result<Option<String>> {
        let Request {
            path_with_query, ..
        } = get_request(&mut self.table, &req)?;
        Ok(path_with_query.as_ref().map(|pq| pq.as_str().into()))
    }

    fn set_path_with_query(
        &mut self,
        req: Resource<OutgoingRequest>,
        path_with_query: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let req = get_request_mut(&mut self.table, &req)?;
        let Some(path_with_query) = path_with_query else {
            req.path_with_query = None;
            return Ok(Ok(()));
        };
        let Ok(path_with_query) = path_with_query.try_into() else {
            return Ok(Err(()));
        };
        req.path_with_query = Some(path_with_query);
        Ok(Ok(()))
    }

    fn scheme(&mut self, req: Resource<OutgoingRequest>) -> wasmtime::Result<Option<Scheme>> {
        let Request { scheme, .. } = get_request(&mut self.table, &req)?;
        Ok(scheme.as_ref().map(Into::into))
    }

    fn set_scheme(
        &mut self,
        req: Resource<OutgoingRequest>,
        scheme: Option<Scheme>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let req = get_request_mut(&mut self.table, &req)?;
        let Some(scheme) = scheme else {
            req.scheme = None;
            return Ok(Ok(()));
        };
        let Ok(scheme) = scheme.try_into() else {
            return Ok(Err(()));
        };
        req.scheme = Some(scheme);
        Ok(Ok(()))
    }

    fn authority(&mut self, req: Resource<OutgoingRequest>) -> wasmtime::Result<Option<String>> {
        let Request { authority, .. } = get_request(&mut self.table, &req)?;
        Ok(authority.as_ref().map(|auth| auth.as_str().into()))
    }

    fn set_authority(
        &mut self,
        req: Resource<OutgoingRequest>,
        authority: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let req = get_request_mut(&mut self.table, &req)?;
        let Some(authority) = authority else {
            req.authority = None;
            return Ok(Ok(()));
        };
        let has_port = authority.contains(':');
        let Ok(authority) = http::uri::Authority::try_from(authority) else {
            return Ok(Err(()));
        };
        if has_port && authority.port_u16().is_none() {
            return Ok(Err(()));
        }
        req.authority = Some(authority);
        Ok(Ok(()))
    }

    fn headers(
        &mut self,
        req: Resource<OutgoingRequest>,
    ) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        let Request { headers, .. } = get_request(&self.table, &req)?;
        let headers = headers.child();
        push_fields_child(&mut self.table, headers, &req)
    }

    fn drop(&mut self, req: Resource<OutgoingRequest>) -> wasmtime::Result<()> {
        delete_request(&mut self.table, req)?;
        Ok(())
    }
}

impl HostIncomingRequest for Ctx {
    fn consume(
        &mut self,
        req: Resource<IncomingRequest>,
    ) -> wasmtime::Result<Result<Resource<IncomingBody>, ()>> {
        let IncomingRequest { body, .. } = get_request_mut(&mut self.table, &req)?;
        let Some(body) = body.take() else {
            return Ok(Err(()));
        };
        let body = push_body(&mut self.table, body)?;
        Ok(Ok(body))
    }

    fn method(&mut self, req: Resource<IncomingRequest>) -> wasmtime::Result<Method> {
        let IncomingRequest { method, .. } = get_request(&mut self.table, &req)?;
        Ok(method.into())
    }

    fn path_with_query(
        &mut self,
        req: Resource<IncomingRequest>,
    ) -> wasmtime::Result<Option<String>> {
        let IncomingRequest {
            path_with_query, ..
        } = get_request(&mut self.table, &req)?;
        Ok(path_with_query.as_ref().map(|pq| pq.as_str().into()))
    }

    fn scheme(&mut self, req: Resource<IncomingRequest>) -> wasmtime::Result<Option<Scheme>> {
        let IncomingRequest { scheme, .. } = get_request(&mut self.table, &req)?;
        Ok(scheme.as_ref().map(Into::into))
    }

    fn authority(&mut self, req: Resource<IncomingRequest>) -> wasmtime::Result<Option<String>> {
        let IncomingRequest { authority, .. } = get_request(&mut self.table, &req)?;
        Ok(authority.as_ref().map(|auth| auth.as_str().into()))
    }

    fn headers(
        &mut self,
        req: Resource<IncomingRequest>,
    ) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        let IncomingRequest { headers, .. } = get_request(&self.table, &req)?;
        let headers = headers.child();
        push_fields_child(&mut self.table, headers, &req)
    }

    fn drop(&mut self, req: Resource<IncomingRequest>) -> wasmtime::Result<()> {
        delete_request(&mut self.table, req)?;
        Ok(())
    }
}

impl HostRequestOptions for Ctx {
    fn new(&mut self) -> wasmtime::Result<Resource<WithChildren<RequestOptions>>> {
        push_request_options(&mut self.table, WithChildren::default())
    }

    fn connect_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
    ) -> wasmtime::Result<Option<Duration>> {
        let RequestOptions {
            connect_timeout: Some(connect_timeout),
            ..
        } = *get_request_options_inner(&mut self.table, &opts)?
        else {
            return Ok(None);
        };
        let ns = connect_timeout.as_nanos();
        let ns = ns
            .try_into()
            .context("connect timeout duration nanoseconds do not fit in u64")?;
        Ok(Some(ns))
    }

    fn set_connect_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
        duration: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let Some(mut opts) = get_request_options_inner_mut(&mut self.table, &opts)? else {
            return Ok(Err(()));
        };
        opts.connect_timeout = duration.map(core::time::Duration::from_nanos);
        Ok(Ok(()))
    }

    fn first_byte_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
    ) -> wasmtime::Result<Option<Duration>> {
        let RequestOptions {
            first_byte_timeout: Some(first_byte_timeout),
            ..
        } = *get_request_options_inner(&mut self.table, &opts)?
        else {
            return Ok(None);
        };
        let ns = first_byte_timeout.as_nanos();
        let ns = ns
            .try_into()
            .context("first byte timeout duration nanoseconds do not fit in u64")?;
        Ok(Some(ns))
    }

    fn set_first_byte_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
        duration: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let Some(mut opts) = get_request_options_inner_mut(&mut self.table, &opts)? else {
            return Ok(Err(()));
        };
        opts.first_byte_timeout = duration.map(core::time::Duration::from_nanos);
        Ok(Ok(()))
    }

    fn between_bytes_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
    ) -> wasmtime::Result<Option<Duration>> {
        let RequestOptions {
            between_bytes_timeout: Some(between_bytes_timeout),
            ..
        } = *get_request_options_inner(&mut self.table, &opts)?
        else {
            return Ok(None);
        };
        let ns = between_bytes_timeout.as_nanos();
        let ns = ns
            .try_into()
            .context("between bytes timeout duration nanoseconds do not fit in u64")?;
        Ok(Some(ns))
    }

    fn set_between_bytes_timeout(
        &mut self,
        opts: Resource<WithChildren<RequestOptions>>,
        duration: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        let Some(mut opts) = get_request_options_inner_mut(&mut self.table, &opts)? else {
            return Ok(Err(()));
        };
        opts.between_bytes_timeout = duration.map(core::time::Duration::from_nanos);
        Ok(Ok(()))
    }

    fn drop(&mut self, opts: Resource<WithChildren<RequestOptions>>) -> wasmtime::Result<()> {
        delete_request_options(&mut self.table, opts)?;
        Ok(())
    }
}

impl HostIncomingResponse for Ctx {
    fn status(&mut self, res: Resource<IncomingResponse>) -> wasmtime::Result<StatusCode> {
        let res = get_response(&mut self.table, &res)?;
        Ok(res.status.into())
    }

    fn consume(
        &mut self,
        self_: Resource<IncomingResponse>,
    ) -> wasmtime::Result<Result<Resource<IncomingBody>, ()>> {
        todo!()
    }

    fn headers(
        &mut self,
        res: Resource<IncomingResponse>,
    ) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        // TODO
        todo!()
        //let IncomingResponse { headers, .. } = get_response(table, &res)?;
        //push_fields_child(table, headers.child(), &res)
    }

    fn drop(&mut self, res: Resource<IncomingResponse>) -> wasmtime::Result<()> {
        delete_response(&mut self.table, res)?;
        Ok(())
    }
}

impl HostOutgoingResponse for Ctx {
    fn new(
        &mut self,
        headers: Resource<WithChildren<http::HeaderMap>>,
    ) -> wasmtime::Result<Resource<OutgoingResponse>> {
        let headers = delete_fields(&mut self.table, headers)?;
        let res = OutgoingResponse {
            status: http::StatusCode::OK,
            headers,
            body: None,
        };
        push_response(&mut self.table, res)
    }

    fn body(
        &mut self,
        res: Resource<OutgoingResponse>,
    ) -> wasmtime::Result<Result<Resource<Arc<std::sync::Mutex<OutgoingBody>>>, ()>> {
        let res @ OutgoingResponse { body: None, .. } = get_response_mut(&mut self.table, &res)?
        else {
            return Ok(Err(()));
        };
        let body = Arc::new(std::sync::Mutex::new(OutgoingBody::Created {
            limit: None, // TODO: set content-length
            body: None,
        }));
        res.body = Some(Arc::clone(&body));
        let body = push_body(&mut self.table, body)?;
        Ok(Ok(body))
    }

    fn status_code(&mut self, res: Resource<OutgoingResponse>) -> wasmtime::Result<StatusCode> {
        let res = get_response(&mut self.table, &res)?;
        Ok(res.status.into())
    }

    fn set_status_code(
        &mut self,
        res: Resource<OutgoingResponse>,
        status_code: StatusCode,
    ) -> wasmtime::Result<Result<(), ()>> {
        let res = get_response_mut(&mut self.table, &res)?;
        let Ok(status) = http::StatusCode::from_u16(status_code) else {
            return Ok(Err(()));
        };
        res.status = status;
        Ok(Ok(()))
    }

    fn headers(
        &mut self,
        res: Resource<OutgoingResponse>,
    ) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
        todo!()
        //let OutgoingResponse { headers, .. } = get_response(table, &res)?;
        //push_fields_child(table, headers.child(), &res)
    }

    fn drop(&mut self, res: Resource<OutgoingResponse>) -> wasmtime::Result<()> {
        delete_response(&mut self.table, res)?;
        Ok(())
    }
}
