use core::future::Future;
use core::net::SocketAddr;
use core::ops::Deref;
use core::pin::Pin;

use std::sync::Arc;

use wasmtime::component::Linker;
use wasmtime::component::ResourceTable;

use crate::engine::ResourceView;

pub use crate::engine::bindings::wasi::sockets::network::ErrorCode;

pub struct ResolveAddressStream;

mod host;
pub mod tcp;
pub mod udp;
pub mod util;

#[derive(Debug, Clone)]
pub struct Network;

#[repr(transparent)]
pub struct WasiSocketsImpl<T>(pub T);

impl<T: WasiSocketsView> WasiSocketsView for &mut T {
    fn sockets(&self) -> &WasiSocketsCtx {
        (**self).sockets()
    }
}

impl<T: WasiSocketsView> WasiSocketsView for WasiSocketsImpl<T> {
    fn sockets(&self) -> &WasiSocketsCtx {
        self.0.sockets()
    }
}

impl<T: ResourceView> ResourceView for WasiSocketsImpl<T> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }
}

pub trait WasiSocketsView: ResourceView + Send {
    fn sockets(&self) -> &WasiSocketsCtx;
}

#[derive(Clone, Default)]
pub struct WasiSocketsCtx {}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum SocketAddressFamily {
    Ipv4,
    Ipv6,
}

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: WasiSocketsView>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    use crate::engine::bindings::wasi::sockets;

    let closure = annotate_sockets(|cx| WasiSocketsImpl(cx));
    sockets::instance_network::add_to_linker_get_host(linker, closure)?;
    sockets::ip_name_lookup::add_to_linker_get_host(linker, closure)?;
    sockets::network::add_to_linker_get_host(
        linker,
        &sockets::network::LinkOptions::default(),
        closure,
    )?;
    sockets::tcp::add_to_linker_get_host(linker, closure)?;
    sockets::tcp_create_socket::add_to_linker_get_host(linker, closure)?;
    sockets::udp::add_to_linker_get_host(linker, closure)?;
    sockets::udp_create_socket::add_to_linker_get_host(linker, closure)?;
    Ok(())
}

fn annotate_sockets<T, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasiSocketsImpl<&mut T>,
{
    val
}
