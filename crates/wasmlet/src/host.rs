use core::future::Future;
use core::marker::PhantomData;
use core::net::SocketAddr;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{ready, Context, Poll};

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use bytes::{Buf, Bytes};
use futures::stream::FuturesUnordered;
use futures::TryStreamExt as _;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt as _;
use hyper_util::rt::TokioIo;
use pin_project_lite::pin_project;
use tokio::fs;
use tokio::net::TcpSocket;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::{oneshot, Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio::task::JoinSet;
use tokio::try_join;
use tracing::{debug, error, info, instrument, warn, Instrument as _, Span};
use url::Url;

use crate::config::{Service, Workload};
use crate::engine::wasi;
use crate::{Cmd, Manifest, WorkloadInvocation, WorkloadInvocationPayload, EPOCH_MONOTONIC_NOW};

fn bind_tcp(address: SocketAddr) -> anyhow::Result<TcpSocket> {
    debug!("binding TCP socket");
    let sock = match address {
        SocketAddr::V4(..) => TcpSocket::new_v4(),
        SocketAddr::V6(..) => TcpSocket::new_v6(),
    }
    .context("failed to create HTTP proxy TCP socket")?;
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
    Ok(sock)
}

fn build_http_response<T, E>(
    code: http::StatusCode,
    body: impl Into<T>,
) -> anyhow::Result<http::Response<BoxBody<T, E>>>
where
    T: Buf + Sync + Send + 'static,
{
    http::Response::builder()
        .status(code)
        .body(
            http_body_util::Full::new(body.into())
                .map_err(|_| unreachable!())
                .boxed(),
        )
        .context("failed to build response")
}

async fn fetch_src(src: &str) -> anyhow::Result<Bytes> {
    enum Src {
        Url(Url),
        Binary(Vec<u8>),
    }
    let src = if src.starts_with('.') || src.starts_with('/') {
        fs::read(src)
            .await
            .with_context(|| format!("failed to read bytes from `{src}`"))
            .map(Src::Binary)
    } else {
        Url::parse(src)
            .with_context(|| format!("failed to parse bytes URL `{src}`"))
            .map(Src::Url)
    }?;
    match src {
        Src::Url(url) => match url.scheme() {
            "file" => {
                let buf = url
                    .to_file_path()
                    .map_err(|()| anyhow!("failed to convert bytes URL to file path"))?;
                let buf = fs::read(buf)
                    .await
                    .context("failed to read bytes from file URL")?;
                Ok(buf.into())
            }
            "http" | "https" => {
                let buf = reqwest::get(url).await.context("failed to GET bytes URL")?;
                buf.bytes().await.context("failed fetch bytes from URL")
            }
            scheme => bail!("URL scheme `{scheme}` not supported"),
        },
        Src::Binary(buf) => Ok(buf.into()),
    }
}

#[instrument(level = "debug")]
pub async fn load_manifest(
    Manifest {
        plugins,
        workloads,
        services,
    }: Manifest<Box<str>>,
) -> anyhow::Result<Manifest<Bytes>> {
    let workloads = workloads
        .into_iter()
        .map(
            |(
                name,
                Workload {
                    component,
                    env,
                    pool,
                    limits,
                },
            )| async move {
                let (src, component) = component.take_src();
                let src = fetch_src(&src).await?;
                let component = component.map_src(|()| src);
                anyhow::Ok((
                    name,
                    Workload {
                        component,
                        env,
                        pool,
                        limits,
                    },
                ))
            },
        )
        .collect::<FuturesUnordered<_>>();
    let services = services
        .into_iter()
        .map(|(name, Service { component, env })| async move {
            let (src, component) = component.take_src();
            let src = fetch_src(&src).await?;
            let component = component.map_src(|()| src);
            anyhow::Ok((name, Service { component, env }))
        })
        .collect::<FuturesUnordered<_>>();
    let (services, workloads) = try_join!(services.try_collect(), workloads.try_collect())?;
    Ok(Manifest {
        plugins,
        services,
        workloads,
    })
}

#[instrument(level = "debug", skip_all)]
pub async fn load_and_apply_manifest(
    cmds: &mpsc::Sender<Cmd>,
    manifest: Manifest<Box<str>>,
) -> anyhow::Result<()> {
    let manifest = load_manifest(manifest).await?;
    apply_manifest(cmds, manifest).await
}

#[instrument(level = "debug", fields(path = ?path.as_ref()))]
pub async fn read_manifest(path: impl AsRef<Path>) -> anyhow::Result<Manifest<Bytes>> {
    let path = path.as_ref();
    debug!(?path, "reading manifest file");
    let manifest = match fs::read_to_string(path).await {
        Ok(conf) => toml::from_str(&conf)
            .with_context(|| format!("failed to parse `{}`", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            debug!(?path, "manifest not found");
            Manifest::default()
        }
        Err(err) => {
            bail!(anyhow!(err).context("failed to read `wasmlet.toml`"))
        }
    };
    load_manifest(manifest).await
}

#[instrument(level = "debug", skip_all)]
pub async fn apply_manifest(
    cmds: &mpsc::Sender<Cmd>,
    manifest: Manifest<Bytes>,
) -> anyhow::Result<()> {
    let (apply_tx, apply_rx) = oneshot::channel();
    // 1s grace period
    let deadline = EPOCH_MONOTONIC_NOW
        .load(Ordering::Relaxed)
        .saturating_add(1000);
    cmds.send(Cmd::ApplyManifest {
        manifest,
        deadline,
        result: apply_tx,
    })
    .await
    .map_err(|_| anyhow!("scheduler thread exited"))?;
    debug!("waiting for manifest application result");
    apply_rx
        .await
        .context("scheduler thread unexpectedly exited")?
        .context("failed to schedule workloads")
}

#[instrument(level = "debug", skip_all, fields(path = ?path.as_ref()))]
pub async fn read_and_apply_manifest(
    cmds: &mpsc::Sender<Cmd>,
    path: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let manifest = read_manifest(path).await?;
    apply_manifest(cmds, manifest).await
}

struct ContextServiceFn<C, F, Fut> {
    cx: Arc<C>,
    f: F,
    _ty: PhantomData<Fut>,
}

impl<C, F: Clone, Fut> Clone for ContextServiceFn<C, F, Fut> {
    fn clone(&self) -> Self {
        Self {
            cx: Arc::clone(&self.cx),
            f: self.f.clone(),
            _ty: PhantomData,
        }
    }
}

impl<C, F, T, U, E, Fut> hyper::service::Service<http::Request<T>> for ContextServiceFn<C, F, Fut>
where
    F: Fn(Arc<C>, http::Request<T>) -> Fut,
    Fut: Future<Output = Result<http::Response<U>, E>>,
{
    type Response = http::Response<U>;
    type Error = E;
    type Future = Fut;

    fn call(&self, req: http::Request<T>) -> Self::Future {
        (self.f)(Arc::clone(&self.cx), req)
    }
}

impl<C, F, Fut> ContextServiceFn<C, F, Fut> {
    fn new(cx: impl Into<Arc<C>>, f: F) -> Self {
        Self {
            cx: cx.into(),
            f,
            _ty: PhantomData,
        }
    }
}

pin_project! {
    struct OutgoingBodyReceiver {
        data: Option<UnboundedReceiver<(Vec<u8>, OwnedSemaphorePermit)>>,
        #[pin]
        trailers: oneshot::Receiver<Option<http::HeaderMap>>,
        done: Option<oneshot::Receiver<anyhow::Result<()>>>,
    }
}

impl http_body::Body for OutgoingBodyReceiver {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(done) = self.as_mut().done.as_mut() {
            match done.try_recv() {
                Ok(Ok(())) => {
                    self.done = None;
                }
                Ok(Err(err)) => {
                    self.done = None;
                    return Poll::Ready(Some(Err(err)));
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.done = None;
                    return Poll::Ready(Some(Err(anyhow!("result sender closed"))));
                }
            }
        }
        if let Some(data) = self.as_mut().data.as_mut() {
            if let Some((buf, _)) = ready!(data.poll_recv(cx)) {
                return Poll::Ready(Some(Ok(http_body::Frame::data(buf.into()))));
            }
            self.data = None;
        }
        match ready!(self.project().trailers.poll(cx)) {
            Ok(Some(trailers)) => Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers)))),
            Ok(None) => Poll::Ready(None),
            Err(..) => Poll::Ready(Some(Err(anyhow!("trailer sender closed")))),
        }
    }
}

pub struct Host {
    cmds: mpsc::Sender<Cmd>,
    max_instances: usize,
    instance_permits: Arc<Semaphore>,
}

impl Host {
    pub fn new(cmds: mpsc::Sender<Cmd>, max_instances: usize) -> Self {
        Self {
            cmds,
            max_instances,
            instance_permits: Arc::new(Semaphore::new(max_instances)),
        }
    }

    #[instrument(skip(self))]
    pub async fn handle_http_proxy(
        &mut self,
        address: SocketAddr,
    ) -> anyhow::Result<impl Future<Output = ()>> {
        let sock = bind_tcp(address)?;
        let sock = sock
            .listen(self.max_instances.try_into().unwrap_or(u32::MAX))
            .context("failed to listen on TCP socket")?;

        let cmds = self.cmds.clone();
        let instance_permits = Arc::clone(&self.instance_permits);
        let svc = move |conn, req: http::Request<hyper::body::Incoming>| {
            let instance_permits = Arc::clone(&instance_permits);
            let cmds = cmds.clone();
            async move {
                let (mut parts, body) = req.into_parts();
                let Some(name) = parts.headers.remove("X-Wasmlet-Id") else {
                    return build_http_response(
                        http::StatusCode::BAD_REQUEST,
                        "`X-Wasmlet-Id` header missing",
                    );
                };
                let name = match name.to_str() {
                    Ok(name) => name,
                    Err(err) => {
                        return build_http_response(
                            http::StatusCode::BAD_REQUEST,
                            format!("`X-Wasmlet-Id` header value is not valid UTF-8: {err}"),
                        );
                    }
                };
                let _permit = match instance_permits.try_acquire() {
                    Ok(permit) => permit,
                    Err(TryAcquireError::NoPermits) => {
                        return build_http_response(
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            "maximum instance count reached",
                        );
                    }
                    Err(TryAcquireError::Closed) => {
                        return build_http_response(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            "semaphore closed",
                        );
                    }
                };
                // TODO: Set scheme

                let (trailers_tx, trailers_rx) = oneshot::channel();
                let (response_tx, response_rx) = oneshot::channel();
                let (result_tx, result_rx) = oneshot::channel();
                let (done_tx, done_rx) = oneshot::channel();
                let permits = Arc::new(Semaphore::new(u16::MAX.into()));
                let (data_tx, data_rx) = mpsc::unbounded_channel();
                match cmds.try_send(Cmd::Invoke {
                    name: name.into(),
                    invocation: WorkloadInvocation {
                        span: Span::current(),
                        payload: WorkloadInvocationPayload::WasiHttpHandler {
                            request: http::Request::from_parts(parts, body).into(),
                            response: wasi::http::ResponseOutparam {
                                response: response_tx,
                                body: wasi::http::OutgoingBodySender {
                                    conn,
                                    permits,
                                    data: data_tx,
                                    trailers: trailers_tx,
                                },
                            },
                            result: done_tx,
                        },
                    },
                    result: result_tx,
                }) {
                    Ok(()) => {
                        result_rx.await.context("workload thread exited")??;
                        match response_rx.await {
                            Err(..) => match done_rx.await {
                                Ok(Ok(())) => bail!("workload did not send a response"),
                                Ok(Err(err)) => Err(err.context("workload trapped")),
                                Err(..) => bail!("workload thread exited"),
                            },
                            Ok(Err(err)) => Err(anyhow!(err).context("`wasi:http` handler failed")),
                            Ok(Ok(response)) => Ok(response.map(|()| {
                                OutgoingBodyReceiver {
                                    data: Some(data_rx),
                                    trailers: trailers_rx,
                                    done: Some(done_rx),
                                }
                                .boxed()
                            })),
                        }
                    }
                    Err(mpsc::error::TrySendError::Full(..)) => build_http_response(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        "engine buffer full",
                    ),
                    Err(mpsc::error::TrySendError::Closed(..)) => build_http_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        "engine thread exited",
                    ),
                }
            }
        };
        let srv = hyper::server::conn::http1::Builder::new();
        Ok(async move {
            let mut tasks = JoinSet::new();
            info!("HTTP proxy endpoint started");
            // TODO: check for shutdown, gracefully shutdown HTTP
            // TODO: join conn tasks
            loop {
                while let Some(res) = tasks.try_join_next() {
                    if let Err(err) = res {
                        error!(?err, "HTTP proxy endpoint connection task panicked");
                    }
                }
                let stream = match sock.accept().await {
                    Ok((stream, addr)) => {
                        info!(?addr, "accepted HTTP proxy connection");
                        stream
                    }
                    Err(err) => {
                        error!(?err, "failed to accept HTTP proxy endpoint connection");
                        continue;
                    }
                };
                let (err_tx, err_rx) = oneshot::channel();
                let conn = srv.serve_connection(
                    TokioIo::new(stream),
                    ContextServiceFn::new(Mutex::new(err_rx), svc.clone()),
                );
                tasks.spawn(
                    async move {
                        if let Err(err) = conn.await {
                            warn!(?err, "failed to serve HTTP proxy endpoint connection");
                            // TODO: This error conversion does not make any sense
                            _ = err_tx.send(wasi::http::ErrorCode::from_hyper_response_error(err));
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span())
    }

    #[instrument(skip(self))]
    pub async fn handle_http_admin(
        &mut self,
        address: SocketAddr,
        ready: Arc<AtomicBool>,
    ) -> anyhow::Result<impl Future<Output = ()>> {
        let sock = bind_tcp(address)?;
        let sock = sock.listen(128).context("failed to listen on TCP socket")?;

        let svc = hyper::service::service_fn({
            move |req: http::Request<hyper::body::Incoming>| {
                let ready = Arc::clone(&ready);
                async move {
                    const OK: &str = r#"{"status":"ok"}"#;
                    const FAIL: &str = r#"{"status":"failure"}"#;
                    let (parts, ..) = req.into_parts();
                    match (parts.method.as_str(), parts.uri.path()) {
                        ("GET", "/livez") => anyhow::Ok(http::Response::new(
                            http_body_util::Full::new(Bytes::from(OK)),
                        )),
                        ("GET", "/readyz") => {
                            if ready.load(Ordering::Relaxed) {
                                Ok(http::Response::new(http_body_util::Full::new(Bytes::from(
                                    OK,
                                ))))
                            } else {
                                Ok(http::Response::new(http_body_util::Full::new(Bytes::from(
                                    FAIL,
                                ))))
                            }
                        }
                        (method, path @ "/livez" | path @ "/readyz") => {
                            let mut res =
                                http::Response::new(http_body_util::Full::new(Bytes::from(
                                    format!("method `{method}` not supported for path `{path}`"),
                                )));
                            *res.status_mut() = http::StatusCode::METHOD_NOT_ALLOWED;
                            Ok(res)
                        }
                        (.., path) => {
                            let mut res = http::Response::new(http_body_util::Full::new(
                                Bytes::from(format!("path `{path}` not found")),
                            ));
                            *res.status_mut() = http::StatusCode::NOT_FOUND;
                            Ok(res)
                        }
                    }
                }
            }
        });
        let srv = hyper::server::conn::http1::Builder::new();
        Ok(async move {
            let mut tasks = JoinSet::new();
            info!("HTTP admin endpoint started");
            // TODO: check for shutdown, gracefully shutdown HTTP
            // TODO: join conn tasks
            loop {
                while let Some(res) = tasks.try_join_next() {
                    if let Err(err) = res {
                        error!(?err, "HTTP admin endpoint connection task panicked");
                    }
                }
                let stream = match sock.accept().await {
                    Ok((stream, addr)) => {
                        info!(?addr, "accepted HTTP admin connection");
                        stream
                    }
                    Err(err) => {
                        error!(?err, "failed to accept HTTP admin endpoint connection");
                        continue;
                    }
                };
                let conn = srv.serve_connection(TokioIo::new(stream), svc.clone());
                tasks.spawn(
                    async move {
                        if let Err(err) = conn.await {
                            warn!(?err, "failed to serve HTTP admin endpoint connection");
                        }
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span())
    }
}
