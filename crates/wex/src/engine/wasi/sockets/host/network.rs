use core::net::SocketAddr;

use anyhow::Context as _;
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::sockets::instance_network;
use crate::engine::bindings::wasi::sockets::network::{ErrorCode, Host, HostNetwork};
use crate::engine::wasi;
use crate::engine::wasi::sockets::{Network, WasiSocketsImpl, WasiSocketsView};
use crate::engine::ResourceView as _;

impl<T> instance_network::Host for WasiSocketsImpl<&mut T>
where
    T: WasiSocketsView,
{
    fn instance_network(&mut self) -> wasmtime::Result<Resource<Network>> {
        self.table()
            .push(Network)
            .context("failed to push network resource")
    }
}

impl<T> HostNetwork for WasiSocketsImpl<&mut T>
where
    T: WasiSocketsView,
{
    fn drop(&mut self, net: Resource<Network>) -> wasmtime::Result<()> {
        self.table()
            .delete(net)
            .context("failed to delete network resource")?;
        Ok(())
    }
}

impl<T> Host for WasiSocketsImpl<&mut T>
where
    T: WasiSocketsView,
{
    fn network_error_code(
        &mut self,
        err: Resource<wasi::io::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        todo!()
    }
}
