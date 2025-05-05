use core::ptr::null;
use core::ptr::NonNull;
use core::{
    ffi::{c_char, c_void, CStr},
    ptr::null_mut,
};

use std::ffi::CString;
use std::path::Path;

use anyhow::{bail, Context as _};
use async_ffi::FfiFuture;
use libloading::{library_filename, Library};
use tracing::debug;
use wasmtime::component::{
    types::{self, ComponentItem},
    LinkerInstance, Val,
};
use wasmtime::StoreContextMut;

use crate::engine::Ctx;

mod fns {
    use super::*;

    //type PluginCallFn = unsafe fn(
    //    workload: *const c_char,
    //    instance: *const c_char,
    //    name: *const c_char,
    //    ty: *const types::ComponentFunc,
    //    params: *const Val,
    //    results: *mut Val,
    //) -> bool;

    pub type InitPlugin = unsafe extern "C" fn(conf: PluginConfig) -> bool;

    pub type AddToLinker = unsafe extern "C" fn(
        cx: *const AddToLinkerCtx<'_>,
        engine: *const wasmtime::Engine,
        linker: *mut LinkerInstance<Ctx>,
        instance: *const c_char,
        ty: *const types::ComponentInstance,
    ) -> bool;

    pub type FuncNew = extern "C" fn(
        linker: *mut LinkerInstance<Ctx>,
        name: *const c_char,
        f: unsafe extern "C" fn(
            store: *mut StoreContextMut<'_, Ctx>,
            param_ptr: *const Val,
            param_len: usize,
            result_ptr: *mut Val,
            result_len: usize,
        ) -> bool,
        blocking: bool,
    ) -> bool;

    pub type FuncNewAsync = extern "C" fn(
        linker: *mut LinkerInstance<Ctx>,
        name: *const c_char,
        f: unsafe extern "C" fn(
            store: *mut StoreContextMut<'_, Ctx>,
            param_ptr: *const Val,
            param_len: usize,
            result_ptr: *mut Val,
            result_len: usize,
        ) -> FfiFuture<bool>,
    ) -> bool;
}

pub enum AddToLinkerCtx<'a> {
    Workload { name: &'a str },
    Service { name: &'a str },
}

// Align to 16 for future-proofness.
// Better safe than sorry!
#[repr(C, align(16))]
struct PluginConfig {
    pub version: u64,
    // TODO: Figure out config
    pub config: *const c_void,
    pub func_new: fns::FuncNew,
    pub func_new_async: fns::FuncNewAsync,
    pub write_string: extern "C" fn(val: *mut Val, data: *const c_char) -> bool,
}

pub struct Plugin {
    add_to_linker: Option<fns::AddToLinker>,
    lib: Library,
}

impl Plugin {
    pub fn load(src: impl AsRef<Path>) -> anyhow::Result<Self> {
        let src = src.as_ref();
        let lib =
            if src.has_root() || src.extension().is_some() || src.parent() != Some(Path::new("")) {
                unsafe { Library::new(src) }
            } else {
                unsafe { Library::new(library_filename(src)) }
            }
            .context("failed to load dynamic library")?;

        let init_plugin = unsafe { lib.get::<fns::InitPlugin>(b"wex_plugin_init") }
            .context("failed to lookup `wex_plugin_init`")?;
        if !unsafe {
            init_plugin(PluginConfig {
                version: 0,
                // TODO: Set config
                config: null(),
                func_new,
                func_new_async,
                write_string,
            })
        } {
            bail!("failed to initialize plugin");
        }

        let add_to_linker =
            match unsafe { lib.get::<fns::AddToLinker>(b"wex_plugin_add_to_linker") } {
                Ok(add_to_linker) => Some(*add_to_linker),
                Err(err) => {
                    debug!(?err, "failed to lookup `wex_plugin_add_to_linker`");
                    None
                }
            };
        Ok(Self { lib, add_to_linker })
    }

    pub fn add_to_linker(
        &self,
        engine: &wasmtime::Engine,
        linker: &mut LinkerInstance<Ctx>,
        cx: &AddToLinkerCtx,
        instance_name: &str,
        ty: &types::ComponentInstance,
    ) -> anyhow::Result<()> {
        let Plugin { add_to_linker, lib } = self;
        let instance_name_c =
            CString::new(instance_name).context("failed to construct C string")?;
        if let Some(add_to_linker) = add_to_linker {
            if !unsafe { add_to_linker(cx, engine, linker, instance_name_c.as_ptr(), ty) } {
                bail!("failed to link `{instance_name}`")
            }
            return Ok(());
        }
        for (name, ty) in ty.exports(engine) {
            match ty {
                ComponentItem::ComponentFunc(ty) => {
                    let symbol = format!("{instance_name}#{name}");
                    let f = unsafe {
                        lib.get::<unsafe extern "C" fn() -> *const u8>(symbol.as_bytes())
                    }
                    .with_context(|| format!("failed to lookup `{symbol}` in plugin `{target}`"))?;
                    let f = *f;
                    linker
                        .func_wrap::<_, (), (FfiReturn, FfiReturn)>(name, move |store, params| {
                            todo!()
                            //let ptr = unsafe { f() };
                            //Ok((ptr,))
                            //let ptr = unsafe { f() }.cast::<(*const u8, usize)>();
                            //let (ptr, len) = unsafe { *ptr };
                            //let s = unsafe { slice::from_raw_parts(ptr, len) };
                            //Ok((String::from_utf8_lossy(s).to_string(),))
                        })
                        .with_context(|| format!("failed to define function `{name}`"))?;
                }
                ComponentItem::Resource(ty) => {
                    bail!("res")
                }
                ComponentItem::CoreFunc(ty) => {}
                ComponentItem::Module(module) => {}
                ComponentItem::Component(component) => {}
                ComponentItem::ComponentInstance(component_instance) => {}
                ComponentItem::Type(_) => {}
            }
        }
        Ok(())
    }
}

extern "C" fn func_new(
    linker: *mut LinkerInstance<Ctx>,
    name: *const c_char,
    f: unsafe extern "C" fn(
        store: *mut StoreContextMut<'_, Ctx>,
        param_ptr: *const Val,
        param_len: usize,
        result_ptr: *mut Val,
        result_len: usize,
    ) -> bool,
    blocking: bool,
) -> bool {
    let Some(mut linker) = NonNull::new(linker) else {
        return false;
    };
    if name.is_null() {
        return false;
    }
    let name = unsafe { CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        return false;
    };
    let linker = unsafe { linker.as_mut() };
    if !blocking {
        linker
            .func_new(name, move |mut store, params, results| {
                // TODO: lower params
                if !unsafe {
                    f(
                        &mut store,
                        params.as_ptr(),
                        params.len(),
                        results.as_mut_ptr(),
                        results.len(),
                    )
                } {
                    bail!("failed to call plugin")
                }
                // TODO: lift results
                Ok(())
            })
            .is_ok()
    } else {
        linker
            .func_new_async(name, move |store, params, results| {
                let f = f;
                Box::new(async move {
                    eprintln!("hello {f:?}");
                    Ok(())
                })
            })
            .is_ok()
    }
}

extern "C" fn func_new_async(
    linker: *mut LinkerInstance<Ctx>,
    name: *const c_char,
    f: unsafe extern "C" fn(
        store: *mut StoreContextMut<'_, Ctx>,
        param_ptr: *const Val,
        param_len: usize,
        result_ptr: *mut Val,
        result_len: usize,
    ) -> FfiFuture<bool>,
) -> bool {
    todo!("func_new_async");
    false
}

extern "C" fn write_char(val: *mut Val, data: u32) -> bool {
    let Some(val) = NonNull::new(val) else {
        return false;
    };
    let data = char::from_u32(data).with_context(|| format!("`{data}` is not a valid char"))?;
    let data = Val::Char(data);
    unsafe { val.write(data) };
    true
}

extern "C" fn write_string(val: *mut Val, data: *const c_char) -> bool {
    let Some(val) = NonNull::new(val) else {
        return false;
    };
    if data.is_null() {
        return false;
    }
    let data = unsafe { CStr::from_ptr(data) };
    let Ok(data) = data.to_str() else {
        return false;
    };
    let data = Val::String(data.into());
    unsafe { val.write(data) };
    true
}

extern "C" fn make_list(val: *mut Val, len: usize) -> *mut Val {
    let mut vals = vec![Val::Bool(false); len];
    let ptr = vals.as_mut_ptr();
    let data = Val::List(vals);
    unsafe { val.write(data) };
    ptr
}

//extern "C" fn make_record(val: *mut Val, len: usize) -> *mut (String, Val) {
//    let mut vals = vec![(String::default(), Val::Bool(false)); len];
//    let ptr = vals.as_mut_ptr();
//    let data = Val::Record(vals);
//    unsafe { val.write(data) };
//    ptr
//}

//extern "C" fn write_record_field(val: *mut (String, Val), name: *const c_char) -> *mut Val {
//    let Some(val) = NonNull::new(val) else {
//        return null_mut();
//    };
//    if name.is_null() {
//        return null_mut();
//    }
//    let name = unsafe { CStr::from_ptr(name) };
//    let Ok(name) = name.to_str() else {
//        return null_mut();
//    };
//
//    unsafe { val.write(data) };
//    true
//}

extern "C" fn make_tuple(val: *mut Val, len: usize) -> *mut Val {
    let mut vals = vec![Val::Bool(false); len];
    let ptr = vals.as_mut_ptr();
    let data = Val::Tuple(vals);
    unsafe { val.write(data) };
    ptr
}
