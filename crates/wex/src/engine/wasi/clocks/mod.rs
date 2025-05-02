mod host;

use core::sync::atomic::Ordering;

use quanta::{Clock, Instant};
use wasmtime::component::{Linker, ResourceTable};

use crate::{engine::ResourceView, EPOCH_MONOTONIC_NOW};

#[repr(transparent)]
pub struct WasiClocksImpl<T>(pub T);

impl<T: WasiClocksView> WasiClocksView for &mut T {
    fn clocks(&mut self) -> &mut WasiClocksCtx {
        (**self).clocks()
    }
}

impl<T: WasiClocksView> WasiClocksView for WasiClocksImpl<T> {
    fn clocks(&mut self) -> &mut WasiClocksCtx {
        self.0.clocks()
    }
}

impl<T: ResourceView> ResourceView for WasiClocksImpl<T> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }
}

pub trait WasiClocksView: ResourceView + Send {
    fn clocks(&mut self) -> &mut WasiClocksCtx;
}

pub struct WasiClocksCtx {
    init: u64,
}

impl Default for WasiClocksCtx {
    fn default() -> Self {
        let init = EPOCH_MONOTONIC_NOW.load(Ordering::Relaxed);
        Self { init }
    }
}

/// Add all WASI interfaces from this module into the `linker` provided.
pub fn add_to_linker<T: WasiClocksView>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    let closure = annotate_clocks(|cx| WasiClocksImpl(cx));
    crate::engine::bindings::wasi::clocks::wall_clock::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::clocks::monotonic_clock::add_to_linker_get_host(
        linker, closure,
    )?;
    Ok(())
}

fn annotate_clocks<T, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasiClocksImpl<&mut T>,
{
    val
}
