use core::future::{poll_fn, Future};
use core::mem;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::HashMap;
use std::net::Shutdown;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::task::noop_waker_ref;
use rustix::io::Errno;
use rustix::net::sockopt;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::{Mutex, MutexGuard};
use tracing::{debug, error, info, instrument, trace};
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::clocks::monotonic_clock::Duration;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi::{
    async_trait, subscribe, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
    SocketError, StreamError, Subscribe,
};

use crate::bindings::wasi::sockets::tcp::ShutdownType;
use crate::bindings::wasi::sockets::udp::{IncomingDatagram, OutgoingDatagram};
use crate::bindings::wasi::sockets::{
    instance_network, ip_name_lookup, network, tcp, tcp_create_socket, udp, udp_create_socket,
};
use crate::wasi::sockets::{Network, SocketAddressFamily};
use crate::Ctx;
use crate::{
    bindings::wasi::sockets::network::{
        ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress,
        Ipv6SocketAddress,
    },
    config,
};

/// Value taken from rust std library.
const DEFAULT_BACKLOG: u32 = 128;

/// The state of a TCP socket.
///
/// This represents the various states a socket can be in during the
/// activities of binding, listening, accepting, and connecting.
enum State {
    /// The initial state for a newly-created socket.
    Default(TcpSocket),

    /// Binding started via `start_bind`.
    BindStarted(TcpSocket),

    /// Binding finished via `finish_bind`. The socket has an address but
    /// is not yet listening for connections.
    Bound(TcpSocket),

    /// Listening started via `listen_start`.
    ListenStarted(TcpSocket),

    /// The socket is now listening and waiting for an incoming connection.
    Listening {
        listener: TcpListener,
        pending_accept: Option<std::io::Result<TcpStream>>,
    },

    /// An outgoing connection is started via `start_connect`.
    Connecting(Pin<Box<dyn Future<Output = std::io::Result<TcpStream>> + Send>>),

    /// An outgoing connection is ready to be established.
    ConnectReady(std::io::Result<TcpStream>),

    /// An outgoing connection has been established.
    Connected {
        stream: Arc<TcpStream>,

        // WASI is single threaded, so in practice these Mutexes should never be contended:
        reader: Arc<Mutex<TcpReader>>,
        writer: Arc<Mutex<TcpWriter>>,
    },

    Closed,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default(_) => f.debug_tuple("Default").finish(),
            Self::BindStarted(_) => f.debug_tuple("BindStarted").finish(),
            Self::Bound(_) => f.debug_tuple("Bound").finish(),
            Self::ListenStarted(_) => f.debug_tuple("ListenStarted").finish(),
            Self::Listening { pending_accept, .. } => f
                .debug_struct("Listening")
                .field("pending_accept", pending_accept)
                .finish(),
            Self::Connecting(_) => f.debug_tuple("Connecting").finish(),
            Self::ConnectReady(_) => f.debug_tuple("ConnectReady").finish(),
            Self::Connected { .. } => f.debug_tuple("Connected").finish(),
            Self::Closed => write!(f, "Closed"),
        }
    }
}

pub struct Socket {
    /// The current state in the bind/listen/accept/connect progression.
    state: State,

    /// The desired listen queue size.
    listen_backlog_size: u32,

    family: SocketAddressFamily,

    // The socket options below are not automatically inherited from the listener
    // on all platforms. So we keep track of which options have been explicitly
    // set and manually apply those values to newly accepted clients.
    #[cfg(target_os = "macos")]
    receive_buffer_size: Option<usize>,
    #[cfg(target_os = "macos")]
    send_buffer_size: Option<usize>,
    #[cfg(target_os = "macos")]
    hop_limit: Option<u8>,
    #[cfg(target_os = "macos")]
    keep_alive_idle_time: Option<core::time::Duration>,
}

impl Socket {
    /// Create a `TcpSocket` from an existing socket state.
    fn from_state(state: State, family: SocketAddressFamily) -> Self {
        Self {
            state,
            listen_backlog_size: DEFAULT_BACKLOG,
            family,
            #[cfg(target_os = "macos")]
            receive_buffer_size: None,
            #[cfg(target_os = "macos")]
            send_buffer_size: None,
            #[cfg(target_os = "macos")]
            hop_limit: None,
            #[cfg(target_os = "macos")]
            keep_alive_idle_time: None,
        }
    }
}

#[async_trait]
impl Subscribe for Socket {
    async fn ready(&mut self) {
        match &mut self.state {
            State::Default(..)
            | State::BindStarted(..)
            | State::Bound(..)
            | State::ListenStarted(..)
            | State::ConnectReady(..)
            | State::Closed
            | State::Connected { .. } => {
                // No async operation in progress.
            }
            State::Connecting(future) => {
                self.state = State::ConnectReady(future.as_mut().await);
            }
            State::Listening {
                listener,
                pending_accept,
            } => match pending_accept {
                Some(_) => {}
                None => {
                    let result =
                        poll_fn(|cx| listener.poll_accept(cx).map_ok(|(stream, _)| stream)).await;
                    *pending_accept = Some(result);
                }
            },
        }
    }
}

struct TcpReader {
    stream: Arc<TcpStream>,
    closed: bool,
}

impl TcpReader {
    fn new(stream: Arc<TcpStream>) -> Self {
        Self {
            stream,
            closed: false,
        }
    }
    fn read(&mut self, size: usize) -> Result<Bytes, StreamError> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        if size == 0 {
            return Ok(Bytes::new());
        }

        let mut buf = BytesMut::with_capacity(size);
        let n = match self.stream.try_read_buf(&mut buf) {
            // A 0-byte read indicates that the stream has closed.
            Ok(0) => {
                self.closed = true;
                return Err(StreamError::Closed);
            }
            Ok(n) => n,

            // Failing with `EWOULDBLOCK` is how we differentiate between a closed channel and no
            // data to read right now.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => 0,

            Err(e) => {
                self.closed = true;
                return Err(StreamError::LastOperationFailed(e.into()));
            }
        };

        buf.truncate(n);
        Ok(buf.freeze())
    }

    fn shutdown(&mut self) {
        native_shutdown(&self.stream, Shutdown::Read);
        self.closed = true;
    }

    async fn ready(&mut self) {
        if self.closed {
            return;
        }

        self.stream.readable().await.unwrap();
    }
}

struct TcpReadStream(Arc<Mutex<TcpReader>>);

#[async_trait]
impl HostInputStream for TcpReadStream {
    fn read(&mut self, size: usize) -> Result<Bytes, StreamError> {
        try_lock_for_stream(&self.0)?.read(size)
    }
}

#[async_trait]
impl Subscribe for TcpReadStream {
    async fn ready(&mut self) {
        self.0.lock().await.ready().await
    }
}

const SOCKET_READY_SIZE: usize = 1024 * 1024 * 1024;

struct TcpWriter {
    stream: Arc<TcpStream>,
    state: WriteState,
}

enum WriteState {
    Ready,
    Writing(AbortOnDropJoinHandle<std::io::Result<()>>),
    Closing(AbortOnDropJoinHandle<std::io::Result<()>>),
    Closed,
    Error(std::io::Error),
}

impl TcpWriter {
    fn new(stream: Arc<TcpStream>) -> Self {
        Self {
            stream,
            state: WriteState::Ready,
        }
    }

    fn try_write_portable(stream: &TcpStream, buf: &[u8]) -> std::io::Result<usize> {
        stream.try_write(buf).map_err(|error| {
            match Errno::from_io_error(&error) {
                // Windows returns `WSAESHUTDOWN` when writing to a shut down socket.
                // We normalize this to EPIPE, because that is what the other platforms return.
                // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-send#:~:text=WSAESHUTDOWN
                #[cfg(windows)]
                Some(Errno::SHUTDOWN) => io::Error::new(io::ErrorKind::BrokenPipe, error),

                _ => error,
            }
        })
    }

    /// Write `bytes` in a background task, remembering the task handle for use in a future call to
    /// `write_ready`
    fn background_write(&mut self, mut bytes: Bytes) {
        assert!(matches!(self.state, WriteState::Ready));

        let stream = self.stream.clone();
        self.state = WriteState::Writing(wasmtime_wasi::runtime::spawn(async move {
            // Note: we are not using the AsyncWrite impl here, and instead using the TcpStream
            // primitive try_write, which goes directly to attempt a write with mio. This has
            // two advantages: 1. this operation takes a &TcpStream instead of a &mut TcpStream
            // required to AsyncWrite, and 2. it eliminates any buffering in tokio we may need
            // to flush.
            while !bytes.is_empty() {
                stream.writable().await?;
                match Self::try_write_portable(&stream, &bytes) {
                    Ok(n) => {
                        let _ = bytes.split_to(n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(())
        }));
    }

    fn write(&mut self, mut bytes: Bytes) -> Result<(), StreamError> {
        match self.state {
            WriteState::Ready => {}
            WriteState::Closed => return Err(StreamError::Closed),
            WriteState::Writing(_) | WriteState::Closing(_) | WriteState::Error(_) => {
                return Err(StreamError::Trap(anyhow::anyhow!(
                    "unpermitted: must call check_write first"
                )));
            }
        }
        while !bytes.is_empty() {
            match Self::try_write_portable(&self.stream, &bytes) {
                Ok(n) => {
                    let _ = bytes.split_to(n);
                }

                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // As `try_write` indicated that it would have blocked, we'll perform the write
                    // in the background to allow us to return immediately.
                    self.background_write(bytes);

                    return Ok(());
                }

                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                    self.state = WriteState::Closed;
                    return Err(StreamError::Closed);
                }

                Err(e) => return Err(StreamError::LastOperationFailed(e.into())),
            }
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        // `flush` is a no-op here, as we're not managing any internal buffer. Additionally,
        // `write_ready` will join the background write task if it's active, so following `flush`
        // with `write_ready` will have the desired effect.
        match self.state {
            WriteState::Ready
            | WriteState::Writing(_)
            | WriteState::Closing(_)
            | WriteState::Error(_) => Ok(()),
            WriteState::Closed => Err(StreamError::Closed),
        }
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        match mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Writing(task) => {
                self.state = WriteState::Writing(task);
                return Ok(0);
            }
            WriteState::Closing(task) => {
                self.state = WriteState::Closing(task);
                return Ok(0);
            }
            WriteState::Ready => {
                self.state = WriteState::Ready;
            }
            WriteState::Closed => return Err(StreamError::Closed),
            WriteState::Error(e) => return Err(StreamError::LastOperationFailed(e.into())),
        }

        let writable = self.stream.writable();
        futures::pin_mut!(writable);
        if wasmtime_wasi::runtime::poll_noop(writable).is_none() {
            return Ok(0);
        }
        Ok(SOCKET_READY_SIZE)
    }

    fn shutdown(&mut self) {
        self.state = match mem::replace(&mut self.state, WriteState::Closed) {
            // No write in progress, immediately shut down:
            WriteState::Ready => {
                native_shutdown(&self.stream, Shutdown::Write);
                WriteState::Closed
            }

            // Schedule the shutdown after the current write has finished:
            WriteState::Writing(write) => {
                let stream = self.stream.clone();
                WriteState::Closing(wasmtime_wasi::runtime::spawn(async move {
                    let result = write.await;
                    native_shutdown(&stream, Shutdown::Write);
                    result
                }))
            }

            s => s,
        };
    }

    async fn cancel(&mut self) {
        match mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Writing(task) | WriteState::Closing(task) => _ = task.abort(),
            _ => {}
        }
    }

    async fn ready(&mut self) {
        match &mut self.state {
            WriteState::Writing(task) => {
                self.state = match task.await {
                    Ok(()) => WriteState::Ready,
                    Err(e) => WriteState::Error(e),
                }
            }
            WriteState::Closing(task) => {
                self.state = match task.await {
                    Ok(()) => WriteState::Closed,
                    Err(e) => WriteState::Error(e),
                }
            }
            _ => {}
        }

        if let WriteState::Ready = self.state {
            self.stream.writable().await.unwrap();
        }
    }
}

struct TcpWriteStream(Arc<Mutex<TcpWriter>>);

#[async_trait]
impl HostOutputStream for TcpWriteStream {
    fn write(&mut self, bytes: Bytes) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.write(bytes)
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.flush()
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        try_lock_for_stream(&self.0)?.check_write()
    }

    async fn cancel(&mut self) {
        self.0.lock().await.cancel().await
    }
}

#[async_trait]
impl Subscribe for TcpWriteStream {
    async fn ready(&mut self) {
        self.0.lock().await.ready().await
    }
}

fn native_shutdown(stream: &TcpStream, how: Shutdown) {
    todo!()
    //_ = stream
    //    .as_socketlike_view::<std::net::TcpStream>()
    //    .shutdown(how);
}

fn try_lock_for_stream<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, StreamError> {
    mutex
        .try_lock()
        .map_err(|_| StreamError::trap("concurrent access to resource not supported"))
}

fn try_lock_for_socket<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, SocketError> {
    mutex.try_lock().map_err(|_| {
        SocketError::trap(anyhow::anyhow!(
            "concurrent access to resource not supported"
        ))
    })
}

#[async_trait]
impl tcp_create_socket::Host for Ctx {
    async fn create_tcp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> wasmtime::Result<Result<Resource<Socket>, ErrorCode>> {
        let (sock, family) = match match address_family {
            IpAddressFamily::Ipv4 => (
                TcpSocket::new_v4().map_err(ErrorCode::from),
                SocketAddressFamily::Ipv4,
            ),
            IpAddressFamily::Ipv6 => (
                TcpSocket::new_v6()
                    .map_err(ErrorCode::from)
                    .and_then(|sock| {
                        sockopt::set_ipv6_v6only(&sock, true)?;
                        Ok(sock)
                    }),
                SocketAddressFamily::Ipv6,
            ),
        } {
            (Ok(sock), family) => (sock, family),
            (Err(err), ..) => return Ok(Err(err)),
        };
        let sock = self
            .table
            .push(Socket {
                state: State::Default(sock),
                listen_backlog_size: DEFAULT_BACKLOG,
                family,
                #[cfg(target_os = "macos")]
                receive_buffer_size: None,
                #[cfg(target_os = "macos")]
                send_buffer_size: None,
                #[cfg(target_os = "macos")]
                hop_limit: None,
                #[cfg(target_os = "macos")]
                keep_alive_idle_time: None,
            })
            .context("failed to push socket to table")?;
        Ok(Ok(sock))
    }
}

#[async_trait]
impl tcp::Host for Ctx {}

#[async_trait]
impl tcp::HostTcpSocket for Ctx {
    async fn start_bind(
        &mut self,
        sock: Resource<Socket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut local_address = SocketAddr::from(local_address);
        // TODO: Validate address
        //
        //network::util::validate_unicast(&remote_address)?;
        //network::util::validate_address_family(&remote_address, &self.family)?;

        let network = self
            .table
            .get(&network)
            .context("failed to get network from table")?;
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        match mem::replace(&mut sock.state, State::Closed) {
            State::Default(socket) => {
                match &mut local_address {
                    SocketAddr::V4(local_address) => match &self.network.tcp.ipv4 {
                        config::component::network::Network::None { loopback } => {
                            let ip = local_address.ip();
                            if !ip.is_loopback() && !ip.is_unspecified() {
                                return Ok(Err(ErrorCode::AddressNotBindable));
                            }
                            match loopback {
                                config::component::network::none::Loopback::None => {
                                    return Ok(Err(ErrorCode::AddressNotBindable))
                                }
                                config::component::network::none::Loopback::Tun => {
                                    local_address.set_ip(self.ipv4_tun_addr);
                                }
                                config::component::network::none::Loopback::Composition {
                                    ..
                                } => {
                                    error!("TODO: implement");
                                    return Ok(Err(ErrorCode::AddressNotBindable));
                                }
                            }
                        }
                        config::component::network::Network::Host(
                            config::component::network::host::Config {
                                address,
                                ports,
                                loopback,
                            },
                        ) => {
                            let ip = local_address.ip();
                            if ip.is_loopback() {
                                match loopback {
                                    config::component::network::host::Loopback::None => {
                                        return Ok(Err(ErrorCode::AddressNotBindable))
                                    }
                                    config::component::network::host::Loopback::Tun => {
                                        local_address.set_ip(self.ipv4_tun_addr);
                                    }
                                    config::component::network::host::Loopback::Host => {}
                                    config::component::network::host::Loopback::Composition {
                                        ..
                                    } => {
                                        error!("TODO: implement");
                                        return Ok(Err(ErrorCode::AddressNotBindable));
                                    }
                                }
                            } else if let Some(address) = address {
                                if ip.is_unspecified() {
                                    local_address.set_ip(*address);
                                } else if ip != address {
                                    return Ok(Err(ErrorCode::AddressNotBindable));
                                }
                            }
                            match ports {
                                config::component::network::Ports::Direct => {}
                                config::component::network::Ports::Dynamic => {
                                    local_address.set_port(0);
                                }
                            }
                        }
                    },
                    SocketAddr::V6(local_address) => {
                        error!("TODO: implement");
                        return Ok(Err(ErrorCode::AddressNotBindable));
                    }
                }

                // Automatically bypass the TIME_WAIT state when binding to a specific port
                // Unconditionally (re)set SO_REUSEADDR, even when the value is false.
                // This ensures we're not accidentally affected by any socket option
                // state left behind by a previous failed call to this method (start_bind).
                #[cfg(not(windows))]
                if let Err(err) = socket.set_reuseaddr(local_address.port() > 0) {
                    return Ok(Err(err.into()));
                }
                debug!(?local_address, "binding socket");
                if let Err(err) = socket.bind(local_address) {
                    match Errno::from_io_error(&err) {
                        // From https://pubs.opengroup.org/onlinepubs/9699919799/functions/bind.html:
                        // > [EAFNOSUPPORT] The specified address is not a valid address for the address family of the specified socket
                        //
                        // The most common reasons for this error should have already
                        // been handled by our own validation slightly higher up in this
                        // function. This error mapping is here just in case there is
                        // an edge case we didn't catch.
                        Some(Errno::AFNOSUPPORT) => return Ok(Err(ErrorCode::InvalidArgument)),
                        // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-bind#:~:text=WSAENOBUFS
                        // Windows returns WSAENOBUFS when the ephemeral ports have been exhausted.
                        #[cfg(windows)]
                        Some(Errno::NOBUFS) => return Ok(Err(ErrorCode::AddressInUse)),
                        _ => return Ok(Err(err.into())),
                    }
                }
                let addr = socket
                    .local_addr()
                    .context("failed to get local socket address")?;
                eprintln!("bind {addr} (host) -> {local_address} (component)");
                sock.state = State::BindStarted(socket);
                Ok(Ok(()))
            }
            state @ State::BindStarted(..) => {
                sock.state = state;
                Ok(Err(ErrorCode::ConcurrencyConflict))
            }
            state => {
                sock.state = state;
                Ok(Err(ErrorCode::InvalidState))
            }
        }
    }

    async fn finish_bind(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        match std::mem::replace(&mut sock.state, State::Closed) {
            State::BindStarted(socket) => {
                sock.state = State::Bound(socket);
                Ok(Ok(()))
            }
            state => {
                sock.state = state;
                Ok(Err(ErrorCode::NotInProgress))
            }
        }
    }

    async fn start_connect(
        &mut self,
        sock: Resource<Socket>,
        network: Resource<Network>,
        remote_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let remote_address: SocketAddr = remote_address.into();
        if remote_address.ip().to_canonical().is_unspecified() || remote_address.port() == 0 {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        // TODO: Validate address
        //
        //network::util::validate_unicast(&remote_address)?;
        //network::util::validate_address_family(&remote_address, &self.family)?;

        let network = self
            .table
            .get(&network)
            .context("failed to get network from table")?;
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;

        match mem::replace(&mut sock.state, State::Closed) {
            State::Default(socket) => {
                match remote_address {
                    SocketAddr::V4(mut remote_address) => match &self.network.tcp.ipv4 {
                        config::component::network::Network::None { loopback } => {
                            let ip = remote_address.ip();
                            if !ip.is_loopback() {
                                return Ok(Err(ErrorCode::ConnectionRefused));
                            }
                            match loopback {
                                config::component::network::none::Loopback::None => {
                                    return Ok(Err(ErrorCode::ConnectionRefused))
                                }
                                config::component::network::none::Loopback::Tun => {
                                    let addr = self.ipv4_tun_addr;
                                    remote_address.set_ip(addr);
                                    sock.state = State::Connecting(Box::pin(async move {
                                        socket.bind(SocketAddr::V4(SocketAddrV4::new(addr, 0)))?;
                                        socket.connect(SocketAddr::V4(remote_address)).await
                                    }));
                                    return Ok(Ok(()));
                                }
                                config::component::network::none::Loopback::Composition {
                                    ..
                                } => {
                                    error!("TODO: implement");
                                    return Ok(Err(ErrorCode::ConnectionRefused));
                                }
                            }
                        }
                        config::component::network::Network::Host(
                            config::component::network::host::Config {
                                address,
                                ports,
                                loopback,
                            },
                        ) => {
                            let ip = remote_address.ip();
                            if ip.is_loopback() {
                                match loopback {
                                    config::component::network::host::Loopback::None => {
                                        return Ok(Err(ErrorCode::ConnectionRefused))
                                    }
                                    config::component::network::host::Loopback::Tun => {
                                        let addr = self.ipv4_tun_addr;
                                        remote_address.set_ip(addr);
                                        sock.state = State::Connecting(Box::pin(async move {
                                            socket
                                                .bind(SocketAddr::V4(SocketAddrV4::new(addr, 0)))?;
                                            socket.connect(SocketAddr::V4(remote_address)).await
                                        }));
                                        return Ok(Ok(()));
                                    }
                                    config::component::network::host::Loopback::Host => {}
                                    config::component::network::host::Loopback::Composition {
                                        ..
                                    } => {
                                        error!("TODO: implement");
                                        return Ok(Err(ErrorCode::ConnectionRefused));
                                    }
                                }
                            } else if let Some(address) = address {
                                let address = *address;
                                sock.state = State::Connecting(Box::pin(async move {
                                    socket.bind(SocketAddr::V4(SocketAddrV4::new(address, 0)))?;
                                    socket.connect(SocketAddr::V4(remote_address)).await
                                }));
                                return Ok(Ok(()));
                            }
                        }
                    },
                    SocketAddr::V6(local_address) => {
                        error!("TODO: implement");
                        return Ok(Err(ErrorCode::ConnectionRefused));
                    }
                }
                sock.state = State::Connecting(Box::pin(socket.connect(remote_address)));
                Ok(Ok(()))
            }
            State::Bound(socket) => {
                sock.state = State::Connecting(Box::pin(socket.connect(remote_address)));
                Ok(Ok(()))
            }
            state @ State::Connecting(..) | state @ State::ConnectReady(..) => {
                sock.state = state;
                Ok(Err(ErrorCode::ConcurrencyConflict))
            }
            state => {
                sock.state = state;
                Ok(Err(ErrorCode::InvalidState))
            }
        }
    }

    async fn finish_connect(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<(Resource<InputStream>, Resource<OutputStream>), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn start_listen(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        match mem::replace(&mut sock.state, State::Closed) {
            State::Bound(socket) => {
                sock.state = State::ListenStarted(socket);
                Ok(Ok(()))
            }
            state @ State::ListenStarted(..) => {
                sock.state = state;
                Ok(Err(ErrorCode::ConcurrencyConflict))
            }
            state => {
                sock.state = state;
                Ok(Err(ErrorCode::InvalidState))
            }
        }
    }

    async fn finish_listen(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        match mem::replace(&mut sock.state, State::Closed) {
            State::ListenStarted(socket) => {
                match socket.listen(sock.listen_backlog_size) {
                    Ok(socket) => {
                        sock.state = State::Listening {
                            listener: socket,
                            pending_accept: None,
                        };
                        Ok(Ok(()))
                    }
                    Err(err) => {
                        sock.state = State::Closed;
                        match Errno::from_io_error(&err) {
                            // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-listen#:~:text=WSAEMFILE
                            // According to the docs, `listen` can return EMFILE on Windows.
                            // This is odd, because we're not trying to create a new socket
                            // or file descriptor of any kind. So we rewrite it to less
                            // surprising error code.
                            //
                            // At the time of writing, this behavior has never been experimentally
                            // observed by any of the wasmtime authors, so we're relying fully
                            // on Microsoft's documentation here.
                            #[cfg(windows)]
                            Some(Errno::MFILE) => Ok(Err(ErrorCode::OutOfMemory)),

                            _ => Ok(Err(err.into())),
                        }
                    }
                }
            }
            state => {
                sock.state = state;
                Ok(Err(ErrorCode::NotInProgress))
            }
        }
    }

    async fn accept(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<
        Result<
            (
                Resource<Socket>,
                Resource<InputStream>,
                Resource<OutputStream>,
            ),
            ErrorCode,
        >,
    > {
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        let State::Listening {
            ref listener,
            ref mut pending_accept,
        } = sock.state
        else {
            return Err(ErrorCode::InvalidState.into());
        };
        let stream = match match pending_accept.take() {
            Some(result) => result,
            None => {
                let mut cx = Context::from_waker(noop_waker_ref());
                match listener.poll_accept(&mut cx).map_ok(|(stream, _)| stream) {
                    Poll::Ready(result) => result,
                    Poll::Pending => Err(Errno::WOULDBLOCK.into()),
                }
            }
        } {
            Ok(stream) => stream,
            Err(err) => match Errno::from_io_error(&err) {
                // From: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-accept#:~:text=WSAEINPROGRESS
                // > WSAEINPROGRESS: A blocking Windows Sockets 1.1 call is in progress,
                // > or the service provider is still processing a callback function.
                //
                // wasi-sockets doesn't have an equivalent to the EINPROGRESS error,
                // because in POSIX this error is only returned by a non-blocking
                // `connect` and wasi-sockets has a different solution for that.
                #[cfg(windows)]
                Some(Errno::INPROGRESS) => return Ok(Err(ErrorCode::WouldBlock)),

                // Normalize Linux' non-standard behavior.
                //
                // From https://man7.org/linux/man-pages/man2/accept.2.html:
                // > Linux accept() passes already-pending network errors on the
                // > new socket as an error code from accept(). This behavior
                // > differs from other BSD socket implementations. (...)
                #[cfg(target_os = "linux")]
                Some(
                    Errno::CONNRESET
                    | Errno::NETRESET
                    | Errno::HOSTUNREACH
                    | Errno::HOSTDOWN
                    | Errno::NETDOWN
                    | Errno::NETUNREACH
                    | Errno::PROTO
                    | Errno::NOPROTOOPT
                    | Errno::NONET
                    | Errno::OPNOTSUPP,
                ) => return Ok(Err(ErrorCode::ConnectionAborted)),
                _ => return Ok(Err(err.into())),
            },
        };

        // TODO: Port this block
        //#[cfg(target_os = "macos")]
        //{
        //    // Manually inherit socket options from listener. We only have to
        //    // do this on platforms that don't already do this automatically
        //    // and only if a specific value was explicitly set on the listener.

        //    if let Some(size) = self.receive_buffer_size {
        //        _ = network::util::set_socket_recv_buffer_size(&stream, size); // Ignore potential error.
        //    }

        //    if let Some(size) = self.send_buffer_size {
        //        _ = network::util::set_socket_send_buffer_size(&stream, size); // Ignore potential error.
        //    }

        //    // For some reason, IP_TTL is inherited, but IPV6_UNICAST_HOPS isn't.
        //    if let (SocketAddressFamily::Ipv6, Some(ttl)) = (self.family, self.hop_limit) {
        //        _ = network::util::set_ipv6_unicast_hops(&stream, ttl); // Ignore potential error.
        //    }

        //    if let Some(value) = self.keep_alive_idle_time {
        //        _ = network::util::set_tcp_keepidle(&stream, value); // Ignore potential error.
        //    }
        //}

        let stream = Arc::new(stream);

        let reader = Arc::new(Mutex::new(TcpReader::new(Arc::clone(&stream))));
        let writer = Arc::new(Mutex::new(TcpWriter::new(Arc::clone(&stream))));

        let family = sock.family;
        let sock = self.table.push(Socket::from_state(
            State::Connected {
                stream,
                reader: Arc::clone(&reader),
                writer: Arc::clone(&writer),
            },
            family,
        ))?;
        let input: Resource<InputStream> = self
            .table
            .push_child(Box::new(TcpReadStream(reader)) as InputStream, &sock)?;
        let output = self
            .table
            .push_child(Box::new(TcpWriteStream(writer)) as OutputStream, &sock)?;
        Ok(Ok((sock, input, output)))
    }

    async fn local_address(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match sock.state {
            State::Bound(ref socket) => match socket.local_addr() {
                Ok(addr) => Ok(Ok(addr.into())),
                Err(err) => Ok(Err(err.into())),
            },
            State::Connected { ref stream, .. } => match stream.local_addr() {
                Ok(addr) => Ok(Ok(addr.into())),
                Err(err) => Ok(Err(err.into())),
            },
            State::Listening { ref listener, .. } => match listener.local_addr() {
                Ok(addr) => Ok(Ok(addr.into())),
                Err(err) => Ok(Err(err.into())),
            },
            State::BindStarted(..) => Ok(Err(ErrorCode::ConcurrencyConflict)),
            _ => Ok(Err(ErrorCode::InvalidState)),
        }
    }

    async fn remote_address(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match sock.state {
            State::Connected { ref stream, .. } => match stream.peer_addr() {
                Ok(addr) => Ok(Ok(addr.into())),
                Err(err) => Ok(Err(err.into())),
            },
            State::Connecting(..) | State::ConnectReady(..) => {
                Ok(Err(ErrorCode::ConcurrencyConflict))
            }
            _ => Ok(Err(ErrorCode::InvalidState)),
        }
    }

    async fn is_listening(&mut self, sock: Resource<Socket>) -> wasmtime::Result<bool> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        Ok(matches!(sock.state, State::Listening { .. }))
    }

    async fn address_family(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match sock.family {
            SocketAddressFamily::Ipv4 => Ok(IpAddressFamily::Ipv4),
            SocketAddressFamily::Ipv6 => Ok(IpAddressFamily::Ipv6),
        }
    }

    async fn set_listen_backlog_size(
        &mut self,
        sock: Resource<Socket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        const MAX_BACKLOG: u32 = i32::MAX as u32; // OS'es will most likely limit it down even further.

        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        // Silently clamp backlog size. This is OK for us to do, because operating systems do this too.
        if value == 0 {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        let value = value.try_into().unwrap_or(MAX_BACKLOG).min(MAX_BACKLOG);
        match &sock.state {
            State::Default(..) | State::Bound(..) => {
                // Socket not listening yet. Stash value for first invocation to `listen`.
                sock.listen_backlog_size = value;
                Ok(Ok(()))
            }
            State::Listening { listener, .. } => {
                // Try to update the backlog by calling `listen` again.
                // Not all platforms support this. We'll only update our own value if the OS supports changing the backlog size after the fact.
                if rustix::net::listen(&listener, value.try_into().unwrap_or(i32::MAX)).is_err() {
                    return Ok(Err(ErrorCode::NotSupported));
                }
                sock.listen_backlog_size = value;
                Ok(Ok(()))
            }
            _ => Ok(Err(ErrorCode::InvalidState.into())),
        }
    }

    async fn keep_alive_enabled(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<bool, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match &sock.state {
            State::Default(socket) | State::Bound(socket) => {
                Ok(sockopt::get_socket_keepalive(socket).map_err(Into::into))
            }
            State::Connected { stream, .. } => {
                Ok(sockopt::get_socket_keepalive(stream).map_err(Into::into))
            }
            State::Listening { listener, .. } => {
                Ok(sockopt::get_socket_keepalive(listener).map_err(Into::into))
            }
            State::BindStarted(..)
            | State::ListenStarted(..)
            | State::Connecting(..)
            | State::ConnectReady(..)
            | State::Closed => Ok(Err(ErrorCode::InvalidState)),
        }
    }

    async fn set_keep_alive_enabled(
        &mut self,
        sock: Resource<Socket>,
        value: bool,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match &sock.state {
            State::Default(socket) | State::Bound(socket) => {
                Ok(sockopt::set_socket_keepalive(socket, value).map_err(Into::into))
            }
            State::Connected { stream, .. } => {
                Ok(sockopt::set_socket_keepalive(stream, value).map_err(Into::into))
            }
            State::Listening { listener, .. } => {
                Ok(sockopt::set_socket_keepalive(listener, value).map_err(Into::into))
            }
            State::BindStarted(..)
            | State::ListenStarted(..)
            | State::Connecting(..)
            | State::ConnectReady(..)
            | State::Closed => Ok(Err(ErrorCode::InvalidState)),
        }
    }

    async fn keep_alive_idle_time(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<Duration, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match match &sock.state {
            State::Default(socket) | State::Bound(socket) => sockopt::get_tcp_keepidle(socket),
            State::Connected { stream, .. } => sockopt::get_tcp_keepidle(stream),
            State::Listening { listener, .. } => sockopt::get_tcp_keepidle(listener),
            State::BindStarted(..)
            | State::ListenStarted(..)
            | State::Connecting(..)
            | State::ConnectReady(..)
            | State::Closed => return Ok(Err(ErrorCode::InvalidState)),
        } {
            Ok(t) => Ok(Ok(t.as_nanos().try_into().unwrap_or(u64::MAX))),
            Err(err) => Ok(Err(err.into())),
        }
    }

    async fn set_keep_alive_idle_time(
        &mut self,
        sock: Resource<Socket>,
        value: Duration,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        // Ensure that the value passed to the actual syscall never gets rounded down to 0.
        const MIN_SECS: core::time::Duration = core::time::Duration::from_secs(1);

        // Cap it at Linux' maximum, which appears to have the lowest limit across our supported platforms.
        const MAX_SECS: core::time::Duration = core::time::Duration::from_secs(i16::MAX as u64);

        if value == 0 {
            // WIT: "If the provided value is 0, an `invalid-argument` error is returned."
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        let value = core::time::Duration::from_nanos(value).clamp(MIN_SECS, MAX_SECS);
        let sock = self
            .table
            .get_mut(&sock)
            .context("failed to get socket from table")?;
        match match &sock.state {
            State::Default(socket) | State::Bound(socket) => {
                sockopt::set_tcp_keepidle(socket, value)
            }
            State::Connected { stream, .. } => sockopt::set_tcp_keepidle(stream, value),
            State::Listening { listener, .. } => sockopt::set_tcp_keepidle(listener, value),
            _ => return Ok(Err(ErrorCode::InvalidState)),
        } {
            Ok(()) => {
                #[cfg(target_os = "macos")]
                {
                    sock.keep_alive_idle_time = Some(value);
                }
                Ok(Ok(()))
            }
            Err(err) => Ok(Err(err.into())),
        }
    }

    async fn keep_alive_interval(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<Duration, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match match &sock.state {
            State::Default(socket) | State::Bound(socket) => sockopt::get_tcp_keepintvl(socket),
            State::Connected { stream, .. } => sockopt::get_tcp_keepintvl(stream),
            State::Listening { listener, .. } => sockopt::get_tcp_keepintvl(listener),
            State::BindStarted(..)
            | State::ListenStarted(..)
            | State::Connecting(..)
            | State::ConnectReady(..)
            | State::Closed => return Ok(Err(ErrorCode::InvalidState)),
        } {
            Ok(t) => Ok(Ok(t.as_nanos().try_into().unwrap_or(u64::MAX))),
            Err(err) => Ok(Err(err.into())),
        }
    }

    async fn set_keep_alive_interval(
        &mut self,
        sock: Resource<Socket>,
        value: Duration,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn keep_alive_count(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<u32, ErrorCode>> {
        let sock = self
            .table
            .get(&sock)
            .context("failed to get socket from table")?;
        match &sock.state {
            State::Default(socket) | State::Bound(socket) => {
                Ok(sockopt::get_tcp_keepcnt(socket).map_err(Into::into))
            }
            State::Connected { stream, .. } => {
                Ok(sockopt::get_tcp_keepcnt(stream).map_err(Into::into))
            }
            State::Listening { listener, .. } => {
                Ok(sockopt::get_tcp_keepcnt(listener).map_err(Into::into))
            }
            State::BindStarted(..)
            | State::ListenStarted(..)
            | State::Connecting(..)
            | State::ConnectReady(..)
            | State::Closed => Ok(Err(ErrorCode::InvalidState)),
        }
    }

    async fn set_keep_alive_count(
        &mut self,
        sock: Resource<Socket>,
        value: u32,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn hop_limit(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<u8, ErrorCode>> {
        bail!("not supported yet")
    }

    async fn set_hop_limit(
        &mut self,
        sock: Resource<Socket>,
        value: u8,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn receive_buffer_size(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        bail!("not supported yet")
    }

    async fn set_receive_buffer_size(
        &mut self,
        sock: Resource<Socket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn send_buffer_size(
        &mut self,
        sock: Resource<Socket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        bail!("not supported yet")
    }

    async fn set_send_buffer_size(
        &mut self,
        sock: Resource<Socket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn subscribe(&mut self, sock: Resource<Socket>) -> wasmtime::Result<Resource<Pollable>> {
        subscribe(&mut self.table, sock)
    }

    async fn shutdown(
        &mut self,
        sock: Resource<Socket>,
        shutdown_type: ShutdownType,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        bail!("not supported yet")
    }

    async fn drop(&mut self, rep: Resource<Socket>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete socket from table")?;
        Ok(())
    }
}
