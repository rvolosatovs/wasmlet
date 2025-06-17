use core::sync::atomic::Ordering;
use core::time::Duration;

use anyhow::Context as _;
use tokio::time::sleep;
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::clocks::{monotonic_clock, wall_clock};
use crate::engine::wasi::io::Pollable;
use crate::{Ctx, EPOCH_INTERVAL, EPOCH_MONOTONIC_NOW, EPOCH_SYSTEM_NOW};

const NANOS_PER_MILLI: u32 = 1_000_000;

impl wall_clock::Host for Ctx {
    fn now(&mut self) -> wasmtime::Result<wall_clock::Datetime> {
        debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));

        let now = Duration::from_millis(EPOCH_SYSTEM_NOW.load(Ordering::Relaxed));
        Ok(wall_clock::Datetime {
            seconds: now.as_secs(),
            nanoseconds: now.subsec_nanos(),
        })
    }

    fn resolution(&mut self) -> wasmtime::Result<wall_clock::Datetime> {
        debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));

        Ok(wall_clock::Datetime {
            seconds: 0,
            nanoseconds: NANOS_PER_MILLI,
        })
    }
}

impl monotonic_clock::Host for Ctx {
    fn now(&mut self) -> wasmtime::Result<monotonic_clock::Instant> {
        debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));

        let now = EPOCH_MONOTONIC_NOW
            .load(Ordering::Relaxed)
            .saturating_sub(self.init_at);
        Ok(now.saturating_mul(NANOS_PER_MILLI.into()))
    }

    fn resolution(&mut self) -> wasmtime::Result<monotonic_clock::Instant> {
        debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));

        Ok(NANOS_PER_MILLI.into())
    }

    fn subscribe_instant(
        &mut self,
        when: monotonic_clock::Instant,
    ) -> wasmtime::Result<Resource<Pollable>> {
        debug_assert_eq!(EPOCH_INTERVAL, Duration::from_millis(1));

        let when = Duration::from_nanos(when);
        let now = Duration::from_millis(EPOCH_MONOTONIC_NOW.load(Ordering::Relaxed));

        let d = when.saturating_sub(now);
        let p = if !d.is_zero() {
            Pollable::sleep(sleep(d))
        } else {
            Pollable::Ready
        };
        self.table
            .push(p)
            .context("failed to push pollable resource")
    }

    fn subscribe_duration(
        &mut self,
        duration: monotonic_clock::Duration,
    ) -> wasmtime::Result<Resource<Pollable>> {
        let duration = Duration::from_nanos(duration);
        let p = if !duration.is_zero() {
            Pollable::sleep(sleep(duration))
        } else {
            Pollable::Ready
        };
        self.table
            .push(p)
            .context("failed to push pollable resource")
    }
}
