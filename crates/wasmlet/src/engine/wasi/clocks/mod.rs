mod host;

use wasmtime::component::Linker;

use crate::engine::bindings::wasi::clocks;
use crate::Ctx;

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: Send>(
    linker: &mut Linker<T>,
    get: impl Fn(&mut T) -> &mut Ctx + Copy + Sync + Send + 'static,
) -> wasmtime::Result<()> {
    clocks::wall_clock::add_to_linker(linker, get)?;
    clocks::monotonic_clock::add_to_linker(linker, get)?;
    Ok(())
}
