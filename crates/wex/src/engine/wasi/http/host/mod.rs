use core::ops::{Deref, DerefMut};

use anyhow::Context as _;
use wasmtime::component::{Resource, ResourceTable};

use crate::engine::WithChildren;

mod handler;
mod types;

fn get_fields<'a>(
    table: &'a ResourceTable,
    fields: &Resource<WithChildren<http::HeaderMap>>,
) -> wasmtime::Result<&'a WithChildren<http::HeaderMap>> {
    table
        .get(&fields)
        .context("failed to get fields from table")
}

fn get_fields_inner<'a>(
    table: &'a ResourceTable,
    fields: &Resource<WithChildren<http::HeaderMap>>,
) -> wasmtime::Result<impl Deref<Target = http::HeaderMap> + use<'a>> {
    let fields = get_fields(table, fields)?;
    fields.get()
}

fn get_fields_mut<'a>(
    table: &'a mut ResourceTable,
    fields: &Resource<WithChildren<http::HeaderMap>>,
) -> wasmtime::Result<&'a mut WithChildren<http::HeaderMap>> {
    table
        .get_mut(&fields)
        .context("failed to get fields from table")
}

fn get_fields_inner_mut<'a>(
    table: &'a mut ResourceTable,
    fields: &Resource<WithChildren<http::HeaderMap>>,
) -> wasmtime::Result<Option<impl DerefMut<Target = http::HeaderMap> + use<'a>>> {
    let fields = get_fields_mut(table, fields)?;
    fields.get_mut()
}

fn push_fields(
    table: &mut ResourceTable,
    fields: WithChildren<http::HeaderMap>,
) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
    table.push(fields).context("failed to push fields to table")
}

fn push_fields_child<T: 'static>(
    table: &mut ResourceTable,
    fields: WithChildren<http::HeaderMap>,
    parent: &Resource<T>,
) -> wasmtime::Result<Resource<WithChildren<http::HeaderMap>>> {
    table
        .push_child(fields, parent)
        .context("failed to push fields to table")
}

fn delete_fields(
    table: &mut ResourceTable,
    fields: Resource<WithChildren<http::HeaderMap>>,
) -> wasmtime::Result<WithChildren<http::HeaderMap>> {
    table
        .delete(fields)
        .context("failed to delete fields from table")
}

fn get_request<'a, T: 'static>(
    table: &'a ResourceTable,
    req: &Resource<T>,
) -> wasmtime::Result<&'a T> {
    table.get(req).context("failed to get request from table")
}

fn get_request_mut<'a, T: 'static>(
    table: &'a mut ResourceTable,
    req: &Resource<T>,
) -> wasmtime::Result<&'a mut T> {
    table
        .get_mut(req)
        .context("failed to get request from table")
}

fn push_request<T: Send + 'static>(
    table: &mut ResourceTable,
    req: T,
) -> wasmtime::Result<Resource<T>> {
    table.push(req).context("failed to push request to table")
}

fn delete_request<T: 'static>(table: &mut ResourceTable, req: Resource<T>) -> wasmtime::Result<T> {
    table
        .delete(req)
        .context("failed to delete request from table")
}

fn get_response<'a, T: 'static>(
    table: &'a ResourceTable,
    res: &Resource<T>,
) -> wasmtime::Result<&'a T> {
    table.get(res).context("failed to get response from table")
}

fn get_response_mut<'a, T: 'static>(
    table: &'a mut ResourceTable,
    res: &Resource<T>,
) -> wasmtime::Result<&'a mut T> {
    table
        .get_mut(res)
        .context("failed to get response from table")
}

fn push_response<T: Send + 'static>(
    table: &mut ResourceTable,
    res: T,
) -> wasmtime::Result<Resource<T>> {
    table.push(res).context("failed to push response to table")
}

fn delete_response<T: 'static>(table: &mut ResourceTable, res: Resource<T>) -> wasmtime::Result<T> {
    table
        .delete(res)
        .context("failed to delete response from table")
}

fn get_body<'a, T: 'static>(
    table: &'a ResourceTable,
    body: &Resource<T>,
) -> wasmtime::Result<&'a T> {
    table.get(body).context("failed to get body from table")
}

fn get_body_mut<'a, T: 'static>(
    table: &'a mut ResourceTable,
    body: &Resource<T>,
) -> wasmtime::Result<&'a mut T> {
    table.get_mut(body).context("failed to get body from table")
}

fn push_body<T: Send + 'static>(
    table: &mut ResourceTable,
    body: T,
) -> wasmtime::Result<Resource<T>> {
    table.push(body).context("failed to push body to table")
}

fn delete_body<T: 'static>(table: &mut ResourceTable, body: Resource<T>) -> wasmtime::Result<T> {
    table
        .delete(body)
        .context("failed to delete body from table")
}
