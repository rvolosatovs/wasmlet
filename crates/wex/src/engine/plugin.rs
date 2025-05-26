use core::ffi::{c_char, c_void, CStr};
use core::ptr::{null, slice_from_raw_parts, NonNull};

use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use async_ffi::FfiFuture;
use libloading::{library_filename, Library, Symbol};
use tracing::debug;
use wasmtime::component::{types, Lift, LinkerInstance, Lower, ResourceTable, Val};
use wasmtime::component::{types::ComponentItem, Type};
use wasmtime::StoreContextMut;
use wasmtime_cabish::{lift_results, CabishView};

use crate::engine::Ctx;

impl CabishView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

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

fn dlsym<'a, T>(lib: &'a Library, symbol: &str) -> anyhow::Result<Symbol<'a, T>> {
    unsafe { lib.get::<T>(symbol.as_bytes()) }
        .with_context(|| format!("failed to lookup `{symbol}`"))
}

fn link_0_1<'a, T: Lower + 'static>(
    linker: &mut LinkerInstance<Ctx>,
    lib: &'a Library,
    symbol: &str,
    name: &str,
) -> anyhow::Result<()> {
    let f = dlsym::<unsafe extern "C" fn() -> *const T>(lib, &symbol)?;
    let f = *f;
    linker.func_wrap(name, move |_, ()| {
        let ptr = unsafe { f() };
        Ok((unsafe { ptr.read() },))
    })
}

fn link_1_0<'a, T: Lift + 'static>(
    linker: &mut LinkerInstance<Ctx>,
    lib: &'a Library,
    symbol: &str,
    name: &str,
) -> anyhow::Result<()> {
    let f = dlsym::<unsafe extern "C" fn(T)>(lib, &symbol)?;
    let f = *f;
    linker.func_wrap(name, move |_, (v,)| {
        unsafe { f(v) };
        Ok(())
    })
}

fn link_func(
    linker: &mut LinkerInstance<Ctx>,
    lib: &Library,
    ty: &types::ComponentFunc,
    instance_name: &str,
    name: &str,
) -> anyhow::Result<()> {
    let param_tys = ty.params().map(|(_, ty)| ty).collect::<Arc<[_]>>();
    let result_tys = ty.results().collect::<Arc<[_]>>();
    let symbol = format!("{instance_name}#{name}");
    match (&*param_tys, &*result_tys) {
        ([], []) => {
            let f = dlsym::<unsafe extern "C" fn()>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                unsafe { f() };
                Ok(())
            })
        }
        ([], [Type::Bool]) => {
            let f = dlsym::<unsafe extern "C" fn() -> *const u8>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                let ptr = unsafe { f() };
                let data = unsafe { ptr.read() };
                Ok((data != 0,))
            })
        }
        ([], [Type::S8]) => link_0_1::<i8>(linker, lib, &symbol, name),
        ([], [Type::U8]) => link_0_1::<u8>(linker, lib, &symbol, name),
        ([], [Type::S16]) => link_0_1::<i16>(linker, lib, &symbol, name),
        ([], [Type::U16]) => link_0_1::<u16>(linker, lib, &symbol, name),
        ([], [Type::S32]) => link_0_1::<i32>(linker, lib, &symbol, name),
        ([], [Type::U32]) => link_0_1::<u32>(linker, lib, &symbol, name),
        ([], [Type::S64]) => link_0_1::<i64>(linker, lib, &symbol, name),
        ([], [Type::U64]) => link_0_1::<u64>(linker, lib, &symbol, name),
        ([], [Type::Float32]) => link_0_1::<f32>(linker, lib, &symbol, name),
        ([], [Type::Float64]) => link_0_1::<f64>(linker, lib, &symbol, name),
        ([], [Type::Char]) => {
            let f = dlsym::<unsafe extern "C" fn() -> *const u32>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                let ptr = unsafe { f() };
                let data = unsafe { ptr.read() };
                let data = char::from_u32(data)
                    .with_context(|| format!("`{data}` is not a valid char"))?;
                Ok((data,))
            })
        }
        ([], [Type::String]) => {
            let f = dlsym::<unsafe extern "C" fn() -> *const (*mut u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                let ptr = unsafe { f() };
                let (data, len) = unsafe { ptr.read() };
                if len > 0 {
                    let data = slice_from_raw_parts(data, len);
                    let data = String::from_utf8_lossy(unsafe { &*data });
                    Ok((data.into(),))
                } else {
                    Ok((String::default(),))
                }
            })
        }
        // TODO: optimize lists of integers/bytes
        // TODO: optimize resources
        ([], [..]) => {
            let f = dlsym::<unsafe extern "C" fn() -> *const c_void>(lib, &symbol)?;
            let f = *f;
            linker.func_new(name, move |store, _, results| {
                let ptr = unsafe { f() };
                lift_results(store, &result_tys, ptr, results)
            })
        }
        ([Type::Bool], []) => {
            let f = dlsym::<unsafe extern "C" fn(u8)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, (v,)| {
                unsafe { f(if v { 1 } else { 0 }) };
                Ok(())
            })
        }
        ([Type::S8], []) => link_1_0::<i8>(linker, lib, &symbol, name),
        ([Type::U8], []) => link_1_0::<u8>(linker, lib, &symbol, name),
        ([Type::S16], []) => link_1_0::<i16>(linker, lib, &symbol, name),
        ([Type::U16], []) => link_1_0::<u16>(linker, lib, &symbol, name),
        ([Type::S32], []) => link_1_0::<i32>(linker, lib, &symbol, name),
        ([Type::U32], []) => link_1_0::<u32>(linker, lib, &symbol, name),
        ([Type::S64], []) => link_1_0::<i64>(linker, lib, &symbol, name),
        ([Type::U64], []) => link_1_0::<u64>(linker, lib, &symbol, name),
        ([Type::Float32], []) => link_1_0::<f32>(linker, lib, &symbol, name),
        ([Type::Float64], []) => link_1_0::<f64>(linker, lib, &symbol, name),
        ([Type::Char], []) => link_1_0::<char>(linker, lib, &symbol, name),
        ([Type::String], []) => {
            let f = dlsym::<unsafe extern "C" fn(*const u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, (s,): (String,)| {
                unsafe { f(s.as_ptr(), s.len()) };
                Ok(())
            })
        }
        // TODO: optimize lists of integers/bytes
        // TODO: optimize resources
        ([..], []) => {
            bail!("TODO")
        }
        ([..], [..]) => {
            bail!("TODO")
        }
    }
    .with_context(|| format!("failed to define function `{name}`"))
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

        match unsafe { lib.get::<fns::InitPlugin>(b"wex_plugin_init") } {
            Ok(init_plugin) => {
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
            }
            Err(err) => {
                debug!(?err, "failed to lookup `wex_plugin_init`");
            }
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
                        lib.get::<unsafe extern "C" fn() -> *const c_void>(symbol.as_bytes())
                    }
                    .with_context(|| format!("failed to lookup `{symbol}`"))?;
                    let f = *f;
                    if ty.params().len() != 0 {
                        bail!("params not supported yet");
                    }
                    let result_tys = ty.results().collect::<Vec<_>>();
                    linker
                        .func_new(name, move |store, params, results| {
                            let ptr = unsafe { f() };
                            lift_results(store, &result_tys, ptr, results)
                        })
                        .with_context(|| format!("failed to define function `{name}`"))?;
                }
                ComponentItem::Resource(ty) => bail!("resources not supported yet"),
                ComponentItem::CoreFunc(..)
                | ComponentItem::Module(..)
                | ComponentItem::Component(..)
                | ComponentItem::ComponentInstance(..)
                | ComponentItem::Type(_) => {}
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
    todo!();
    //let data = char::from_u32(data).with_context(|| format!("`{data}` is not a valid char"))?;
    //let data = Val::Char(data);
    //unsafe { val.write(data) };
    //true
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
