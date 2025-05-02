use core::ffi::{c_char, c_void, CStr, FromBytesWithNulError};

use async_ffi::FfiFuture;

pub use wex;

pub const RUSTC_VERSION: &'static str = env!("RUSTC_VERSION");

pub const RUSTC_VERSION_C: Result<&'static CStr, FromBytesWithNulError> =
    CStr::from_bytes_with_nul(concat!(env!("RUSTC_VERSION"), "\0").as_bytes());

#[repr(C)]
pub struct Workload {
    pub call: extern "C" fn(*mut Self, *const c_char) -> FfiFuture<()>,
    pub call_wasi_http_handler:
        extern "C" fn(*mut Self, *const HttpRequest, *mut HttpResponse) -> FfiFuture<()>,
}

#[repr(C)]
pub struct Stream<T> {
    pub read: extern "C" fn(*mut Self, *mut T, u64) -> FfiFuture<u64>,
}

#[repr(C)]
pub struct HttpRequest {
    body: Stream<u8>,
}

#[repr(C)]
pub struct HttpResponse {}

#[repr(C)]
pub struct Engine {}

pub extern "C" fn init(engine: *const Engine, config: *const c_void) {}

pub extern "C" fn register_workload(workload: *const Workload, id: *const c_char) {}
pub extern "C" fn deregister_workload(workload: *const Workload) {}

//pub extern "C" fn call (*mut Workload, *const c_char) -> FfiFuture<()> {}
//pub extern "C" fn call_wasi_http_handler(*mut Workload, *const HttpRequest, *mut HttpResponse) -> FfiFuture<()> {}
