use anyhow::Context as _;
use wasmtime::component::Resource;

use crate::engine::bindings::wasi::io::error::{Host, HostError};
use crate::engine::wasi::io::Error;
use crate::Ctx;

impl Host for Ctx {}
impl HostError for Ctx {
    fn to_debug_string(&mut self, err: Resource<Error>) -> wasmtime::Result<String> {
        let err = self
            .table
            .get(&err)
            .context("failed to get error resource")?;
        Ok(format!("{err:?}"))
    }

    fn drop(&mut self, err: Resource<Error>) -> wasmtime::Result<()> {
        self.table
            .delete(err)
            .context("failed to delete error resource")?;
        Ok(())
    }
}
