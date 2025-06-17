pub mod cli;
pub mod clocks;
pub mod filesystem;
pub mod http;
pub mod io;
pub mod random;
pub mod sockets;

use wasmtime::component::Linker;

use crate::engine::bindings::LinkOptions;
use crate::engine::Ctx;

pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> wasmtime::Result<()> {
    let options = LinkOptions::default();
    add_to_linker_with_options(linker, get, &options)
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
    options: &LinkOptions,
) -> anyhow::Result<()> {
    io::add_to_linker(linker, get)?;
    cli::add_to_linker_with_options(linker, get, &options.into())?;
    clocks::add_to_linker(linker, get)?;
    filesystem::add_to_linker(linker, get)?;
    http::add_to_linker(linker, get)?;
    random::add_to_linker(linker, get)?;
    sockets::add_to_linker_with_options(linker, get, &options.into())?;
    Ok(())
}
