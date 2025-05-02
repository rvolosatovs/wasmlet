pub mod config;
mod engine;
mod host;
pub mod wasmtime;

use core::fmt::Debug;
use core::str::FromStr;
use core::sync::atomic::AtomicU64;
use core::task::Waker;
use core::time::Duration;

use std::env::{self, VarError};

use tracing::warn;

pub use self::config::Manifest;
pub use self::engine::{
    Cmd, Ctx, DynamicWorkloadInvocation, Engine, WorkloadInvocation, WorkloadInvocationPayload,
};
pub use self::host::{apply_manifest, load_and_apply_manifest, load_manifest, Host};

pub const EPOCH_INTERVAL: Duration = Duration::from_millis(1);

pub static EPOCH_MONOTONIC_NOW: AtomicU64 = AtomicU64::new(0);
pub static EPOCH_SYSTEM_NOW: AtomicU64 = AtomicU64::new(0);

pub(crate) const NOOP_WAKER: &Waker = Waker::noop();

fn getenv<T>(key: &str) -> Option<T>
where
    T: FromStr,
    T::Err: Debug,
{
    match env::var(key).as_deref().map(FromStr::from_str) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(err)) => {
            warn!(?err, "failed to parse `{key}` value, ignoring");
            None
        }
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(..)) => {
            warn!("`{key}` value is not valid UTF-8, ignoring");
            None
        }
    }
}
