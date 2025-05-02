use core::ffi::{c_char, c_void, CStr};

use async_ffi::{FfiFuture, FutureExt as _};

pub const VERSION: &CStr = c"0.1.0";

static mut X: usize = 0;

#[no_mangle]
pub extern "C" fn hello() -> usize {
    unsafe { X += 1 };
    unsafe { X }
}

extern "C" fn wex_init(version: *const c_char, config: *const c_void) -> bool {
    false
}

extern "C" fn wex_call(
    target: *const c_char,
    params: *const c_void,
    params_len: usize,
    results: *mut c_void,
    results_len: usize,
) -> bool {
    false
}

extern "C" fn wex_call_async(
    target: *const c_char,
    params: *const c_void,
    params_len: usize,
    results: *mut c_void,
    results_len: usize,
) -> FfiFuture<bool> {
    async { false }.into_ffi()
}

extern "C" fn add_to_linker(linker: *mut c_void, target: *const c_char) -> bool {
    false
}

extern "C" fn func_new_async(
    linker: *mut c_void,
    name: *const c_char,
    f: unsafe extern "C" fn(
        store: *mut c_void,
        params: *const c_void,
        results: *mut c_void,
    ) -> FfiFuture<bool>,
) -> bool {
    false
}
