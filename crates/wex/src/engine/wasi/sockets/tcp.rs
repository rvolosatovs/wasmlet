use core::future::Future;
use core::mem;
use core::net::SocketAddr;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};
use core::{fmt::Debug, task::ready};

use std::net::Shutdown;
use std::sync::{Arc, RwLockReadGuard};

use cap_net_ext::AddressFamily;
use io_lifetimes::views::SocketlikeView;
use io_lifetimes::AsSocketlike as _;
use rustix::io::Errno;
use rustix::net::sockopt;
use tokio::sync::{Mutex, RwLock};
use wasmtime_wasi::runtime::with_ambient_tokio_runtime;

use crate::engine::bindings::wasi::clocks::monotonic_clock::Duration;
use crate::engine::bindings::wasi::sockets::network::{
    ErrorCode, IpAddressFamily, IpSocketAddress,
};
use crate::engine::wasi::io::{InputStream, OutputStream, Pollable};
use crate::engine::wasi::sockets::util::{
    get_unicast_hop_limit, is_valid_address_family, is_valid_unicast_address, receive_buffer_size,
    send_buffer_size, set_receive_buffer_size, set_send_buffer_size, set_unicast_hop_limit,
};
use crate::engine::wasi::sockets::SocketAddressFamily;
use crate::NOOP_WAKER;

use super::util::is_valid_remote_address;

/// Value taken from rust std library.
const DEFAULT_BACKLOG: u32 = 128;

/// The state of a TCP socket.
///
/// This represents the various states a socket can be in during the
/// activities of binding, listening, accepting, and connecting.
pub enum TcpState {
    /// The initial state for a newly-created socket.
    Default(tokio::net::TcpSocket),

    /// Binding finished. The socket has an address but is not yet listening for connections.
    BindStarted(tokio::net::TcpSocket),

    /// Binding finished. The socket has an address but is not yet listening for connections.
    Bound(tokio::net::TcpSocket),

    /// The socket is now listening and waiting for an incoming connection.
    ListenStarted(tokio::net::TcpListener),

    /// The socket is now listening and waiting for an incoming connection.
    Listening {
        listener: tokio::net::TcpListener,
        pending_accept: Option<std::io::Result<(tokio::net::TcpStream, SocketAddr)>>,
    },

    /// An outgoing connection is started.
    ConnectStarted(
        Pin<Box<dyn Future<Output = std::io::Result<tokio::net::TcpStream>> + Send + Sync>>,
    ),

    /// An outgoing connection is ready.
    ConnectReady(std::io::Result<tokio::net::TcpStream>),

    /// A connection has been established.
    Connected(Arc<tokio::net::TcpStream>),

    Closed,
}

impl TcpState {
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self {
            Self::Default(..)
            | Self::BindStarted(..)
            | Self::Bound(..)
            | Self::ListenStarted(..)
            | Self::Listening {
                pending_accept: Some(..),
                ..
            }
            | Self::ConnectReady(..)
            | Self::Connected(..)
            | Self::Closed => Poll::Ready(()),
            Self::Listening {
                listener,
                pending_accept,
            } => {
                let res = ready!(listener.poll_accept(cx));
                *pending_accept = Some(res);
                Poll::Ready(())
            }
            Self::ConnectStarted(fut) => {
                let res = ready!(fut.as_mut().poll(cx));
                *self = Self::ConnectReady(res);
                Poll::Ready(())
            }
        }
    }

    pub fn as_std_view(&self) -> Result<SocketlikeView<'_, std::net::TcpStream>, ErrorCode> {
        match &self {
            Self::Default(socket) | Self::BindStarted(socket) | Self::Bound(socket) => {
                Ok(socket.as_socketlike_view())
            }
            Self::Connected(stream) => Ok(stream.as_socketlike_view()),
            Self::ListenStarted(listener) | Self::Listening { listener, .. } => {
                Ok(listener.as_socketlike_view())
            }
            Self::ConnectStarted { .. } | Self::ConnectReady(..) | Self::Closed => {
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn start_bind(
        &mut self,
        family: SocketAddressFamily,
        addr: impl Into<SocketAddr>,
    ) -> Result<(), ErrorCode> {
        let addr = addr.into();
        let ip = addr.ip();
        if !is_valid_unicast_address(ip) || !is_valid_address_family(ip, family) {
            return Err(ErrorCode::InvalidArgument);
        }
        match mem::replace(self, Self::Closed) {
            Self::Default(sock) => {
                if let Err(err) = bind(&sock, addr) {
                    *self = Self::Default(sock);
                    Err(err)
                } else {
                    *self = Self::BindStarted(sock);
                    Ok(())
                }
            }
            state => {
                *self = state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match mem::replace(self, Self::Closed) {
            Self::BindStarted(sock) => {
                *self = Self::Bound(sock);
                Ok(())
            }
            state => {
                *self = state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    pub fn start_connect(
        &mut self,
        family: SocketAddressFamily,
        addr: impl Into<SocketAddr>,
    ) -> Result<(), ErrorCode> {
        let addr = addr.into();
        let ip = addr.ip();
        if !is_valid_unicast_address(ip)
            || !is_valid_remote_address(addr)
            || !is_valid_address_family(ip, family)
        {
            return Err(ErrorCode::InvalidArgument);
        }
        match mem::replace(self, Self::Closed) {
            Self::Default(sock) | Self::BindStarted(sock) | Self::Bound(sock) => {
                *self = Self::ConnectStarted(Box::pin(sock.connect(addr)));
                Ok(())
            }
            state => {
                *self = state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn finish_connect(&mut self) -> Result<(InputStream, OutputStream), ErrorCode> {
        let prev = mem::replace(self, Self::Closed);
        let stream = match prev {
            Self::ConnectStarted(mut fut) => {
                match fut.as_mut().poll(&mut Context::from_waker(NOOP_WAKER)) {
                    Poll::Ready(Ok(stream)) => stream,
                    Poll::Ready(Err(err)) => return Err(err.into()),
                    Poll::Pending => {
                        *self = Self::ConnectStarted(fut);
                        return Err(ErrorCode::WouldBlock);
                    }
                }
            }
            Self::ConnectReady(Ok(stream)) => stream,
            Self::ConnectReady(Err(err)) => return Err(err.into()),
            _ => {
                *self = prev;
                return Err(ErrorCode::NotInProgress);
            }
        };
        let stream = Arc::new(stream);
        let rx = InputStream::TcpStream(Arc::clone(&stream));
        let tx = OutputStream::TcpStream(Arc::clone(&stream));
        *self = Self::Connected(stream);
        Ok((rx, tx))
    }

    pub fn start_listen(&mut self, backlog: u32) -> Result<(), ErrorCode> {
        match mem::replace(self, Self::Closed) {
            Self::BindStarted(socket) | Self::Bound(socket) => {
                let listener = socket.listen(backlog).map_err(|err| {
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
                        Some(Errno::MFILE) => ErrorCode::OutOfMemory,
                        _ => ErrorCode::from(err),
                    }
                })?;
                *self = Self::ListenStarted(listener);
                Ok(())
            }
            Self::ListenStarted(listener) => {
                *self = Self::ListenStarted(listener);
                Err(ErrorCode::ConcurrencyConflict)
            }
            state => {
                *self = state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn finish_listen(&mut self) -> Result<(), ErrorCode> {
        match mem::replace(self, TcpState::Closed) {
            Self::ListenStarted(listener) => {
                *self = Self::Listening {
                    listener,
                    pending_accept: None,
                };
                Ok(())
            }
            state => {
                *self = state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    pub fn accept(&mut self) -> Result<(tokio::net::TcpStream, SocketAddr), ErrorCode> {
        let Self::Listening {
            listener,
            pending_accept,
        } = self
        else {
            return Err(ErrorCode::InvalidState);
        };
        let res = if let Some(res) = pending_accept.take() {
            res
        } else if let Poll::Ready(res) = listener.poll_accept(&mut Context::from_waker(NOOP_WAKER))
        {
            res
        } else {
            return Err(ErrorCode::WouldBlock);
        };
        res.map_err(|err| match Errno::from_io_error(&err) {
            // From: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-accept#:~:text=WSAEINPROGRESS
            // > WSAEINPROGRESS: A blocking Windows Sockets 1.1 call is in progress,
            // > or the service provider is still processing a callback function.
            //
            // wasi-sockets doesn't have an equivalent to the EINPROGRESS error,
            // because in POSIX this error is only returned by a non-blocking
            // `connect` and wasi-sockets has a different solution for that.
            #[cfg(windows)]
            Some(Errno::INPROGRESS) => ErrorCode::Unknown,

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
            ) => ErrorCode::ConnectionAborted,
            _ => err.into(),
        })
    }

    pub fn shutdown(&self, how: Shutdown) -> Result<(), ErrorCode> {
        let Self::Connected(stream) = self else {
            return Err(ErrorCode::InvalidState);
        };
        let socket = stream.as_socketlike_view::<std::net::TcpStream>();
        socket.shutdown(how);
        Ok(())
    }

    pub fn local_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        match self {
            Self::BindStarted(socket) | Self::Bound(socket) => {
                let addr = socket.local_addr()?;
                Ok(addr.into())
            }
            Self::Connected(stream) => {
                let addr = stream.local_addr()?;
                Ok(addr.into())
            }
            Self::ListenStarted(listener) | Self::Listening { listener, .. } => {
                let addr = listener.local_addr()?;
                Ok(addr.into())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn remote_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        match self {
            Self::Connected(stream) => {
                let addr = stream.peer_addr()?;
                Ok(addr.into())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn is_listening(&self) -> bool {
        matches!(self, Self::Listening { .. })
    }
}

impl Debug for TcpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default(_) => f.debug_tuple("Default").finish(),
            Self::BindStarted(_) => f.debug_tuple("BindStarted").finish(),
            Self::Bound(_) => f.debug_tuple("Bound").finish(),
            Self::ListenStarted { .. } => f.debug_tuple("ListenStarted").finish(),
            Self::Listening { .. } => f.debug_tuple("Listening").finish(),
            Self::ConnectStarted { .. } => f.debug_tuple("ConnectStarted").finish(),
            Self::ConnectReady { .. } => f.debug_tuple("ConnectReady").finish(),
            Self::Connected { .. } => f.debug_tuple("Connected").finish(),
            Self::Closed => write!(f, "Closed"),
        }
    }
}

/// A host TCP socket, plus associated bookkeeping.
pub struct TcpSocket {
    /// The current state in the bind/listen/accept/connect progression.
    pub tcp_state: Arc<std::sync::RwLock<TcpState>>,

    /// The desired listen queue size.
    pub listen_backlog_size: u32,

    pub family: SocketAddressFamily,

    // The socket options below are not automatically inherited from the listener
    // on all platforms. So we keep track of which options have been explicitly
    // set and manually apply those values to newly accepted clients.
    #[cfg(target_os = "macos")]
    pub receive_buffer_size: Arc<core::sync::atomic::AtomicUsize>,
    #[cfg(target_os = "macos")]
    pub send_buffer_size: Arc<core::sync::atomic::AtomicUsize>,
    #[cfg(target_os = "macos")]
    pub hop_limit: Arc<core::sync::atomic::AtomicU8>,
    #[cfg(target_os = "macos")]
    pub keep_alive_idle_time: Arc<core::sync::atomic::AtomicU64>, // nanoseconds
}

impl TcpSocket {
    /// Create a new socket in the given family.
    pub fn new(family: AddressFamily) -> std::io::Result<Self> {
        with_ambient_tokio_runtime(|| {
            let (socket, family) = match family {
                AddressFamily::Ipv4 => {
                    let socket = tokio::net::TcpSocket::new_v4()?;
                    (socket, SocketAddressFamily::Ipv4)
                }
                AddressFamily::Ipv6 => {
                    let socket = tokio::net::TcpSocket::new_v6()?;
                    sockopt::set_ipv6_v6only(&socket, true)?;
                    (socket, SocketAddressFamily::Ipv6)
                }
            };

            Ok(Self::from_state(TcpState::Default(socket), family))
        })
    }

    /// Create a `TcpSocket` from an existing socket.
    pub fn from_state(state: TcpState, family: SocketAddressFamily) -> Self {
        Self {
            tcp_state: Arc::new(std::sync::RwLock::new(state)),
            listen_backlog_size: DEFAULT_BACKLOG,
            family,
            #[cfg(target_os = "macos")]
            receive_buffer_size: Arc::default(),
            #[cfg(target_os = "macos")]
            send_buffer_size: Arc::default(),
            #[cfg(target_os = "macos")]
            hop_limit: Arc::default(),
            #[cfg(target_os = "macos")]
            keep_alive_idle_time: Arc::default(),
        }
    }

    pub fn start_bind(&self, addr: impl Into<SocketAddr>) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.start_bind(self.family, addr)
    }

    pub fn finish_bind(&self) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.finish_bind()
    }

    pub fn start_connect(&self, addr: impl Into<SocketAddr>) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.start_connect(self.family, addr)
    }

    pub fn finish_connect(&self) -> Result<(InputStream, OutputStream), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.finish_connect()
    }

    pub fn start_listen(&self) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.start_listen(self.listen_backlog_size)
    }

    pub fn finish_listen(&self) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        state.finish_listen()
    }

    pub fn accept(&mut self) -> Result<(TcpSocket, InputStream, OutputStream), ErrorCode> {
        let Ok(mut state) = self.tcp_state.write() else {
            return Err(ErrorCode::Unknown);
        };
        let (stream, _addr) = state.accept()?;
        #[cfg(target_os = "macos")]
        {
            // Manually inherit socket options from listener. We only have to
            // do this on platforms that don't already do this automatically
            // and only if a specific value was explicitly set on the listener.

            let receive_buffer_size = self
                .receive_buffer_size
                .load(core::sync::atomic::Ordering::Relaxed);
            if receive_buffer_size > 0 {
                // Ignore potential error.
                _ = rustix::net::sockopt::set_socket_recv_buffer_size(&stream, receive_buffer_size);
            }

            let send_buffer_size = self
                .send_buffer_size
                .load(core::sync::atomic::Ordering::Relaxed);
            if send_buffer_size > 0 {
                // Ignore potential error.
                _ = rustix::net::sockopt::set_socket_send_buffer_size(&stream, send_buffer_size);
            }

            // For some reason, IP_TTL is inherited, but IPV6_UNICAST_HOPS isn't.
            if self.family == SocketAddressFamily::Ipv6 {
                let hop_limit = self.hop_limit.load(core::sync::atomic::Ordering::Relaxed);
                if hop_limit > 0 {
                    // Ignore potential error.
                    _ = rustix::net::sockopt::set_ipv6_unicast_hops(&stream, Some(hop_limit));
                }
            }

            let keep_alive_idle_time = self
                .keep_alive_idle_time
                .load(core::sync::atomic::Ordering::Relaxed);
            if keep_alive_idle_time > 0 {
                // Ignore potential error.
                _ = rustix::net::sockopt::set_tcp_keepidle(
                    &stream,
                    core::time::Duration::from_nanos(keep_alive_idle_time),
                );
            }
        }
        let stream = Arc::new(stream);
        let rx = InputStream::TcpStream(Arc::clone(&stream));
        let tx = OutputStream::TcpStream(Arc::clone(&stream));
        let sock = Self::from_state(TcpState::Connected(stream), self.family);
        Ok((sock, rx, tx))
    }

    pub fn shutdown(&self, how: Shutdown) -> Result<(), ErrorCode> {
        let Ok(mut state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        state.shutdown(how)
    }

    pub fn local_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        let Ok(mut state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        state.local_address()
    }

    pub fn remote_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        let Ok(mut state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        state.remote_address()
    }

    pub fn is_listening(&self) -> bool {
        let Ok(mut state) = self.tcp_state.read() else {
            return false;
        };
        state.is_listening()
    }

    pub fn address_family(&self) -> IpAddressFamily {
        match self.family {
            SocketAddressFamily::Ipv4 => IpAddressFamily::Ipv4,
            SocketAddressFamily::Ipv6 => IpAddressFamily::Ipv6,
        }
    }

    pub fn set_listen_backlog_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        const MIN_BACKLOG: u32 = 1;
        const MAX_BACKLOG: u32 = i32::MAX as u32; // OS'es will most likely limit it down even further.

        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        // Silently clamp backlog size. This is OK for us to do, because operating systems do this too.
        let value = value
            .try_into()
            .unwrap_or(MAX_BACKLOG)
            .clamp(MIN_BACKLOG, MAX_BACKLOG);
        match self.tcp_state.read().as_deref() {
            Ok(TcpState::Default(..) | TcpState::BindStarted(..) | TcpState::Bound(..)) => {
                // Socket not listening yet. Stash value for first invocation to `listen`.
                self.listen_backlog_size = value;
                Ok(())
            }
            Ok(TcpState::ListenStarted(listener) | TcpState::Listening { listener, .. }) => {
                // Try to update the backlog by calling `listen` again.
                // Not all platforms support this. We'll only update our own value if the OS supports changing the backlog size after the fact.
                if rustix::net::listen(&listener, value.try_into().unwrap_or(i32::MAX)).is_err() {
                    return Err(ErrorCode::NotSupported);
                }
                self.listen_backlog_size = value;
                Ok(())
            }
            Err(..) => Err(ErrorCode::Unknown),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn keep_alive_enabled(&self) -> Result<bool, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let v = sockopt::socket_keepalive(&*fd)?;
        Ok(v)
    }

    pub fn set_keep_alive_enabled(&self, value: bool) -> Result<(), ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        sockopt::set_socket_keepalive(&*fd, value)?;
        Ok(())
    }

    pub fn keep_alive_idle_time(&self) -> Result<Duration, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let v = sockopt::tcp_keepidle(&*fd)?;
        Ok(v.as_nanos().try_into().unwrap_or(u64::MAX))
    }

    pub fn set_keep_alive_idle_time(&mut self, value: Duration) -> Result<(), ErrorCode> {
        const NANOS_PER_SEC: u64 = 1_000_000_000;

        // Ensure that the value passed to the actual syscall never gets rounded down to 0.
        const MIN: u64 = NANOS_PER_SEC;

        // Cap it at Linux' maximum, which appears to have the lowest limit across our supported platforms.
        const MAX: u64 = (i16::MAX as u64) * NANOS_PER_SEC;

        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        if value == 0 {
            // WIT: "If the provided value is 0, an `invalid-argument` error is returned."
            return Err(ErrorCode::InvalidArgument);
        }
        let value = value.clamp(MIN, MAX);
        sockopt::set_tcp_keepidle(&*fd, core::time::Duration::from_nanos(value))?;
        #[cfg(target_os = "macos")]
        {
            self.keep_alive_idle_time
                .store(value, core::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn keep_alive_interval(&self) -> Result<Duration, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let v = sockopt::tcp_keepintvl(&*fd)?;
        Ok(v.as_nanos().try_into().unwrap_or(u64::MAX))
    }

    pub fn set_keep_alive_interval(&self, value: Duration) -> Result<(), ErrorCode> {
        // Ensure that any fractional value passed to the actual syscall never gets rounded down to 0.
        const MIN_SECS: core::time::Duration = core::time::Duration::from_secs(1);

        // Cap it at Linux' maximum, which appears to have the lowest limit across our supported platforms.
        const MAX_SECS: core::time::Duration = core::time::Duration::from_secs(i16::MAX as u64);

        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        if value == 0 {
            // WIT: "If the provided value is 0, an `invalid-argument` error is returned."
            return Err(ErrorCode::InvalidArgument);
        }
        sockopt::set_tcp_keepintvl(
            &*fd,
            core::time::Duration::from_nanos(value).clamp(MIN_SECS, MAX_SECS),
        )?;
        Ok(())
    }

    pub fn keep_alive_count(&self) -> Result<u32, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let v = sockopt::tcp_keepcnt(&*fd)?;
        Ok(v)
    }

    pub fn set_keep_alive_count(&self, value: u32) -> Result<(), ErrorCode> {
        const MIN_CNT: u32 = 1;
        // Cap it at Linux' maximum, which appears to have the lowest limit across our supported platforms.
        const MAX_CNT: u32 = i8::MAX as u32;

        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        if value == 0 {
            // WIT: "If the provided value is 0, an `invalid-argument` error is returned."
            return Err(ErrorCode::InvalidArgument);
        }
        sockopt::set_tcp_keepcnt(&*fd, value.clamp(MIN_CNT, MAX_CNT))?;
        Ok(())
    }

    pub fn hop_limit(&self) -> Result<u8, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        get_unicast_hop_limit(&*fd, self.family)
    }

    pub fn set_hop_limit(&self, value: u8) -> Result<(), ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        set_unicast_hop_limit(&*fd, self.family, value)?;
        #[cfg(target_os = "macos")]
        {
            self.hop_limit
                .store(value, core::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        receive_buffer_size(&*fd)
    }

    pub fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let res = set_receive_buffer_size(&*fd, value);
        #[cfg(target_os = "macos")]
        {
            let value = res?;
            self.receive_buffer_size
                .store(value, core::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(not(target_os = "macos"))]
        {
            res?;
        }
        Ok(())
    }

    pub fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        send_buffer_size(&*fd)
    }

    pub fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        let Ok(state) = self.tcp_state.read() else {
            return Err(ErrorCode::Unknown);
        };
        let fd = state.as_std_view()?;
        let res = set_send_buffer_size(&*fd, value);
        #[cfg(target_os = "macos")]
        {
            let value = res?;
            self.send_buffer_size
                .store(value, core::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(not(target_os = "macos"))]
        {
            res?;
        }
        Ok(())
    }
}

fn bind(socket: &tokio::net::TcpSocket, local_address: SocketAddr) -> Result<(), ErrorCode> {
    // Automatically bypass the TIME_WAIT state when binding to a specific port
    // Unconditionally (re)set SO_REUSEADDR, even when the value is false.
    // This ensures we're not accidentally affected by any socket option
    // state left behind by a previous failed call to this method.
    #[cfg(not(windows))]
    if let Err(err) = sockopt::set_socket_reuseaddr(&socket, local_address.port() > 0) {
        return Err(err.into());
    }

    // Perform the OS bind call.
    socket
        .bind(local_address)
        .map_err(|err| match Errno::from_io_error(&err) {
            // From https://pubs.opengroup.org/onlinepubs/9699919799/functions/bind.html:
            // > [EAFNOSUPPORT] The specified address is not a valid address for the address family of the specified socket
            //
            // The most common reasons for this error should have already
            // been handled by our own validation slightly higher up in this
            // function. This error mapping is here just in case there is
            // an edge case we didn't catch.
            Some(Errno::AFNOSUPPORT) => ErrorCode::InvalidArgument,
            // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-bind#:~:text=WSAENOBUFS
            // Windows returns WSAENOBUFS when the ephemeral ports have been exhausted.
            #[cfg(windows)]
            Some(Errno::NOBUFS) => ErrorCode::AddressInUse,
            _ => err.into(),
        })
}
