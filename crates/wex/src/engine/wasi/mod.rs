#![allow(unused)]

pub mod cli;
pub mod clocks;
pub mod filesystem;
pub mod http;
pub mod io;
pub mod random;
pub mod sockets;

use wasmtime::component::Linker;

use crate::engine::bindings::LinkOptions;

pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: clocks::WasiClocksView
        + random::WasiRandomView
        + sockets::WasiSocketsView
        + filesystem::WasiFilesystemView
        + cli::WasiCliView
        + http::WasiHttpView,
{
    let options = LinkOptions::default();
    add_to_linker_with_options(linker, &options)
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T>(
    linker: &mut Linker<T>,
    options: &LinkOptions,
) -> anyhow::Result<()>
where
    T: clocks::WasiClocksView
        + random::WasiRandomView
        + sockets::WasiSocketsView
        + filesystem::WasiFilesystemView
        + cli::WasiCliView
        + http::WasiHttpView,
{
    cli::add_to_linker_with_options(linker, &options.into())?;
    clocks::add_to_linker(linker)?;
    filesystem::add_to_linker(linker)?;
    http::add_to_linker(linker)?;
    random::add_to_linker(linker)?;
    sockets::add_to_linker(linker)?;
    Ok(())
}
