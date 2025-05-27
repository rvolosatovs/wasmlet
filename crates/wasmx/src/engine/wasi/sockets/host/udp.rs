#![allow(unused)] // TODO: Remove

use anyhow::Context as _;
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::bindings::wasi::sockets::network::{
    ErrorCode, IpAddressFamily, IpSocketAddress,
};
use crate::engine::bindings::wasi::sockets::udp::{
    Host, HostIncomingDatagramStream, HostOutgoingDatagramStream, HostUdpSocket, IncomingDatagram,
    OutgoingDatagram,
};
use crate::engine::bindings::wasi::sockets::udp_create_socket;
use crate::engine::wasi::io::Pollable;
use crate::engine::wasi::sockets::udp::{
    IncomingDatagramStream, OutgoingDatagramStream, UdpSocket,
};
use crate::engine::wasi::sockets::Network;
use crate::Ctx;

fn get_socket<'a>(
    table: &'a ResourceTable,
    socket: &'a Resource<UdpSocket>,
) -> wasmtime::Result<&'a UdpSocket> {
    table
        .get(socket)
        .context("failed to get socket resource from table")
}

fn get_socket_mut<'a>(
    table: &'a mut ResourceTable,
    socket: &'a Resource<UdpSocket>,
) -> wasmtime::Result<&'a mut UdpSocket> {
    table
        .get_mut(socket)
        .context("failed to get socket resource from table")
}

impl udp_create_socket::Host for Ctx {
    fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> wasmtime::Result<Result<Resource<UdpSocket>, ErrorCode>> {
        let socket = UdpSocket::new(address_family.into()).context("failed to create socket")?;
        let socket = self
            .table
            .push(socket)
            .context("failed to push socket resource to table")?;
        Ok(Ok(socket))
    }
}

impl Host for Ctx {}

impl HostIncomingDatagramStream for Ctx {
    fn receive(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
        max_results: u64,
    ) -> wasmtime::Result<Result<Vec<IncomingDatagram>, ErrorCode>> {
        todo!()
    }

    fn subscribe(
        &mut self,
        self_: Resource<IncomingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<IncomingDatagramStream>) -> wasmtime::Result<()> {
        todo!()
    }
}

impl HostOutgoingDatagramStream for Ctx {
    fn check_send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    fn send(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
        datagrams: Vec<OutgoingDatagram>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        todo!()
    }

    fn subscribe(
        &mut self,
        self_: Resource<OutgoingDatagramStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<OutgoingDatagramStream>) -> wasmtime::Result<()> {
        todo!()
    }
}

impl HostUdpSocket for Ctx {
    fn start_bind(
        &mut self,
        self_: Resource<UdpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn finish_bind(
        &mut self,
        self_: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn stream(
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

    fn subscribe(&mut self, self_: Resource<UdpSocket>) -> wasmtime::Result<Resource<Pollable>> {
        todo!()
    }

    fn local_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.local_address())
    }

    fn remote_address(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<IpSocketAddress, ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.remote_address())
    }

    fn address_family(&mut self, socket: Resource<UdpSocket>) -> wasmtime::Result<IpAddressFamily> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.address_family())
    }

    fn unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u8, ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.unicast_hop_limit())
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u8,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.set_unicast_hop_limit(value))
    }

    fn receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.receive_buffer_size())
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.set_receive_buffer_size(value))
    }

    fn send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
    ) -> wasmtime::Result<Result<u64, ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.send_buffer_size())
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UdpSocket>,
        value: u64,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let sock = get_socket(&mut self.table, &socket)?;
        Ok(sock.set_send_buffer_size(value))
    }

    fn drop(&mut self, socket: Resource<UdpSocket>) -> wasmtime::Result<()> {
        self.table
            .delete(socket)
            .context("failed to delete socket resource from table")?;
        Ok(())
    }
}
