#![allow(unused)]

use std::collections::HashMap;

use anyhow::Context as _;
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::async_trait;

use crate::bindings::wasi::keyvalue::{atomics, batch, store};
use crate::Ctx;

type Result<T, E = store::Error> = core::result::Result<T, E>;

pub enum Bucket {
    Memory(HashMap<Box<str>, Box<str>>),
}

#[async_trait]
impl store::Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn open(&mut self, name: String) -> anyhow::Result<Result<Resource<store::Bucket>>> {
        todo!()
    }
}

#[async_trait]
impl atomics::Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn increment(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!();
    }
}

#[async_trait]
impl batch::Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn get_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Vec<u8>)>>>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        entries: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }
}

#[async_trait]
impl store::HostBucket for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn set(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn exists(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn list_keys(
        &mut self,
        bucket: Resource<store::Bucket>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(&mut self, bucket: Resource<store::Bucket>) -> anyhow::Result<()> {
        self.table
            .delete(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }
}
