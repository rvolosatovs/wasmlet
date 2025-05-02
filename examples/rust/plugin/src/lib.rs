use core::{
    ffi::{c_char, CStr},
    future::Future,
};

use bindings::wex_examples::db::handler::Database;
use wasmtime::component::{Linker, LinkerInstance, Resource, ResourceType};
use wex_plugin::wex;
use wex_plugin::wex::wasmtime;
use wex_plugin::wex::wasmtime::component::types;
use wex_plugin::wex::Ctx;
use wex_plugin::RUSTC_VERSION_C;

mod bindings {
    wex_plugin::wex::wasmtime::component::bindgen!();
}

#[no_mangle]
pub extern "C" fn wex_plugin_rustc_version() -> *const c_char {
    RUSTC_VERSION_C.expect("invalid `RUSTC_VERSION`").as_ptr()
}

#[no_mangle]
pub extern "C" fn wex_plugin_wex_version() -> *const c_char {
    wex::VERSION_C.expect("invalid `WEX_VERSION`").as_ptr()
}

impl bindings::wex_examples::db::handler::HostDatabase for Ctx {
    fn fetch_add(&mut self, self_: Resource<Database>) -> u64 {
        todo!()
    }

    fn drop(&mut self, rep: Resource<Database>) -> wasmtime::Result<()> {
        todo!()
    }
}

impl bindings::wex_examples::db::handler::Host for Ctx {
    fn connect(&mut self) -> Result<Resource<Database>, ()> {
        todo!()
    }
}

#[no_mangle]
pub fn wex_plugin_call(
    workload: *const c_char,
    instance: *const c_char,
    name: *const c_char,
    ty: *const c_void,
    store: *mut c_void,
    params: *const c_void,
    results: *mut c_void,
) -> bool {
    eprintln!("add to linker target: {}", unsafe {
        CStr::from_ptr(target).to_string_lossy()
    });
    eprintln!("add to linker instance: {}", unsafe {
        CStr::from_ptr(name).to_string_lossy()
    });
    eprintln!("instance type: {ty:?}, linking ");
    if linker
        .resource("database", ResourceType::host::<()>(), |_, _| Ok(()))
        .is_err()
    {
        return false;
    }
    //if bindings::wex_examples::db::handler::add_to_linker(linker, |x| x).is_err() {
    //    return false;
    //}
    eprintln!("done linking");
    //for (name, ty) in ty.exports(engine) {}
    true
}
