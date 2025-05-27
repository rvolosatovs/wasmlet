use wasmtime::component::Linker;

use crate::engine::bindings::wasi::sockets;
use crate::Ctx;

pub use sockets::network::ErrorCode;

pub struct ResolveAddressStream;

mod host;
pub mod tcp;
pub mod udp;
pub mod util;

#[derive(Debug, Clone)]
pub struct Network;

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum SocketAddressFamily {
    Ipv4,
    Ipv6,
}

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> wasmtime::Result<()> {
    add_to_linker_with_options(linker, get, &sockets::network::LinkOptions::default())
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
    network_options: &sockets::network::LinkOptions,
) -> wasmtime::Result<()> {
    sockets::instance_network::add_to_linker(linker, get)?;
    sockets::ip_name_lookup::add_to_linker(linker, get)?;
    sockets::network::add_to_linker(linker, network_options, get)?;
    sockets::tcp::add_to_linker(linker, get)?;
    sockets::tcp_create_socket::add_to_linker(linker, get)?;
    sockets::udp::add_to_linker(linker, get)?;
    sockets::udp_create_socket::add_to_linker(linker, get)?;
    Ok(())
}
