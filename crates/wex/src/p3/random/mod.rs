mod host;

use cap_rand::{Rng as _, RngCore, SeedableRng as _};
use wasmtime::component::Linker;

#[repr(transparent)]
pub struct WasiRandomImpl<T>(pub T);

impl<T: WasiRandomView> WasiRandomView for &mut T {
    fn random(&mut self) -> &mut WasiRandomCtx {
        (**self).random()
    }
}

impl<T: WasiRandomView> WasiRandomView for WasiRandomImpl<T> {
    fn random(&mut self) -> &mut WasiRandomCtx {
        self.0.random()
    }
}

pub trait WasiRandomView: Send {
    fn random(&mut self) -> &mut WasiRandomCtx;
}

pub struct WasiRandomCtx {
    pub random: Box<dyn RngCore + Send>,
    pub insecure_random: Box<dyn RngCore + Send>,
    pub insecure_random_seed: u128,
}

impl Default for WasiRandomCtx {
    fn default() -> Self {
        // For the insecure random API, use `SmallRng`, which is fast. It's
        // also insecure, but that's the deal here.
        let insecure_random = Box::new(
            cap_rand::rngs::SmallRng::from_rng(cap_rand::thread_rng(cap_rand::ambient_authority()))
                .unwrap(),
        );
        // For the insecure random seed, use a `u128` generated from
        // `thread_rng()`, so that it's not guessable from the insecure_random
        // API.
        let insecure_random_seed =
            cap_rand::thread_rng(cap_rand::ambient_authority()).r#gen::<u128>();
        Self {
            random: thread_rng(),
            insecure_random,
            insecure_random_seed,
        }
    }
}

/// Implement `insecure-random` using a deterministic cycle of bytes.
pub struct Deterministic {
    cycle: std::iter::Cycle<std::vec::IntoIter<u8>>,
}

impl Deterministic {
    pub fn new(bytes: Vec<u8>) -> Self {
        Deterministic {
            cycle: bytes.into_iter().cycle(),
        }
    }
}

impl RngCore for Deterministic {
    fn next_u32(&mut self) -> u32 {
        let b0 = self.cycle.next().expect("infinite sequence");
        let b1 = self.cycle.next().expect("infinite sequence");
        let b2 = self.cycle.next().expect("infinite sequence");
        let b3 = self.cycle.next().expect("infinite sequence");
        ((b0 as u32) << 24) + ((b1 as u32) << 16) + ((b2 as u32) << 8) + (b3 as u32)
    }
    fn next_u64(&mut self) -> u64 {
        let w0 = self.next_u32();
        let w1 = self.next_u32();
        ((w0 as u64) << 32) + (w1 as u64)
    }
    fn fill_bytes(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            *b = self.cycle.next().expect("infinite sequence");
        }
    }
    fn try_fill_bytes(&mut self, buf: &mut [u8]) -> Result<(), cap_rand::Error> {
        self.fill_bytes(buf);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn deterministic() {
        let mut det = Deterministic::new(vec![1, 2, 3, 4]);
        let mut buf = vec![0; 1024];
        det.try_fill_bytes(&mut buf).expect("get randomness");
        for (ix, b) in buf.iter().enumerate() {
            assert_eq!(*b, (ix % 4) as u8 + 1)
        }
    }
}

pub fn thread_rng() -> Box<dyn RngCore + Send> {
    use cap_rand::{Rng, SeedableRng};
    let mut rng = cap_rand::thread_rng(cap_rand::ambient_authority());
    Box::new(cap_rand::rngs::StdRng::from_seed(rng.r#gen()))
}

/// Add all WASI interfaces from this module into the `linker` provided.
///
/// This function will add the `async` variant of all interfaces into the
/// [`Linker`] provided. By `async` this means that this function is only
/// compatible with [`Config::async_support(true)`][async]. For embeddings with
/// async support disabled see [`add_to_linker_sync`] instead.
///
/// This function will add all interfaces implemented by this crate to the
/// [`Linker`], which corresponds to the `wasi:random/imports` world supported by
/// this crate.
///
/// [async]: wasmtime::Config::async_support
///
/// # Example
///
/// ```
/// use wasmtime::{Engine, Result, Store, Config};
/// use wasmtime::component::{ResourceTable, Linker};
/// use wasmtime_wasi::p3::random::{WasiRandomView, WasiRandomCtx};
///
/// fn main() -> Result<()> {
///     let mut config = Config::new();
///     config.async_support(true);
///     let engine = Engine::new(&config)?;
///
///     let mut linker = Linker::<MyState>::new(&engine);
///     wasmtime_wasi::p3::random::add_to_linker(&mut linker)?;
///     // ... add any further functionality to `linker` if desired ...
///
///     let mut store = Store::new(
///         &engine,
///         MyState {
///             random: WasiRandomCtx::default(),
///         },
///     );
///
///     // ... use `linker` to instantiate within `store` ...
///
///     Ok(())
/// }
///
/// struct MyState {
///     random: WasiRandomCtx,
/// }
///
/// impl WasiRandomView for MyState {
///     fn random(&mut self) -> &mut WasiRandomCtx { &mut self.random }
/// }
/// ```
pub fn add_to_linker<T: WasiRandomView + 'static>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    let closure = annotate_random(|cx| WasiRandomImpl(cx));
    crate::p3::bindings::random::random::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::random::insecure::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::random::insecure_seed::add_to_linker_get_host(linker, closure)?;
    Ok(())
}

fn annotate_random<T, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasiRandomImpl<&mut T>,
{
    val
}
