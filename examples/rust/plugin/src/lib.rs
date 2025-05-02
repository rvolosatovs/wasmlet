use core::ffi::{c_char, c_void, CStr};
use std::sync::{Mutex, OnceLock};

type FuncNewFn = unsafe extern "C" fn(
    linker: *mut c_void,
    name: *const c_char,
    f: extern "C" fn(
        store: *mut c_void,
        param_ptr: *const c_void,
        param_len: usize,
        result_ptr: *mut c_void,
        result_len: usize,
    ) -> bool,
    blocking: bool,
) -> bool;

//type FuncNewAsyncFn = unsafe extern "C" fn(
//    linker: *mut c_void,
//    name: *const c_char,
//    f: extern "C" fn(
//        store: *mut c_void,
//        param_ptr: *const c_void,
//        param_len: usize,
//        result_ptr: *mut c_void,
//        result_len: usize,
//    ) -> FfiFuture<bool>,
//) -> bool;

#[derive(Debug)]
#[repr(C, align(16))]
pub struct PluginConfig {
    pub version: u64,
    // TODO: Figure out config
    pub config: *const c_void,
    pub func_new: FuncNewFn,
    // TODO: figure out async
    pub func_new_async: unsafe extern "C" fn(),
    pub write_string: extern "C" fn(val: *mut c_void, data: *const c_char) -> bool,
}

pub struct Engine {
    pub func_new: FuncNewFn,
    // TODO: figure out async
    pub func_new_async: unsafe extern "C" fn(),
    pub write_string: extern "C" fn(val: *mut c_void, data: *const c_char) -> bool,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

#[no_mangle]
pub extern "C" fn wex_plugin_init(
    PluginConfig {
        version,
        config,
        func_new,
        func_new_async,
        write_string,
    }: PluginConfig,
) -> bool {
    eprintln!("{version} {config:?}");
    ENGINE
        .set(Engine {
            func_new,
            func_new_async,
            write_string,
        })
        .is_ok()
}

extern "C" fn handle_hello(
    store: *mut c_void,
    param_ptr: *const c_void,
    param_len: usize,
    result_ptr: *mut c_void,
    result_len: usize,
) -> bool {
    let Some(Engine {
        write_string,
        ..
    }) = ENGINE.get()
    else {
        return false;
    };
    write_string(result_ptr, c"hello from plugin".as_ptr())
}

#[no_mangle]
pub extern "C" fn wex_plugin_add_to_linker(
    cx: *const c_void,
    engine: *const c_void,
    linker: *mut c_void,
    name: *const c_char,
    ty: *const c_void,
) -> bool {
    let Some(Engine {
        func_new,
        func_new_async,
        ..
    }) = ENGINE.get()
    else {
        return false;
    };
    if name.is_null() {
        return false;
    }
    let name = unsafe { CStr::from_ptr(name) };
    match name.to_str() {
        Ok("wex-examples:hello/handler") => unsafe {
            func_new(linker, c"hello".as_ptr(), handle_hello, false)
        },
        Ok(name) => {
            eprintln!("attempted to link unknown instance {name}");
            false
        }
        Err(err) => {
            eprintln!(
                "instance name `{}` is not valid utf-8: {err}",
                name.to_string_lossy()
            );
            false
        }
    }
}
