#![allow(unused)]

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::collections::HashMap;
use std::net::ToSocketAddrs;

use anyhow::Context as _;
use rustix::io::Errno;
use tracing::{debug, instrument};
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::clocks::monotonic_clock::Duration;
use wasmtime_wasi::{async_trait, subscribe, InputStream, OutputStream, Pollable, Subscribe};

use crate::bindings::wasi::sockets::network::{
    ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress, Ipv4Address, Ipv4SocketAddress,
    Ipv6Address, Ipv6SocketAddress,
};
use crate::bindings::wasi::sockets::tcp::ShutdownType;
use crate::bindings::wasi::sockets::udp::{IncomingDatagram, OutgoingDatagram};
use crate::bindings::wasi::sockets::{
    instance_network, ip_name_lookup, network, udp, udp_create_socket,
};
use crate::Ctx;

mod tcp;

pub use self::tcp::Socket as TcpSocket;

pub struct Network {}
pub struct ResolveAddressStream {}

pub struct UdpSocket {}
pub struct IncomingDatagramStream {}
pub struct OutgoingDatagramStream {}

impl From<std::io::Error> for ErrorCode {
    fn from(value: std::io::Error) -> Self {
        (&value).into()
    }
}

impl From<&std::io::Error> for ErrorCode {
    fn from(err: &std::io::Error) -> Self {
        // Attempt the more detailed native error code first:
        if let Some(errno) = Errno::from_io_error(err) {
            return errno.into();
        }
        match err.kind() {
            std::io::ErrorKind::AddrInUse => ErrorCode::AddressInUse,
            std::io::ErrorKind::AddrNotAvailable => ErrorCode::AddressNotBindable,
            std::io::ErrorKind::ConnectionAborted => ErrorCode::ConnectionAborted,
            std::io::ErrorKind::ConnectionRefused => ErrorCode::ConnectionRefused,
            std::io::ErrorKind::ConnectionReset => ErrorCode::ConnectionReset,
            std::io::ErrorKind::Interrupted => ErrorCode::WouldBlock,
            std::io::ErrorKind::InvalidInput => ErrorCode::InvalidArgument,
            std::io::ErrorKind::NotConnected => ErrorCode::InvalidState,
            std::io::ErrorKind::OutOfMemory => ErrorCode::OutOfMemory,
            std::io::ErrorKind::PermissionDenied => ErrorCode::AccessDenied,
            std::io::ErrorKind::TimedOut => ErrorCode::Timeout,
            std::io::ErrorKind::Unsupported => ErrorCode::NotSupported,
            std::io::ErrorKind::WouldBlock => ErrorCode::WouldBlock,
            _ => {
                debug!(?err, "unknown I/O error");
                ErrorCode::Unknown
            }
        }
    }
}

impl From<Errno> for ErrorCode {
    fn from(err: Errno) -> Self {
        (&err).into()
    }
}

impl From<&Errno> for ErrorCode {
    fn from(err: &Errno) -> Self {
        match *err {
            Errno::WOULDBLOCK => ErrorCode::WouldBlock,
            #[allow(unreachable_patterns)] // EWOULDBLOCK and EAGAIN can have the same value.
            Errno::AGAIN => ErrorCode::WouldBlock,
            Errno::INTR => ErrorCode::WouldBlock,
            #[cfg(not(windows))]
            Errno::PERM => ErrorCode::AccessDenied,
            Errno::ACCESS => ErrorCode::AccessDenied,
            Errno::ADDRINUSE => ErrorCode::AddressInUse,
            Errno::ADDRNOTAVAIL => ErrorCode::AddressNotBindable,
            Errno::ALREADY => ErrorCode::ConcurrencyConflict,
            Errno::TIMEDOUT => ErrorCode::Timeout,
            Errno::CONNREFUSED => ErrorCode::ConnectionRefused,
            Errno::CONNRESET => ErrorCode::ConnectionReset,
            Errno::CONNABORTED => ErrorCode::ConnectionAborted,
            Errno::INVAL => ErrorCode::InvalidArgument,
            Errno::HOSTUNREACH => ErrorCode::RemoteUnreachable,
            Errno::HOSTDOWN => ErrorCode::RemoteUnreachable,
            Errno::NETDOWN => ErrorCode::RemoteUnreachable,
            Errno::NETUNREACH => ErrorCode::RemoteUnreachable,
            #[cfg(target_os = "linux")]
            Errno::NONET => ErrorCode::RemoteUnreachable,
            Errno::ISCONN => ErrorCode::InvalidState,
            Errno::NOTCONN => ErrorCode::InvalidState,
            Errno::DESTADDRREQ => ErrorCode::InvalidState,
            #[cfg(not(windows))]
            Errno::NFILE => ErrorCode::NewSocketLimit,
            Errno::MFILE => ErrorCode::NewSocketLimit,
            Errno::MSGSIZE => ErrorCode::DatagramTooLarge,
            #[cfg(not(windows))]
            Errno::NOMEM => ErrorCode::OutOfMemory,
            Errno::NOBUFS => ErrorCode::OutOfMemory,
            Errno::OPNOTSUPP => ErrorCode::NotSupported,
            Errno::NOPROTOOPT => ErrorCode::NotSupported,
            Errno::PFNOSUPPORT => ErrorCode::NotSupported,
            Errno::PROTONOSUPPORT => ErrorCode::NotSupported,
            Errno::PROTOTYPE => ErrorCode::NotSupported,
            Errno::SOCKTNOSUPPORT => ErrorCode::NotSupported,
            Errno::AFNOSUPPORT => ErrorCode::NotSupported,

            // FYI, EINPROGRESS should have already been handled by connect.
            _ => ErrorCode::Unknown,
        }
    }
}

#[derive(Copy, Clone)]
pub enum SocketAddressFamily {
    Ipv4,
    Ipv6,
}

fn to_ipv4_addr(addr: Ipv4Address) -> Ipv4Addr {
    let (x0, x1, x2, x3) = addr;
    Ipv4Addr::new(x0, x1, x2, x3)
}

fn from_ipv4_addr(addr: Ipv4Addr) -> Ipv4Address {
    let [x0, x1, x2, x3] = addr.octets();
    (x0, x1, x2, x3)
}

fn to_ipv6_addr(addr: Ipv6Address) -> Ipv6Addr {
    let (x0, x1, x2, x3, x4, x5, x6, x7) = addr;
    Ipv6Addr::new(x0, x1, x2, x3, x4, x5, x6, x7)
}

fn from_ipv6_addr(addr: Ipv6Addr) -> Ipv6Address {
    let [x0, x1, x2, x3, x4, x5, x6, x7] = addr.segments();
    (x0, x1, x2, x3, x4, x5, x6, x7)
}

impl From<IpAddr> for IpAddress {
    fn from(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(v4) => Self::Ipv4(from_ipv4_addr(v4)),
            IpAddr::V6(v6) => Self::Ipv6(from_ipv6_addr(v6)),
        }
    }
}

impl From<IpSocketAddress> for SocketAddr {
    fn from(addr: IpSocketAddress) -> Self {
        match addr {
            IpSocketAddress::Ipv4(ipv4) => Self::V4(ipv4.into()),
            IpSocketAddress::Ipv6(ipv6) => Self::V6(ipv6.into()),
        }
    }
}

impl From<SocketAddr> for IpSocketAddress {
    fn from(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(v4) => Self::Ipv4(v4.into()),
            SocketAddr::V6(v6) => Self::Ipv6(v6.into()),
        }
    }
}

impl From<Ipv4SocketAddress> for SocketAddrV4 {
    fn from(addr: Ipv4SocketAddress) -> Self {
        Self::new(to_ipv4_addr(addr.address), addr.port)
    }
}

impl From<SocketAddrV4> for Ipv4SocketAddress {
    fn from(addr: SocketAddrV4) -> Self {
        Self {
            address: from_ipv4_addr(*addr.ip()),
            port: addr.port(),
        }
    }
}

impl From<Ipv6SocketAddress> for SocketAddrV6 {
    fn from(addr: Ipv6SocketAddress) -> Self {
        Self::new(
            to_ipv6_addr(addr.address),
            addr.port,
            addr.flow_info,
            addr.scope_id,
        )
    }
}

impl From<SocketAddrV6> for Ipv6SocketAddress {
    fn from(addr: SocketAddrV6) -> Self {
        Self {
            address: from_ipv6_addr(*addr.ip()),
            port: addr.port(),
            flow_info: addr.flowinfo(),
            scope_id: addr.scope_id(),
        }
    }
}

impl ToSocketAddrs for IpSocketAddress {
    type Iter = <SocketAddr as ToSocketAddrs>::Iter;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        SocketAddr::from(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for Ipv4SocketAddress {
    type Iter = <SocketAddrV4 as ToSocketAddrs>::Iter;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        SocketAddrV4::from(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for Ipv6SocketAddress {
    type Iter = <SocketAddrV6 as ToSocketAddrs>::Iter;

    fn to_socket_addrs(&self) -> std::io::Result<Self::Iter> {
        SocketAddrV6::from(*self).to_socket_addrs()
    }
}

#[async_trait]
impl network::Host for Ctx {
    async fn network_error_code(
        &mut self,
        err: Resource<wasmtime_wasi::bindings::io::error::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        todo!()
    }
}

#[async_trait]
impl network::HostNetwork for Ctx {
    async fn drop(&mut self, rep: Resource<Network>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete network from table")?;
        Ok(())
    }
}

#[async_trait]
impl instance_network::Host for Ctx {
    async fn instance_network(&mut self) -> wasmtime::Result<Resource<Network>> {
        self.table
            .push(Network {})
            .context("failed to push network to table")
    }
}

#[async_trait]
impl ip_name_lookup::Host for Ctx {
    async fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
    ) -> wasmtime::Result<Result<Resource<ResolveAddressStream>, ErrorCode>> {
        todo!()
    }
}

#[async_trait]
impl ip_name_lookup::HostResolveAddressStream for Ctx {
    async fn resolve_next_address(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> wasmtime::Result<Result<Option<IpAddress>, ErrorCode>> {
        todo!()
    }

    async fn subscribe(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    async fn drop(&mut self, rep: Resource<ResolveAddressStream>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete stream from table")?;
        Ok(())
    }
}

#[async_trait]
impl udp_create_socket::Host for Ctx {
    async fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> wasmtime::Result<Result<Resource<UdpSocket>, ErrorCode>> {
        todo!()
    }
}

#[async_trait]
impl udp::Host for Ctx {}

#[async_trait]
impl udp::HostUdpSocket for Ctx {
    async fn start_bind(
        &mut self,
        self_: Resource<UdpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    async fn finish_bind(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    async fn stream(
        &mut self,
        self_: Resource<UdpSocket>,
        remote_address: Option<IpSocketAddress>,
    ) -> wasmtime::Result<
        Result<
            (
                Resource<IncomingDatagramStream>,
                Resource<OutgoingDatagramStream>,
            ),
            ErrorCode,
        >,
    > {
        todo!()
    }

    async fn local_address(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        todo!()
    }

    async fn remote_address(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        todo!()
    }

    async fn address_family(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        todo!()
    }

    async fn unicast_hop_limit(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u8, ErrorCode>> {
        todo!()
    }

    async fn set_unicast_hop_limit(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u8,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    async fn receive_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    async fn set_receive_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    async fn send_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    async fn set_send_buffer_size(
        &mut self,
        self_: Resource<UdpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    async fn subscribe(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    async fn drop(&mut self, rep: Resource<UdpSocket>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete socket from table")?;
        Ok(())
    }
}

#[async_trait]
impl udp::HostIncomingDatagramStream for Ctx {
    async fn receive(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
        max_results: u64,
    ) -> wasmtime::Result<Result<Vec<IncomingDatagram>, ErrorCode>> {
        todo!()
    }

    async fn subscribe(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    async fn drop(&mut self, rep: Resource<IncomingDatagramStream>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete stream from table")?;
        Ok(())
    }
}

#[async_trait]
impl udp::HostOutgoingDatagramStream for Ctx {
    async fn check_send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    async fn send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
        datagrams: Vec<OutgoingDatagram>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    async fn subscribe(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    async fn drop(&mut self, rep: Resource<OutgoingDatagramStream>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete stream from table")?;
        Ok(())
    }
}
