use core::future::{poll_fn, Future as _};
use core::iter::zip;
use core::sync::atomic::Ordering;
use core::task::{ready, Poll};
use core::time::Duration;

use anyhow::{anyhow, ensure, Context as _};
use tokio::time::sleep;
use tracing::{debug, instrument};
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::io::poll::{Host, HostPollable};
use crate::engine::wasi::io::Pollable;
use crate::{Ctx, EPOCH_MONOTONIC_NOW};

impl Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn poll(&mut self, pollables: Vec<Resource<Pollable>>) -> wasmtime::Result<Vec<u32>> {
        ensure!(!pollables.is_empty(), "pollables list cannot be empty");
        ensure!(
            u32::try_from(pollables.len()).is_ok(),
            "there can be at most {} pollables in the list",
            u32::MAX
        );

        let mut pollables = pollables
            .into_iter()
            .map(|p| {
                self.table
                    .get(&p)
                    .context("failed to get pollable resource")
            })
            .collect::<wasmtime::Result<Vec<_>>>()?;

        debug!(?pollables, "first poll");
        let mut ready = Vec::with_capacity(pollables.len());
        for (i, p) in zip(0.., &pollables) {
            if p.is_ready() {
                ready.push(i);
            }
        }
        if !ready.is_empty() {
            debug!(?ready, "first poll ready");
            return Ok(ready);
        }

        let mut deadline_sleep = None;
        let mut shutdown_changed = None;
        poll_fn(|cx| {
            debug!(?pollables, "async poll");

            for (i, p) in zip(0.., &mut *pollables) {
                if let Poll::Ready(()) = p.poll(cx) {
                    ready.push(i);
                }
            }
            if !ready.is_empty() {
                debug!(?ready, "async poll ready");
                return Poll::Ready(Ok(()));
            };

            loop {
                let shutdown = self.shutdown.borrow();
                if self.deadline != 0 && *shutdown != 0 {
                    self.deadline = shutdown.min(self.deadline);
                } else if self.deadline == 0 {
                    self.deadline = *shutdown;
                }

                let now = EPOCH_MONOTONIC_NOW.load(Ordering::Relaxed);
                if self.deadline > 0 {
                    debug!("poll deadline");
                    let d = self.deadline.saturating_sub(now);
                    if d == 0 {
                        return Poll::Ready(Err(anyhow!(
                            "execution time deadline reached in `wasi:io/poll#poll`"
                        )));
                    }
                    let fut = deadline_sleep
                        .get_or_insert_with(|| Box::pin(sleep(Duration::from_millis(d))));
                    if let Poll::Ready(()) = fut.as_mut().poll(cx) {
                        return Poll::Ready(Err(anyhow!(
                            "execution time deadline reached in `wasi:io/poll#poll`"
                        )));
                    }
                }
                if *shutdown == 0 {
                    drop(shutdown);
                    debug!("poll shutdown");
                    let fut = shutdown_changed.get_or_insert_with(|| {
                        let mut shutdown = self.shutdown.clone();
                        Box::pin(async move { shutdown.changed().await })
                    });
                    if ready!(fut.as_mut().poll(cx)).is_err() {
                        debug!("forcing shutdown");
                        return Poll::Ready(Err(anyhow!("forced shutdown in `wasi:io/poll#poll`")));
                    }
                    debug!("shutdown requested");
                    deadline_sleep.take();
                }
            }
        })
        .await?;
        Ok(ready)
    }
}

impl HostPollable for Ctx {
    fn ready(&mut self, pollable: Resource<Pollable>) -> wasmtime::Result<bool> {
        let pollable = self
            .table
            .get(&pollable)
            .context("failed to get pollable resource")?;
        Ok(pollable.is_ready())
    }

    async fn block(&mut self, pollable: Resource<Pollable>) -> wasmtime::Result<()> {
        let pollable = self
            .table
            .get_mut(&pollable)
            .context("failed to get pollable resource")?;

        // TODO: Shutdown deadline

        match pollable {
            Pollable::TcpSocket(sock) => {
                poll_fn(|cx| {
                    let Ok(mut state) = sock.write() else {
                        return Poll::Ready(());
                    };
                    state.poll(cx)
                })
                .await;
            }
            Pollable::TcpStreamReadable(stream) => {
                _ = stream.readable().await;
            }
            Pollable::TcpStreamWritable(stream) => {
                _ = stream.writable().await;
            }
            Pollable::UdpSocketReadable(socket) => {
                _ = socket.readable().await;
            }
            Pollable::UdpSocketWritable(socket) => {
                _ = socket.writable().await;
            }
            Pollable::Semaphore(semaphore) => {
                if semaphore.available_permits() == 0 {
                    _ = semaphore.acquire().await;
                }
            }
            Pollable::Sleep(sleep) => {
                poll_fn(|cx| {
                    let Ok(mut state) = sleep.write() else {
                        return Poll::Ready(());
                    };
                    state.poll(cx)
                })
                .await;
            }
            Pollable::Ready => {}
        }
        Ok(())
    }

    fn drop(&mut self, pollable: Resource<Pollable>) -> wasmtime::Result<()> {
        self.table
            .delete(pollable)
            .context("failed to delete pollable resource")?;
        Ok(())
    }
}
