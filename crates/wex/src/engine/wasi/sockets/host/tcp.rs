use core::future::Future as _;
use core::mem;
use core::net::SocketAddr;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use std::net::Shutdown;
use std::sync::{Arc, TryLockError};

use anyhow::{bail, ensure, Context as _};
use io_lifetimes::AsSocketlike as _;
use rustix::io::Errno;
use tokio::sync::{mpsc, Mutex};
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::bindings::wasi::clocks::monotonic_clock::Duration;
use crate::engine::bindings::wasi::sockets::network::{
    ErrorCode, IpAddressFamily, IpSocketAddress,
};
use crate::engine::bindings::wasi::sockets::tcp::{Host, HostTcpSocket, ShutdownType};
use crate::engine::bindings::wasi::sockets::tcp_create_socket;
use crate::engine::wasi::io::{push_pollable, InputStream, OutputStream, Pollable};
use crate::engine::wasi::sockets::tcp::{TcpSocket, TcpState};
use crate::engine::wasi::sockets::util::{
    is_valid_address_family, is_valid_remote_address, is_valid_unicast_address,
};
use crate::engine::wasi::sockets::{
    Network, SocketAddressFamily, WasiSocketsImpl, WasiSocketsView,
};
use crate::engine::ResourceView as _;
use crate::NOOP_WAKER;

fn get_socket<'a>(
    table: &'a ResourceTable,
    socket: &'a Resource<TcpSocket>,
) -> wasmtime::Result<&'a TcpSocket> {
    table
        .get(socket)
        .context("failed to get socket resource from table")
}

fn get_socket_mut<'a>(
    table: &'a mut ResourceTable,
    socket: &'a Resource<TcpSocket>,
) -> wasmtime::Result<&'a mut TcpSocket> {
    table
        .get_mut(socket)
        .context("failed to get socket resource from table")
}

impl<T> Host for WasiSocketsImpl<&mut T> where T: WasiSocketsView {}

impl<T> tcp_create_socket::Host for WasiSocketsImpl<&mut T>
where
    T: WasiSocketsView,
{
    fn create_tcp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> wasmtime::Result<Result<Resource<TcpSocket>, ErrorCode>> {
        let sock = TcpSocket::new(address_family.into()).context("failed to create socket")?;
        let sock = self
            .table()
            .push(sock)
            .context("failed to push socket resource to table")?;
        Ok(Ok(sock))
    }
}

impl<T> HostTcpSocket for WasiSocketsImpl<&mut T>
where
    T: WasiSocketsView,
{
    fn start_bind(
        &mut self,
        socket: Resource<TcpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let local_address = SocketAddr::from(local_address);
        let sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.start_bind(local_address))
    }

    fn finish_bind(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.finish_bind())
    }

    fn start_connect(
        &mut self,
        socket: Resource<TcpSocket>,
        network: Resource<Network>,
        remote_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.start_connect(remote_address))
    }

    fn finish_connect(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<(Resource<InputStream>, Resource<OutputStream>), ErrorCode>> {
        let table = self.table();
        let mut sock = get_socket_mut(table, &socket)?;
        match sock.finish_connect() {
            Ok((rx, tx)) => {
                let rx = table.push(rx)?;
                let tx = table.push(tx)?;
                Ok(Ok((rx, tx)))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    fn start_listen(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.start_listen())
    }

    fn finish_listen(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.finish_listen())
    }

    fn accept(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<
        Result<
            (
                Resource<TcpSocket>,
                Resource<InputStream>,
                Resource<OutputStream>,
            ),
            ErrorCode,
        >,
    > {
        let table = self.table();
        let mut sock = get_socket_mut(table, &socket)?;
        match sock.accept() {
            Ok((sock, rx, tx)) => {
                let sock = table.push(sock)?;
                let rx = table.push(rx)?;
                let tx = table.push(tx)?;
                Ok(Ok((sock, rx, tx)))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    fn subscribe(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<Resource<Pollable>> {
        let TcpSocket { tcp_state, .. } = get_socket(self.table(), &socket)?;
        let p = Pollable::TcpSocket(Arc::clone(tcp_state));
        push_pollable(self.table(), p)
    }

    fn shutdown(
        &mut self,
        socket: Resource<TcpSocket>,
        shutdown_type: ShutdownType,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.shutdown(match shutdown_type {
            ShutdownType::Receive => Shutdown::Read,
            ShutdownType::Send => Shutdown::Write,
            ShutdownType::Both => Shutdown::Both,
        }))
    }

    fn local_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.local_address())
    }

    fn remote_address(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.remote_address())
    }

    fn is_listening(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<bool> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.is_listening())
    }

    fn address_family(&mut self, socket: Resource<TcpSocket>) -> wasmtime::Result<IpAddressFamily> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.address_family())
    }

    fn set_listen_backlog_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.set_listen_backlog_size(value))
    }

    fn keep_alive_enabled(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<bool, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.keep_alive_enabled())
    }

    fn set_keep_alive_enabled(
        &mut self,
        socket: Resource<TcpSocket>,
        value: bool,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.set_keep_alive_enabled(value))
    }

    fn keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<Duration, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.keep_alive_idle_time())
    }

    fn set_keep_alive_idle_time(
        &mut self,
        socket: Resource<TcpSocket>,
        value: Duration,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.set_keep_alive_idle_time(value))
    }

    fn keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<Duration, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.keep_alive_interval())
    }

    fn set_keep_alive_interval(
        &mut self,
        socket: Resource<TcpSocket>,
        value: Duration,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.set_keep_alive_interval(value))
    }

    fn keep_alive_count(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<u32, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.keep_alive_count())
    }

    fn set_keep_alive_count(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u32,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.set_keep_alive_count(value))
    }

    fn hop_limit(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<u8, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.hop_limit())
    }

    fn set_hop_limit(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u8,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.set_hop_limit(value))
    }

    fn receive_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.receive_buffer_size())
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.set_receive_buffer_size(value))
    }

    fn send_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        let sock = get_socket(self.table(), &socket)?;
        Ok(sock.send_buffer_size())
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<TcpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let mut sock = get_socket_mut(self.table(), &socket)?;
        Ok(sock.set_send_buffer_size(value))
    }

    fn drop(&mut self, rep: Resource<TcpSocket>) -> wasmtime::Result<()> {
        self.table()
            .delete(rep)
            .context("failed to delete socket resource from table")?;
        Ok(())
    }
}
