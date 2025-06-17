use anyhow::{bail, Context as _};
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::sockets::instance_network;
use crate::engine::bindings::wasi::sockets::network::{ErrorCode, Host, HostNetwork};
use crate::engine::wasi;
use crate::engine::wasi::sockets::Network;
use crate::Ctx;

impl instance_network::Host for Ctx {
    fn instance_network(&mut self) -> wasmtime::Result<Resource<Network>> {
        self.table
            .push(Network)
            .context("failed to push network resource")
    }
}

impl HostNetwork for Ctx {
    fn drop(&mut self, net: Resource<Network>) -> wasmtime::Result<()> {
        self.table
            .delete(net)
            .context("failed to delete network resource")?;
        Ok(())
    }
}

impl Host for Ctx {
    fn network_error_code(
        &mut self,
        _err: Resource<wasi::io::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        bail!("not supported yet")
    }
}
