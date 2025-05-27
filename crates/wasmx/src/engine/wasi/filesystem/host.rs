use anyhow::Context as _;
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::bindings::wasi::filesystem::preopens;
use crate::engine::bindings::wasi::filesystem::types::{
    self, Advice, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry, ErrorCode,
    Filesize, MetadataHashValue, NewTimestamp, OpenFlags, PathFlags,
};
use crate::engine::wasi;
use crate::engine::wasi::filesystem::{Descriptor, DirectoryEntryStream};
use crate::engine::wasi::io::{InputStream, OutputStream};
use crate::Ctx;

fn get_descriptor<'a>(
    table: &'a ResourceTable,
    fd: &'a Resource<Descriptor>,
) -> wasmtime::Result<&'a Descriptor> {
    table
        .get(fd)
        .context("failed to get descriptor resource from table")
}

impl types::Host for Ctx {
    fn filesystem_error_code(
        &mut self,
        err: Resource<wasi::io::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        todo!()
    }
}

impl types::HostDescriptor for Ctx {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> wasmtime::Result<Result<Resource<InputStream>, ErrorCode>> {
        todo!()
    }

    fn write_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> wasmtime::Result<Result<Resource<OutputStream>, ErrorCode>> {
        todo!()
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<Resource<OutputStream>, ErrorCode>> {
        todo!()
    }

    fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn sync_data(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn get_flags(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<DescriptorFlags, ErrorCode>> {
        todo!()
    }

    fn get_type(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<DescriptorType, ErrorCode>> {
        todo!()
    }

    fn set_size(
        &mut self,
        self_: Resource<Descriptor>,
        size: Filesize,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn set_times(
        &mut self,
        self_: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn read(
        &mut self,
        self_: Resource<Descriptor>,
        length: Filesize,
        offset: Filesize,
    ) -> wasmtime::Result<Result<(wasmtime::component::__internal::Vec<u8>, bool), ErrorCode>> {
        todo!()
    }

    fn write(
        &mut self,
        self_: Resource<Descriptor>,
        buffer: wasmtime::component::__internal::Vec<u8>,
        offset: Filesize,
    ) -> wasmtime::Result<Result<Filesize, ErrorCode>> {
        todo!()
    }

    fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<Resource<DirectoryEntryStream>, ErrorCode>> {
        todo!()
    }

    fn sync(&mut self, self_: Resource<Descriptor>) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn stat(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<DescriptorStat, ErrorCode>> {
        todo!()
    }

    fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<DescriptorStat, ErrorCode>> {
        todo!()
    }

    fn set_times_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: wasmtime::component::__internal::String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn link_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path_flags: PathFlags,
        old_path: wasmtime::component::__internal::String,
        new_descriptor: Resource<Descriptor>,
        new_path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn open_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: wasmtime::component::__internal::String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> wasmtime::Result<Result<Resource<Descriptor>, ErrorCode>> {
        todo!()
    }

    fn readlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<wasmtime::component::__internal::String, ErrorCode>> {
        todo!()
    }

    fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn rename_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: wasmtime::component::__internal::String,
        new_descriptor: Resource<Descriptor>,
        new_path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn symlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: wasmtime::component::__internal::String,
        new_path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn unlink_file_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        todo!()
    }

    fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        todo!()
    }

    fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> wasmtime::Result<Result<MetadataHashValue, ErrorCode>> {
        todo!()
    }

    fn metadata_hash_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: wasmtime::component::__internal::String,
    ) -> wasmtime::Result<Result<MetadataHashValue, ErrorCode>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<Descriptor>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete descriptor resource from table")?;
        Ok(())
    }
}

impl types::HostDirectoryEntryStream for Ctx {
    fn read_directory_entry(
        &mut self,
        self_: wasmtime::component::Resource<DirectoryEntryStream>,
    ) -> wasmtime::Result<Result<Option<DirectoryEntry>, ErrorCode>> {
        todo!()
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<DirectoryEntryStream>,
    ) -> wasmtime::Result<()> {
        todo!()
    }
}

impl preopens::Host for Ctx {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        let preopens = self.filesystem.preopens.clone();
        let mut results = Vec::with_capacity(preopens.len());
        for (dir, name) in preopens {
            let fd = self
                .table
                .push(Descriptor::Dir(dir))
                .with_context(|| format!("failed to push preopen {name}"))?;
            results.push((fd, name));
        }
        Ok(results)
    }
}
