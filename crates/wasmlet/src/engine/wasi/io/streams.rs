use anyhow::Context as _;
use tracing::instrument;
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::bindings::wasi::io::poll::Pollable;
use crate::engine::bindings::wasi::io::streams::{
    Host, HostInputStream, HostOutputStream, StreamError,
};
use crate::engine::wasi::io::{push_pollable, Error, InputStream, OutputStream};
use crate::Ctx;

fn get_input_stream<'a>(
    table: &'a mut ResourceTable,
    stream: &Resource<InputStream>,
) -> wasmtime::Result<&'a InputStream> {
    table
        .get(stream)
        .context("failed to get input stream resource from table")
}

fn get_input_stream_mut<'a>(
    table: &'a mut ResourceTable,
    stream: &Resource<InputStream>,
) -> wasmtime::Result<&'a mut InputStream> {
    table
        .get_mut(stream)
        .context("failed to get input stream resource from table")
}

fn get_output_stream_mut<'a>(
    table: &'a mut ResourceTable,
    stream: &Resource<OutputStream>,
) -> wasmtime::Result<&'a mut OutputStream> {
    table
        .get_mut(stream)
        .context("failed to get output stream resource from table")
}

fn push_error(table: &mut ResourceTable, err: Error) -> wasmtime::Result<Resource<Error>> {
    table
        .push(err)
        .context("failed to push error resource to table")
}

impl Host for Ctx {}

impl HostInputStream for Ctx {
    fn read(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, StreamError>> {
        let stream = get_input_stream_mut(&mut self.table, &stream)?;
        match stream.read(len)? {
            Ok(buf) => Ok(Ok(buf)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    async fn blocking_read(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, StreamError>> {
        let stream = get_input_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_read(len).await? {
            Ok(buf) => Ok(Ok(buf)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn skip(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let stream = get_input_stream_mut(&mut self.table, &stream)?;
        match stream.skip(len)? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    async fn blocking_skip(
        &mut self,
        stream: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let stream = get_input_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_skip(len).await? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    fn subscribe(&mut self, stream: Resource<InputStream>) -> wasmtime::Result<Resource<Pollable>> {
        let stream = get_input_stream_mut(&mut self.table, &stream)?;
        let p = stream.subscribe()?;
        push_pollable(&mut self.table, p)
    }

    fn drop(&mut self, stream: Resource<InputStream>) -> wasmtime::Result<()> {
        self.table
            .delete(stream)
            .context("failed to delete input stream resource")?;
        Ok(())
    }
}

impl HostOutputStream for Ctx {
    #[instrument(level = "trace", ret)]
    fn check_write(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        match stream.check_write()? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    fn write(
        &mut self,
        stream: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        match stream.write(contents)? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    async fn blocking_write_and_flush(
        &mut self,
        stream: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_write_and_flush(contents).await? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    fn flush(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        match stream.flush()? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    async fn blocking_flush(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_flush().await? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    fn subscribe(
        &mut self,
        stream: Resource<OutputStream>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        let p = stream.subscribe()?;
        push_pollable(&mut self.table, p)
    }

    #[instrument(level = "trace", ret)]
    fn write_zeroes(
        &mut self,
        stream: Resource<OutputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        match stream.write_zeroes(len)? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    async fn blocking_write_zeroes_and_flush(
        &mut self,
        stream: Resource<OutputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<(), StreamError>> {
        let stream = get_output_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_write_zeroes_and_flush(len).await? {
            Ok(()) => Ok(Ok(())),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    fn splice(
        &mut self,
        stream: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let src = get_input_stream(&mut self.table, &src)?;
        let src = src.clone();
        let stream = get_output_stream_mut(&mut self.table, &stream)?;
        match stream.splice(&src, len)? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    async fn blocking_splice(
        &mut self,
        stream: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> wasmtime::Result<Result<u64, StreamError>> {
        let src = get_input_stream(&mut self.table, &src)?;
        let src = src.clone();
        let stream = get_output_stream_mut(&mut self.table, &stream)?;

        // TODO: Shutdown deadline

        match stream.blocking_splice(&src, len).await? {
            Ok(n) => Ok(Ok(n)),
            Err(None) => Ok(Err(StreamError::Closed)),
            Err(Some(err)) => {
                let err = push_error(&mut self.table, err)?;
                Ok(Err(StreamError::LastOperationFailed(err)))
            }
        }
    }

    #[instrument(level = "trace", ret)]
    fn drop(&mut self, stream: Resource<OutputStream>) -> wasmtime::Result<()> {
        self.table
            .delete(stream)
            .context("failed to delete output stream resource")?;
        Ok(())
    }
}
