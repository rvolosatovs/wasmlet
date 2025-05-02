use core::task::{Context, Poll, Waker};
use core::time::Duration;
use core::{cell::OnceCell, task::ready};
use core::{future::poll_fn, pin::pin};
use core::{future::Future as _, iter::zip};
use core::{num::NonZeroU64, sync::atomic::Ordering};

use anyhow::{anyhow, bail, ensure, Context as _};
use tokio::sync::TryLockError;
use tokio::time::sleep;
use tracing::{debug, instrument};
use wasmtime::component::Resource;

use crate::engine::wasi::{
    io::{ChannelInputStream, Pollable},
    sockets::tcp::TcpState,
};
use crate::engine::{
    bindings::wasi::io::poll::{Host, HostPollable},
    wasi,
};
use crate::{Ctx, EPOCH_MONOTONIC_NOW};

use super::SleepState;

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
                    let mut fut = deadline_sleep
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
                    let mut fut = shutdown_changed.get_or_insert_with(|| {
                        let mut shutdown = self.shutdown.clone();
                        Box::pin(async move { shutdown.changed().await })
                    });
                    if let Err(..) = ready!(fut.as_mut().poll(cx)) {
                        debug!("forcing shutdown");
                        return Poll::Ready(Err(anyhow!("forced shutdown in `wasi:io/poll#poll`")));
                    }
                    let shutdown = self.shutdown.borrow();
                    deadline_sleep.take();
                    debug!("shutdown requested");
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
                stream.readable().await;
            }
            Pollable::TcpStreamWritable(stream) => {
                stream.writable().await;
            }
            Pollable::UdpSocketReadable(socket) => {
                socket.readable().await;
            }
            Pollable::UdpSocketWritable(socket) => {
                socket.writable().await;
            }
            //Pollable::Receiver(stream) => {
            //    let mut stream = stream.lock().await;
            //    let ChannelInputStream { buffer, ref mut rx } = &mut *stream;
            //    if buffer.is_empty() {
            //        if let Some(buf) = rx.recv().await {
            //            *buffer = buf
            //        }
            //    }
            //}
            Pollable::Semaphore(semaphore) => {
                if semaphore.available_permits() == 0 {
                    semaphore.acquire().await;
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
