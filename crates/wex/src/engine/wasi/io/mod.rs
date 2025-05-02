use core::future::Future;
use core::pin::{pin, Pin};
use core::sync::atomic::Ordering;
use core::task::{ready, Context, Poll, Waker};
use core::time::Duration;

use std::{
    io::{Stderr, Stdout, Write as _},
    sync::{Arc, TryLockError},
};

use anyhow::{bail, Context as _};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use rustix::net::sockopt;
use tokio::io::{AsyncRead, AsyncWriteExt as _};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::oneshot::error::TryRecvError;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::time::sleep;
use tokio::time::Sleep;
use tracing::debug;
use wasi::sockets::tcp::TcpState;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime::AsContextMut;

use crate::engine::bindings::wasi::io::streams::StreamError;
use crate::engine::{wasi, ResourceView};
use crate::{EPOCH_MONOTONIC_NOW, NOOP_WAKER};

use super::http::{OutgoingBodyContentSender, OutgoingBodySender};

pub mod error;
pub mod poll;
pub mod streams;

pub const MAX_BUFFER_SIZE: usize = 8192;

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

pub enum InputStream {
    Empty,
    TcpStream(Arc<TcpStream>),
    UdpSocket(Arc<UdpSocket>),
    //Receiver(Arc<Mutex<ChannelInputStream>>),
    Http(Arc<Mutex<HttpInputStream>>),
    Bytes(Bytes),
    AsyncRead(AsyncReadInputStream),
}

pub enum PendingOutputStream {
    Pending(oneshot::Receiver<OutputStream>),
    Ready(Box<OutputStream>),
}

pub enum OutputStream {
    Discard,
    Stdout(Stdout),
    Stderr(Stderr),
    TcpStream(Arc<TcpStream>),
    UdpSocket(Arc<UdpSocket>),
    Limited { budget: u64, stream: Box<Self> },
    HttpPending(oneshot::Receiver<OutgoingBodyContentSender>),
    HttpWriting(OutgoingBodyContentSender),
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
    //Receiver(Arc<Mutex<ChannelInputStream>>),
    Semaphore(Arc<Semaphore>),
    Sleep(Arc<std::sync::RwLock<SleepState>>),
    Ready,
}

impl Pollable {
    pub fn sleep(sleep: Sleep) -> Self {
        Self::Sleep(Arc::new(std::sync::RwLock::new(SleepState::new(sleep))))
    }
}

pub struct AsyncReadInputStream {
    pub buffer: Bytes,
    pub io: Box<dyn AsyncRead + Send>,
}

impl AsyncReadInputStream {
    pub fn new(io: impl Into<Box<dyn AsyncRead + Send>>) -> Self {
        Self {
            buffer: Bytes::default(),
            io: io.into(),
        }
    }
}

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

pub struct ChannelInputStream {
    buffer: Bytes,
    rx: mpsc::Receiver<Bytes>,
}

//pub struct UnbufferedOutputStream {
//    permits: Arc<Semaphore>,
//    data: UnboundedSender<(Vec<u8>, OwnedSemaphorePermit)>,
//    err: Arc<Mutex<oneshot::Receiver<Error>>>,
//}
//
//pub struct BufferedOutputStream {
//    permits: Arc<Semaphore>,
//    write: UnboundedSender<(Vec<u8>, OwnedSemaphorePermit, oneshot::Sender<Error>)>,
//    write_errs: Vec<oneshot::Receiver<Error>>,
//
//    flush: mpsc::Sender<oneshot::Sender<Error>>,
//    flush_err: Option<oneshot::Receiver<Error>>,
//}
//
//impl BufferedOutputStream {
//    fn take_write_error(&mut self) -> Option<Error> {
//        let mut i = 0usize;
//        for rx in &mut self.write_errs {
//            match rx.try_recv() {
//                Ok(err) => {
//                    self.write_errs = self.write_errs.split_off(i.saturating_add(1));
//                    return Some(err);
//                }
//                Err(oneshot::error::TryRecvError::Closed) => {
//                    i = i.saturating_add(1);
//                }
//                Err(oneshot::error::TryRecvError::Empty) => break,
//            }
//        }
//        if i == self.write_errs.len() {
//            self.write_errs.clear();
//        } else if i > 0 {
//            self.write_errs = self.write_errs.split_off(i);
//        }
//        None
//    }
//
//    fn take_error(&mut self) -> Option<Error> {
//        if let Some(err) = self.take_write_error() {
//            return Some(err);
//        };
//
//        if let Some(rx) = self.flush_err.as_mut() {
//            match rx.try_recv() {
//                Ok(err) => {
//                    self.flush_err = None;
//                    return Some(err);
//                }
//                Err(oneshot::error::TryRecvError::Closed) => {
//                    self.flush_err = None;
//                }
//                Err(oneshot::error::TryRecvError::Empty) => {}
//            }
//        }
//        None
//    }
//
//    fn check_write(&mut self) -> Result<Option<u64>, Error> {
//        if let Some(err) = self.take_write_error() {
//            return Err(err);
//        }
//        if let Some(rx) = self.flush_err.as_mut() {
//            match rx.try_recv() {
//                Ok(err) => {
//                    self.flush_err = None;
//                    return Err(err);
//                }
//                Err(oneshot::error::TryRecvError::Closed) => {
//                    self.flush_err = None;
//                }
//                Err(oneshot::error::TryRecvError::Empty) => return Ok(Some(0)),
//            }
//        }
//        let cap = self
//            .permits
//            .available_permits()
//            .try_into()
//            .unwrap_or(u64::MAX);
//        Ok(Some(cap))
//    }
//}

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
            //Pollable::Receiver(stream) => {
            //    let mut stream = ready!(pin!(stream.lock()).poll(cx));
            //    let ChannelInputStream { buffer, ref mut rx } = &mut *stream;
            //    if buffer.is_empty() {
            //        if let Some(buf) = ready!(pin!(rx.recv()).poll(cx)) {
            //            *buffer = buf
            //        }
            //    }
            //    Poll::Ready(())
            //}
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

            //Pollable::Receiver(stream) => match stream.try_lock().as_deref() {
            //    Ok(ChannelInputStream { buffer, rx }) => !buffer.is_empty() || !rx.is_empty(),
            //    Err(..) => true,
            //},
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
            Self::Stdout(stdout) => Ok(Ok(u64::MAX)),
            Self::Stderr(stderr) => Ok(Ok(u64::MAX)),
            Self::TcpStream(stream) => {
                let mut max = match sockopt::socket_send_buffer_size(&stream) {
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
            Self::UdpSocket(_socket) => bail!("not supported yet udp sock check w"),
            //Self::Buffered(stream) => match stream.check_write() {
            //    Ok(Some(n)) => Ok(Ok(n)),
            //    Ok(None) => Ok(Err(None)),
            //    Err(err) => Ok(Err(Some(err))),
            //},
            //Self::Unbuffered(UnbufferedOutputStream {
            //    err: conn,
            //    permits,
            //    data,
            //}) => {
            //    let Ok(mut rx) = conn.try_lock() else {
            //        return Ok(Ok(0));
            //    };
            //    match rx.try_recv() {
            //        Ok(err) => {
            //            drop(rx);
            //            Ok(Err(Some(err)))
            //        }
            //        Err(oneshot::error::TryRecvError::Empty) => Ok(Ok(permits
            //            .available_permits()
            //            .try_into()
            //            .unwrap_or(u64::MAX))),
            //        Err(oneshot::error::TryRecvError::Closed) => Ok(Err(None)),
            //    }
            //}
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
            //Self::Buffered(stream) => {
            //    let n = contents
            //        .len()
            //        .try_into()
            //        .context("content length does not fit in u32")?;

            //    if let Some(err) = stream.take_write_error() {
            //        return Ok(Err(Some(err)));
            //    };

            //    if let Some(rx) = stream.flush_err.as_mut() {
            //        match rx.try_recv() {
            //            Ok(err) => {
            //                stream.flush_err = None;
            //                return Ok(Err(Some(err)));
            //            }
            //            Err(oneshot::error::TryRecvError::Closed) => {
            //                stream.flush_err = None;
            //            }
            //            Err(oneshot::error::TryRecvError::Empty) => bail!("write not permitted"),
            //        }
            //    }
            //    if let Some(err) = stream.take_error() {
            //        return Ok(Err(Some(err)));
            //    }
            //    let permit = match Arc::clone(&stream.permits).try_acquire_many_owned(n) {
            //        Ok(permit) => permit,
            //        Err(TryAcquireError::Closed) => return Ok(Err(None)),
            //        Err(TryAcquireError::NoPermits) => bail!("write not permitted"),
            //    };
            //    let (tx, rx) = oneshot::channel();
            //    match stream.write.send((contents, permit, tx)) {
            //        Ok(()) => {
            //            stream.write_errs.push(rx);
            //            Ok(Ok(()))
            //        }
            //        Err(..) => Ok(Err(None)),
            //    }
            //}
            //Self::Unbuffered(UnbufferedOutputStream {
            //    err: conn,
            //    permits,
            //    data,
            //}) => {
            //    let Ok(mut rx) = conn.try_lock() else {
            //        bail!("write not permitted")
            //    };
            //    match rx.try_recv() {
            //        Ok(err) => {
            //            drop(rx);
            //            Ok(Err(Some(err)))
            //        }
            //        Err(oneshot::error::TryRecvError::Empty) => {
            //            let n = contents
            //                .len()
            //                .try_into()
            //                .context("content length does not fit in u32")?;
            //            match Arc::clone(&permits).try_acquire_many_owned(n) {
            //                Ok(permit) => match data.send((contents, permit)) {
            //                    Ok(()) => Ok(Ok(())),
            //                    Err(..) => Ok(Err(None)),
            //                },
            //                Err(TryAcquireError::Closed) => Ok(Err(None)),
            //                Err(TryAcquireError::NoPermits) => bail!("write not permitted"),
            //            }
            //        }
            //        Err(oneshot::error::TryRecvError::Closed) => Ok(Err(None)),
            //    }
            //}
            Self::Limited { budget, stream } => {
                let Ok(n) = contents.len().try_into() else {
                    return Ok(Err(Some(Error::Overflow)));
                };
                let Some(rem) = budget.checked_sub(n) else {
                    return Ok(Err(Some(Error::WriteBudgetExceeded)));
                };
                stream.write(contents)?;
                *budget = rem;
                Ok(Ok(()))
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
                Box::pin(stream.blocking_write_and_flush(contents)).await?;
                *budget = rem;
                Ok(Ok(()))
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
        }
    }

    fn flush(&mut self) -> wasmtime::Result<Result<(), Option<Error>>> {
        match self {
            Self::Discard
            | Self::TcpStream(..)
            | Self::UdpSocket(..)
            | Self::HttpPending(..)
            | Self::HttpWriting(..) => Ok(Ok(())),
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
            | Self::HttpWriting(..) => Ok(Ok(())),
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
            | Self::Limited { budget: 0, .. } => Ok(Pollable::Ready),
            Self::TcpStream(stream) => Ok(Pollable::TcpStreamWritable(Arc::clone(stream))),
            Self::UdpSocket(socket) => Ok(Pollable::UdpSocketWritable(Arc::clone(socket))),
            //Self::Buffered(BufferedOutputStream { permits, .. })
            //| Self::Unbuffered(UnbufferedOutputStream { permits, .. }) => {
            //    Pollable::Semaphore(Arc::clone(permits))
            //}
            Self::Limited { stream, .. } => stream.subscribe(),
            Self::HttpWriting(OutgoingBodyContentSender { conn, permits, .. }) => {
                let mut conn = match conn.try_lock() {
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
}

impl InputStream {
    fn read(&mut self, len: u64) -> wasmtime::Result<Result<Vec<u8>, Option<Error>>> {
        match self {
            InputStream::Empty => Ok(Err(None)),
            InputStream::TcpStream(stream) => {
                let len = len
                    .try_into()
                    .unwrap_or(MAX_BUFFER_SIZE)
                    .max(MAX_BUFFER_SIZE);
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
                    Err(err) => {
                        Ok(Err(Some(Error::Sockets(err.into()))))
                    }
                }
            }
            InputStream::UdpSocket(socket) => todo!(),
            InputStream::Http(mutex) => todo!(),
            InputStream::Bytes(bytes) => todo!(),
            InputStream::AsyncRead(async_read_input_stream) => todo!(),
        }
    }

    async fn blocking_read(
        &mut self,
        len: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, Option<Error>>> {
        match self {
            InputStream::Empty => Ok(Err(None)),
            InputStream::TcpStream(stream) => {
                let len = len
                    .try_into()
                    .unwrap_or(MAX_BUFFER_SIZE)
                    .max(MAX_BUFFER_SIZE);
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
            InputStream::UdpSocket(socket) => todo!(),
            InputStream::Http(mutex) => todo!(),
            InputStream::Bytes(bytes) => todo!(),
            InputStream::AsyncRead(async_read_input_stream) => todo!(),
        }
    }

    fn subscribe(&mut self) -> wasmtime::Result<Pollable> {
        match self {
            InputStream::Empty => Ok(Pollable::Ready),
            InputStream::TcpStream(stream) => Ok(Pollable::TcpStreamReadable(Arc::clone(stream))),
            InputStream::UdpSocket(socket) => Ok(Pollable::UdpSocketReadable(Arc::clone(socket))),
            InputStream::Http(mutex) => todo!(),
            InputStream::Bytes(bytes) => todo!(),
            InputStream::AsyncRead(async_read_input_stream) => todo!(),
        }
    }
}
