use std::sync::Arc;

use anyhow::{bail, Context as _};
use tokio::sync::{mpsc, oneshot, TryAcquireError};
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::io::poll::Pollable;
use crate::engine::bindings::wasi::io::streams::{
    Host, HostInputStream, HostOutputStream, StreamError,
};
use crate::engine::wasi::io::{ChannelInputStream, Error, InputStream, OutputStream};
use crate::engine::ResourceView as _;
use crate::Ctx;

impl Host for Ctx {}

impl HostInputStream for Ctx {
    fn read(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        match stream.read(len)? {
            Ok(buf) => Ok(Ok(buf)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    async fn blocking_read(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        match stream.blocking_read(len).await? {
            Ok(buf) => Ok(Ok(buf)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn skip(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        bail!("not supported yet (skip)")
    }

    fn blocking_skip(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        bail!("not supported yet (blocking_skip)")
    }

    fn subscribe(&mut self, stream: Resource<InputStream>) -> wasmtime::Result<Resource<Pollable>> {
        let table = self.table();
        let stream = table
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        let p = stream.subscribe()?;
        table.push(p).context("failed to push pollable resource")
    }

    fn drop(&mut self, stream: Resource<InputStream>) -> wasmtime::Result<()> {
        self.table()
            .delete(stream)
            .context("failed to delete input stream resource")?;
        Ok(())
    }
}

impl HostOutputStream for Ctx {
    fn check_write(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        match stream.check_write()? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn write(
        &mut self,
        stream: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        match stream.write(contents)? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        stream: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;

        // TODO: Shutdown deadline

        match stream.blocking_write_and_flush(contents).await? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn flush(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        match stream.flush()? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    async fn blocking_flush(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = self
            .table()
            .get_mut(&stream)
            .context("failed to get stream resource")?;

        // TODO: Shutdown deadline

        match stream.blocking_flush().await? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = self
                    .table()
                    .push(err)
                    .context("failed to push error resource")?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn subscribe(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        let table = self.table();
        let stream = table
            .get_mut(&stream)
            .context("failed to get stream resource")?;
        let p = stream.subscribe()?;
        table.push(p).context("failed to push pollable resource")
    }

    fn write_zeroes(
        &mut self,
        stream: Resource<OutputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        bail!("not supported yet (write_zeroes)")
    }

    fn blocking_write_zeroes_and_flush(
        &mut self,
        stream: Resource<OutputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        bail!("not supported yet (blocking_write_zero)")
    }

    fn splice(
        &mut self,
        stream: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        bail!("not supported yet (splice)")
    }

    fn blocking_splice(
        &mut self,
        stream: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        bail!("not supported yet (blocking_splice)")
    }

    fn drop(&mut self, stream: Resource<OutputStream>) -> wasmtime::Result<()> {
        self.table()
            .delete(stream)
            .context("failed to delete output stream resource")?;
        Ok(())
    }
}
