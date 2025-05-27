use core::future::Future;
use core::pin::{pin, Pin};
use core::task::{ready, Context, Poll};

use std::io::{IsTerminal as _, Stderr, Stdout, Write as _};
use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use rustix::net::sockopt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{oneshot, Mutex, Semaphore};
use tokio::time::Sleep;
use tracing::{debug, info};
use wasi::sockets::tcp::TcpState;
use wasmtime::component::{Linker, Resource, ResourceTable};

use crate::engine::wasi;
use crate::wasi::http::OutgoingBodyContentSender;
use crate::{Ctx, NOOP_WAKER};

pub mod error;
pub mod poll;
pub mod streams;

pub const MAX_BUFFER_SIZE: usize = 8192;

fn read_buffer_size(len: u64) -> usize {
    len.try_into()
        .unwrap_or(MAX_BUFFER_SIZE)
        .min(MAX_BUFFER_SIZE)
}

pub fn push_pollable(
    table: &mut ResourceTable,
    pollable: Pollable,
) -> wasmtime::Result<Resource<Pollable>> {
    table
        .push(pollable)
        .context("failed to push pollable resource to table")
}

#[derive(Debug)]
pub enum Error {
    Http(wasi::http::ErrorCode),
    Sockets(wasi::sockets::ErrorCode),
    Overflow,
    WouldBlock,
    WriteBudgetExceeded,
    ShortWrite(usize),
    Stdio(std::io::Error),
}

#[derive(Clone, Debug, Default)]
pub enum InputStream {
    #[default]
    Empty,
    TcpStream(Arc<TcpStream>),
    UdpSocket(Arc<UdpSocket>),
    Http(Arc<Mutex<HttpInputStream>>),
}

pub enum PendingOutputStream {
    Pending(oneshot::Receiver<OutputStream>),
    Ready(Box<OutputStream>),
}

#[derive(Debug, Default)]
pub enum OutputStream {
    #[default]
    Discard,
    Stdout(Stdout),
    Stderr(Stderr),
    TcpStream(Arc<TcpStream>),
    UdpSocket(Arc<UdpSocket>),
    Limited {
        budget: u64,
        stream: Box<Self>,
    },
    HttpPending(oneshot::Receiver<OutgoingBodyContentSender>),
    HttpWriting(OutgoingBodyContentSender),
    Tracing(tracing::Span),
}

impl OutputStream {
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Stdout(stdout) => stdout.is_terminal(),
            Self::Stderr(stderr) => stderr.is_terminal(),
            Self::Discard
            | Self::TcpStream(..)
            | Self::UdpSocket(..)
            | Self::Limited { .. }
            | Self::HttpPending(..)
            | Self::HttpWriting(..)
            | Self::Tracing(..) => false,
        }
    }
}

#[derive(Debug)]
pub enum SleepState {
    Pending(Pin<Box<Sleep>>),
    Ready,
}

impl SleepState {
    pub fn new(sleep: Sleep) -> Self {
        SleepState::Pending(Box::pin(sleep))
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self {
            Self::Pending(fut) => {
                ready!(fut.as_mut().poll(cx));
                *self = Self::Ready;
                Poll::Ready(())
            }
            Self::Ready => Poll::Ready(()),
        }
    }
}

#[derive(Debug)]
pub enum Pollable {
    TcpSocket(Arc<std::sync::RwLock<TcpState>>),
    TcpStreamReadable(Arc<TcpStream>),
    TcpStreamWritable(Arc<TcpStream>),
    UdpSocketReadable(Arc<UdpSocket>),
    UdpSocketWritable(Arc<UdpSocket>),
    Semaphore(Arc<Semaphore>),
    Sleep(Arc<std::sync::RwLock<SleepState>>),
    Ready,
}

impl Pollable {
    pub fn sleep(sleep: Sleep) -> Self {
        Self::Sleep(Arc::new(std::sync::RwLock::new(SleepState::new(sleep))))
    }
}

#[derive(Debug)]
pub struct HttpInputStream {
    pub buffer: Bytes,
    pub body: BoxBody<Bytes, wasi::http::ErrorCode>,
}

impl HttpInputStream {
    pub fn new(body: impl Into<BoxBody<Bytes, wasi::http::ErrorCode>>) -> Self {
        Self {
            buffer: Bytes::default(),
            body: body.into(),
        }
    }
}

impl Pollable {
    pub fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        match self {
            Pollable::TcpSocket(sock) => {
                let Ok(mut state) = sock.write() else {
                    return Poll::Ready(());
                };
                state.poll(cx)
            }
            Pollable::TcpStreamReadable(stream) => pin!(stream.readable()).poll(cx).map(|_| ()),
            Pollable::TcpStreamWritable(stream) => pin!(stream.writable()).poll(cx).map(|_| ()),
            Pollable::UdpSocketReadable(socket) => pin!(socket.readable()).poll(cx).map(|_| ()),
            Pollable::UdpSocketWritable(socket) => pin!(socket.writable()).poll(cx).map(|_| ()),
            Pollable::Semaphore(semaphore) => {
                if semaphore.available_permits() > 0 {
                    Poll::Ready(())
                } else {
                    pin!(semaphore.acquire()).poll(cx).map(|_| ())
                }
            }
            Pollable::Sleep(sleep) => {
                let Ok(mut state) = sleep.write() else {
                    return Poll::Ready(());
                };
                state.poll(cx)
            }
            Pollable::Ready => Poll::Ready(()),
        }
    }

    pub fn is_ready(&self) -> bool {
        match self {
            Pollable::TcpSocket(sock) => {
                let Ok(mut state) = sock.write() else {
                    return true;
                };
                state.poll(&mut Context::from_waker(NOOP_WAKER)).is_ready()
            }
            Pollable::TcpStreamReadable(stream) => pin!(stream.readable())
                .poll(&mut Context::from_waker(NOOP_WAKER))
                .is_ready(),
            Pollable::TcpStreamWritable(stream) => pin!(stream.writable())
                .poll(&mut Context::from_waker(NOOP_WAKER))
                .is_ready(),
            Pollable::UdpSocketReadable(socket) => pin!(socket.readable())
                .poll(&mut Context::from_waker(NOOP_WAKER))
                .is_ready(),
            Pollable::UdpSocketWritable(socket) => pin!(socket.writable())
                .poll(&mut Context::from_waker(NOOP_WAKER))
                .is_ready(),

            Pollable::Semaphore(semaphore) => semaphore.available_permits() > 0,
            Pollable::Sleep(sleep) => {
                if let Ok(SleepState::Pending(sleep)) = sleep.read().as_deref() {
                    sleep.is_elapsed()
                } else {
                    true
                }
            }
            Pollable::Ready => true,
        }
    }
}

impl OutputStream {
    fn check_write(&mut self) -> wasmtime::Result<Result<u64, Option<Error>>> {
        match self {
            Self::Discard => Ok(Ok(u64::MAX)),
            Self::Stdout(..) => Ok(Ok(u64::MAX)),
            Self::Stderr(..) => Ok(Ok(u64::MAX)),
            Self::TcpStream(stream) => {
                let max = match sockopt::socket_send_buffer_size(&stream) {
                    Ok(size) => size,
                    Err(err) => return Ok(Err(Some(Error::Sockets(err.into())))),
                };
                let mut max = u64::try_from(max).unwrap_or(u64::MAX);
                #[cfg(target_os = "macos")]
                {
                    use std::os::fd::AsRawFd as _;
                    let mut val: core::ffi::c_int = 0;
                    let mut size = size_of::<core::ffi::c_int>() as libc::socklen_t;
                    unsafe {
                        if libc::getsockopt(
                            stream.as_raw_fd(),
                            libc::SOL_SOCKET,
                            libc::SO_NWRITE,
                            &raw mut val as *mut _,
                            &raw mut size,
                        ) < 0
                        {
                            return Ok(Err(Some(Error::Sockets(
                                std::io::Error::last_os_error().into(),
                            ))));
                        }
                    };
                    max = max.saturating_sub(size.into());
                };
                #[cfg(target_os = "linux")]
                {
                    // TODO: ioctl SIOCOUTQ
                }
                Ok(Ok(max))
            }
            // TODO: implement UDP
            Self::UdpSocket(..) => Ok(Err(Some(Error::Sockets(
                wasi::sockets::ErrorCode::NotSupported,
            )))),
            Self::Limited { budget: 0, .. } => Ok(Err(Some(Error::WriteBudgetExceeded))),
            Self::Limited { budget, stream } => match stream.check_write()? {
                Ok(n) => Ok(Ok(n.min(*budget))),
                Err(err) => Ok(Err(err)),
            },
            Self::HttpPending(rx) => match rx.try_recv() {
                Ok(body) => {
                    *self = Self::HttpWriting(body);
                    self.check_write()
                }
                Err(oneshot::error::TryRecvError::Empty) => Ok(Err(Some(Error::Http(
                    wasi::http::ErrorCode::InternalError(Some(
                        "cannot write to unsent body".into(),
                    )),
                )))),
                Err(oneshot::error::TryRecvError::Closed) => Ok(Err(None)),
            },
            Self::HttpWriting(OutgoingBodyContentSender { conn, permits, .. }) => {
                let mut conn = match conn.try_lock() {
                    Ok(conn) => conn,
                    Err(..) => bail!("connection lock contended"),
                };
                match conn.try_recv() {
                    Ok(err) => Ok(Err(Some(Error::Http(err)))),
                    Err(oneshot::error::TryRecvError::Empty) => {
                        Ok(Ok(u32::try_from(permits.available_permits())
                            .unwrap_or(u32::MAX)
                            .into()))
                    }
                    Err(oneshot::error::TryRecvError::Closed) => Ok(Err(None)),
                }
            }
            Self::Tracing(..) => Ok(Ok(u64::MAX)),
        }
    }

    fn write(&mut self, contents: Vec<u8>) -> wasmtime::Result<Result<(), Option<Error>>> {
        match self {
            Self::Discard => {
                debug!(
                    contents = ?String::from_utf8_lossy(&contents),
                    "discard buffer"
                );
                Ok(Ok(()))
            }
            Self::Stdout(stdout) => {
                if let Err(err) = stdout.write_all(&contents) {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::Stderr(stderr) => {
                if let Err(err) = stderr.write_all(&contents) {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::TcpStream(stream) => match stream.try_write(&contents) {
                Ok(n) => {
                    if n < contents.len() {
                        Ok(Err(Some(Error::ShortWrite(n))))
                    } else {
                        Ok(Ok(()))
                    }
                }
                // WASI does not allow this, but we don't really care
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(Err(Some(
                    Error::Sockets(wasi::sockets::ErrorCode::WouldBlock),
                ))),
                Err(err) => Ok(Err(Some(Error::Sockets(err.into())))),
            },
            Self::UdpSocket(socket) => match socket.try_send(&contents) {
                Ok(n) => {
                    if n < contents.len() {
                        Ok(Err(Some(Error::ShortWrite(n))))
                    } else {
                        Ok(Ok(()))
                    }
                }
                // WASI does not allow this, but we don't really care
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(Err(Some(
                    Error::Sockets(wasi::sockets::ErrorCode::WouldBlock),
                ))),
                Err(err) => Ok(Err(Some(Error::Sockets(err.into())))),
            },
            Self::Limited { budget, stream } => {
                let Ok(n) = contents.len().try_into() else {
                    return Ok(Err(Some(Error::Overflow)));
                };
                let Some(rem) = budget.checked_sub(n) else {
                    return Ok(Err(Some(Error::WriteBudgetExceeded)));
                };
                match stream.write(contents)? {
                    Ok(()) => {
                        *budget = rem;
                        Ok(Ok(()))
                    }
                    Err(err) => Ok(Err(err)),
                }
            }
            Self::HttpPending(rx) => match rx.try_recv() {
                Ok(body) => {
                    let res = body.write(contents);
                    *self = Self::HttpWriting(body);
                    res
                }
                Err(oneshot::error::TryRecvError::Empty) => Ok(Err(Some(Error::WouldBlock))),
                Err(oneshot::error::TryRecvError::Closed) => Ok(Err(None)),
            },
            Self::HttpWriting(body) => body.write(contents),
            Self::Tracing(span) => {
                span.in_scope(|| info!(output = ?String::from_utf8_lossy(&contents)));
                Ok(Ok(()))
            }
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), Option<Error>>> {
        match self {
            Self::Discard => {
                debug!(
                    contents = ?String::from_utf8_lossy(&contents),
                    "discard buffer"
                );
                Ok(Ok(()))
            }
            Self::Stdout(stdout) => {
                if let Err(err) = stdout.write_all(&contents) {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                if let Err(err) = stdout.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::Stderr(stderr) => {
                if let Err(err) = stderr.write_all(&contents) {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                if let Err(err) = stderr.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::TcpStream(stream) => {
                let mut contents = contents.as_slice();
                loop {
                    match stream.try_write(contents) {
                        Ok(n) => {
                            if n == contents.len() {
                                return Ok(Ok(()));
                            } else {
                                contents = &contents[n..];
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if let Err(err) = stream.writable().await {
                                return Ok(Err(Some(Error::Sockets(err.into()))));
                            }
                        }
                        Err(err) => return Ok(Err(Some(Error::Sockets(err.into())))),
                    }
                }
            }
            Self::UdpSocket(socket) => {
                let mut contents = contents.as_slice();
                loop {
                    match socket.try_send(contents) {
                        Ok(n) => {
                            if n == contents.len() {
                                return Ok(Ok(()));
                            } else {
                                contents = &contents[n..];
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if let Err(err) = socket.writable().await {
                                return Ok(Err(Some(Error::Sockets(err.into()))));
                            }
                        }
                        Err(err) => return Ok(Err(Some(Error::Sockets(err.into())))),
                    }
                }
            }
            Self::Limited { budget, stream } => {
                let Ok(n) = contents.len().try_into() else {
                    return Ok(Err(Some(Error::Overflow)));
                };
                let Some(rem) = budget.checked_sub(n) else {
                    return Ok(Err(Some(Error::WriteBudgetExceeded)));
                };
                match Box::pin(stream.blocking_write_and_flush(contents)).await? {
                    Ok(()) => {
                        *budget = rem;
                        Ok(Ok(()))
                    }
                    Err(err) => Ok(Err(err)),
                }
            }
            Self::HttpPending(rx) => {
                let Ok(body) = rx.await else {
                    return Ok(Err(None));
                };
                let res = body.blocking_write_and_flush(contents).await;
                *self = Self::HttpWriting(body);
                res
            }
            Self::HttpWriting(body) => body.blocking_write_and_flush(contents).await,
            Self::Tracing(span) => {
                span.in_scope(|| info!(output = ?String::from_utf8_lossy(&contents)));
                Ok(Ok(()))
            }
        }
    }

    fn flush(&mut self) -> wasmtime::Result<Result<(), Option<Error>>> {
        match self {
            Self::Discard
            | Self::TcpStream(..)
            | Self::UdpSocket(..)
            | Self::HttpPending(..)
            | Self::HttpWriting(..)
            | Self::Tracing(..) => Ok(Ok(())),
            Self::Limited { stream, .. } => stream.flush(),
            Self::Stdout(stdout) => {
                if let Err(err) = stdout.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::Stderr(stderr) => {
                if let Err(err) = stderr.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
        }
    }

    async fn blocking_flush(&mut self) -> wasmtime::Result<Result<(), Option<Error>>> {
        match self {
            Self::Discard
            | Self::TcpStream(..)
            | Self::UdpSocket(..)
            | Self::HttpPending(..)
            | Self::HttpWriting(..)
            | Self::Tracing(..) => Ok(Ok(())),
            Self::Limited { stream, .. } => Box::pin(stream.blocking_flush()).await,
            Self::Stdout(stdout) => {
                if let Err(err) = stdout.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
            Self::Stderr(stderr) => {
                if let Err(err) = stderr.flush() {
                    return Ok(Err(Some(Error::Stdio(err))));
                }
                Ok(Ok(()))
            }
        }
    }

    fn subscribe(&mut self) -> wasmtime::Result<Pollable> {
        match self {
            Self::Discard
            | Self::Stdout(..)
            | Self::Stderr(..)
            | Self::HttpPending(..)
            | Self::Limited { budget: 0, .. }
            | Self::Tracing(..) => Ok(Pollable::Ready),
            Self::TcpStream(stream) => Ok(Pollable::TcpStreamWritable(Arc::clone(stream))),
            Self::UdpSocket(socket) => Ok(Pollable::UdpSocketWritable(Arc::clone(socket))),
            Self::Limited { stream, .. } => stream.subscribe(),
            Self::HttpWriting(OutgoingBodyContentSender { conn, permits, .. }) => {
                let conn = match conn.try_lock() {
                    Ok(conn) => conn,
                    Err(..) => bail!("connection lock contended"),
                };
                if !conn.is_empty() {
                    return Ok(Pollable::Ready);
                }
                Ok(Pollable::Semaphore(Arc::clone(permits)))
            }
        }
    }

    fn write_zeroes(&mut self, len: u64) -> wasmtime::Result<Result<(), Option<Error>>> {
        // TODO: optimize
        let len = len.try_into().context("invalid length")?;
        self.write(vec![0; len])
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        len: u64,
    ) -> wasmtime::Result<Result<(), Option<Error>>> {
        // TODO: optimize
        let len = len.try_into().context("invalid length")?;
        self.blocking_write_and_flush(vec![0; len]).await
    }

    fn splice(
        &mut self,
        src: &InputStream,
        len: u64,
    ) -> wasmtime::Result<Result<u64, Option<Error>>> {
        // TODO: optimize
        let n = match self.check_write()? {
            Ok(n) => n,
            Err(err) => return Ok(Err(err)),
        };
        let buf = match src.read(n.min(len))? {
            Ok(buf) => buf,
            Err(err) => return Ok(Err(err)),
        };
        let n = buf.len();
        match self.write(buf)? {
            Ok(()) => Ok(Ok(n as _)),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn blocking_splice(
        &mut self,
        src: &InputStream,
        len: u64,
    ) -> wasmtime::Result<Result<u64, Option<Error>>> {
        // TODO: optimize
        let buf = match src.blocking_read(len).await? {
            Ok(buf) => buf,
            Err(err) => return Ok(Err(err)),
        };
        let n = buf.len();
        match self.blocking_write_and_flush(buf).await? {
            Ok(()) => Ok(Ok(n as _)),
            Err(err) => Ok(Err(err)),
        }
    }
}

impl InputStream {
    fn read(&self, len: u64) -> wasmtime::Result<Result<Vec<u8>, Option<Error>>> {
        match self {
            InputStream::Empty => Ok(Err(None)),
            InputStream::TcpStream(stream) => {
                let len = read_buffer_size(len);
                let mut buf = vec![0; len];
                match stream.try_read(&mut buf) {
                    Ok(0) => {
                        if len == 0 {
                            Ok(Ok(Vec::default()))
                        } else {
                            Ok(Err(None))
                        }
                    }
                    Ok(n) => {
                        buf.truncate(n);
                        Ok(Ok(buf))
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        Ok(Ok(Vec::default()))
                    }
                    Err(err) => Ok(Err(Some(Error::Sockets(err.into())))),
                }
            }
            InputStream::UdpSocket(sock) => {
                let len = read_buffer_size(len);
                let mut buf = vec![0; len];
                match sock.try_recv(&mut buf) {
                    Ok(0) => {
                        if len == 0 {
                            Ok(Ok(Vec::default()))
                        } else {
                            Ok(Err(None))
                        }
                    }
                    Ok(n) => {
                        buf.truncate(n);
                        Ok(Ok(buf))
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        Ok(Ok(Vec::default()))
                    }
                    Err(err) => Ok(Err(Some(Error::Sockets(err.into())))),
                }
            }
            // TODO: Implement HTTP streaming
            InputStream::Http(_stream) => Ok(Err(Some(Error::Http(
                wasi::http::ErrorCode::InternalError(Some("not supported yet".into())),
            )))),
        }
    }

    async fn blocking_read(&self, len: u64) -> wasmtime::Result<Result<Vec<u8>, Option<Error>>> {
        match self {
            InputStream::Empty => Ok(Err(None)),
            InputStream::TcpStream(stream) => {
                let len = read_buffer_size(len);
                let mut buf = vec![0; len];
                loop {
                    match stream.try_read(&mut buf) {
                        Ok(0) => {
                            if len == 0 {
                                return Ok(Ok(Vec::default()));
                            } else {
                                return Ok(Err(None));
                            }
                        }
                        Ok(n) => {
                            buf.truncate(n);
                            return Ok(Ok(buf));
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if let Err(err) = stream.readable().await {
                                return Ok(Err(Some(Error::Sockets(err.into()))));
                            }
                        }
                        Err(err) => {
                            return Ok(Err(Some(Error::Sockets(err.into()))));
                        }
                    }
                }
            }
            InputStream::UdpSocket(sock) => {
                let len = read_buffer_size(len);
                let mut buf = vec![0; len];
                loop {
                    match sock.try_recv(&mut buf) {
                        Ok(0) => {
                            if len == 0 {
                                return Ok(Ok(Vec::default()));
                            } else {
                                return Ok(Err(None));
                            }
                        }
                        Ok(n) => {
                            buf.truncate(n);
                            return Ok(Ok(buf));
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            if let Err(err) = sock.readable().await {
                                return Ok(Err(Some(Error::Sockets(err.into()))));
                            }
                        }
                        Err(err) => {
                            return Ok(Err(Some(Error::Sockets(err.into()))));
                        }
                    }
                }
            }
            // TODO: Implement HTTP streaming
            InputStream::Http(_stream) => Ok(Err(Some(Error::Http(
                wasi::http::ErrorCode::InternalError(Some("not supported yet".into())),
            )))),
        }
    }

    fn skip(&self, len: u64) -> wasmtime::Result<Result<u64, Option<Error>>> {
        // TODO: optimize
        match self.read(len)? {
            Ok(buf) => Ok(Ok(buf.len() as _)),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn blocking_skip(&self, len: u64) -> wasmtime::Result<Result<u64, Option<Error>>> {
        // TODO: optimize
        match self.blocking_read(len).await? {
            Ok(buf) => Ok(Ok(buf.len() as _)),
            Err(err) => Ok(Err(err)),
        }
    }

    fn subscribe(&self) -> wasmtime::Result<Pollable> {
        match self {
            InputStream::Empty => Ok(Pollable::Ready),
            InputStream::TcpStream(stream) => Ok(Pollable::TcpStreamReadable(Arc::clone(stream))),
            InputStream::UdpSocket(socket) => Ok(Pollable::UdpSocketReadable(Arc::clone(socket))),
            // TODO: Implement HTTP streaming
            InputStream::Http(_stream) => Ok(Pollable::Ready),
        }
    }
}

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> wasmtime::Result<()> {
    use crate::engine::bindings::wasi::io;
    io::error::add_to_linker(linker, get)?;
    io::poll::add_to_linker(linker, get)?;
    io::streams::add_to_linker(linker, get)?;
    Ok(())
}
