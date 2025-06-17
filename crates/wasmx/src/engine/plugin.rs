use core::ffi::c_void;
use core::ptr::slice_from_raw_parts;
use core::str;

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use libloading::{library_filename, Library, Symbol};
use wasmtime::component::types::{self, ComponentItem};
use wasmtime::component::{Lift, LinkerInstance, Lower, ResourceTable, Type};
use wasmtime_cabish::{lift_results, CabishView};

use crate::engine::Ctx;

impl CabishView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

pub struct Plugin {
    lib: Library,
}

fn dlsym<'a, T>(lib: &'a Library, symbol: &str) -> anyhow::Result<Symbol<'a, T>> {
    unsafe { lib.get::<T>(symbol.as_bytes()) }
        .with_context(|| format!("failed to lookup `{symbol}`"))
}

// TODO: use this for optimization
//fn is_direct_ty(ty: &Type) -> bool {
//    match ty {
//        Type::Bool => true,
//        Type::S8 => true,
//        Type::U8 => true,
//        Type::S16 => true,
//        Type::U16 => true,
//        Type::S32 => true,
//        Type::U32 => true,
//        Type::S64 => true,
//        Type::U64 => true,
//        Type::Float32 => true,
//        Type::Float64 => true,
//        Type::Char => true,
//        Type::String => true,
//        Type::List(ty) => is_direct_ty(&ty.ty()),
//        Type::Record(ty) => {
//            let mut fields = ty.fields();
//            match (fields.next(), fields.next()) {
//                (None, None) => true,
//                (Some(ty), None) => is_direct_ty(&ty.ty),
//                _ => false,
//            }
//        }
//        Type::Tuple(ty) => {
//            let mut types = ty.types();
//            match (types.next(), types.next()) {
//                (None, None) => true,
//                (Some(ty), None) => is_direct_ty(&ty),
//                _ => false,
//            }
//        }
//        Type::Variant(_ty) => false, // TODO: introspect type
//        Type::Enum(..) => true,
//        Type::Option(ty) => is_direct_ty(&ty.ty()),
//        Type::Result(ty) => {
//            ty.ok().as_ref().map(is_direct_ty).unwrap_or(true)
//                && ty.err().as_ref().map(is_direct_ty).unwrap_or(true)
//        }
//        Type::Flags(..) => true,
//        Type::Own(..) => true,
//        Type::Borrow(..) => true,
//    }
//}

fn link_func_0_1<T: Lower + 'static>(
    linker: &mut LinkerInstance<impl CabishView>,
    lib: &Library,
    symbol: &str,
    name: &str,
) -> anyhow::Result<()> {
    let f = dlsym::<unsafe extern "C" fn() -> *const T>(lib, symbol)?;
    let f = *f;
    linker.func_wrap(name, move |_, ()| {
        let ptr = unsafe { f() };
        Ok((unsafe { ptr.read() },))
    })
}

fn link_func_1_0<T: Lift + 'static>(
    linker: &mut LinkerInstance<impl CabishView>,
    lib: &Library,
    symbol: &str,
    name: &str,
) -> anyhow::Result<()> {
    let f = dlsym::<unsafe extern "C" fn(T)>(lib, symbol)?;
    let f = *f;
    linker.func_wrap(name, move |_, (v,)| {
        unsafe { f(v) };
        Ok(())
    })
}

fn link_func(
    linker: &mut LinkerInstance<impl CabishView>,
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
        ([], [Type::S8]) => link_func_0_1::<i8>(linker, lib, &symbol, name),
        ([], [Type::U8]) => link_func_0_1::<u8>(linker, lib, &symbol, name),
        ([], [Type::S16]) => link_func_0_1::<i16>(linker, lib, &symbol, name),
        ([], [Type::U16]) => link_func_0_1::<u16>(linker, lib, &symbol, name),
        ([], [Type::S32]) => link_func_0_1::<i32>(linker, lib, &symbol, name),
        ([], [Type::U32]) => link_func_0_1::<u32>(linker, lib, &symbol, name),
        ([], [Type::S64]) => link_func_0_1::<i64>(linker, lib, &symbol, name),
        ([], [Type::U64]) => link_func_0_1::<u64>(linker, lib, &symbol, name),
        ([], [Type::Float32]) => link_func_0_1::<f32>(linker, lib, &symbol, name),
        ([], [Type::Float64]) => link_func_0_1::<f64>(linker, lib, &symbol, name),
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
            let f = dlsym::<unsafe extern "C" fn() -> *const (*const u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                let ptr = unsafe { f() };
                let (data, len) = unsafe { ptr.read() };
                if len > 0 {
                    let data = slice_from_raw_parts(data, len);
                    Ok((unsafe { str::from_utf8_unchecked(&*data) },))
                } else {
                    Ok(("",))
                }
            })
        }
        ([], [Type::List(ty)]) if ty.ty() == Type::U8 => {
            let f = dlsym::<unsafe extern "C" fn() -> *const (*const u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, ()| {
                let ptr = unsafe { f() };
                let (data, len) = unsafe { ptr.read() };
                if len > 0 {
                    let data = slice_from_raw_parts(data, len);
                    Ok((unsafe { &*data },))
                } else {
                    Ok((&[],))
                }
            })
        }
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
        ([Type::S8], []) => link_func_1_0::<i8>(linker, lib, &symbol, name),
        ([Type::U8], []) => link_func_1_0::<u8>(linker, lib, &symbol, name),
        ([Type::S16], []) => link_func_1_0::<i16>(linker, lib, &symbol, name),
        ([Type::U16], []) => link_func_1_0::<u16>(linker, lib, &symbol, name),
        ([Type::S32], []) => link_func_1_0::<i32>(linker, lib, &symbol, name),
        ([Type::U32], []) => link_func_1_0::<u32>(linker, lib, &symbol, name),
        ([Type::S64], []) => link_func_1_0::<i64>(linker, lib, &symbol, name),
        ([Type::U64], []) => link_func_1_0::<u64>(linker, lib, &symbol, name),
        ([Type::Float32], []) => link_func_1_0::<f32>(linker, lib, &symbol, name),
        ([Type::Float64], []) => link_func_1_0::<f64>(linker, lib, &symbol, name),
        ([Type::Char], []) => link_func_1_0::<char>(linker, lib, &symbol, name),
        ([Type::String], []) => {
            let f = dlsym::<unsafe extern "C" fn(*const u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, (s,): (Box<str>,)| {
                unsafe { f(s.as_ptr(), s.len()) };
                Ok(())
            })
        }
        ([Type::List(ty)], []) if ty.ty() == Type::U8 => {
            let f = dlsym::<unsafe extern "C" fn(*const u8, usize)>(lib, &symbol)?;
            let f = *f;
            linker.func_wrap(name, move |_, (s,): (Box<[u8]>,)| {
                unsafe { f(s.as_ptr(), s.len()) };
                Ok(())
            })
        }
        // TODO: optimize resources
        ([..], []) => {
            bail!("TODO: cranelift-jit")
        }
        ([..], [..]) => {
            bail!("TODO: cranelift-jit")
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
        Ok(Self { lib })
    }

    pub fn add_to_linker(
        &self,
        engine: &wasmtime::Engine,
        linker: &mut LinkerInstance<impl CabishView>,
        instance_name: &str,
        ty: &types::ComponentInstance,
    ) -> anyhow::Result<()> {
        let Plugin { lib } = self;
        for (name, ty) in ty.exports(engine) {
            match ty {
                ComponentItem::ComponentFunc(ty) => {
                    link_func(linker, lib, &ty, instance_name, name)
                        .with_context(|| format!("failed to define function `{name}`"))?;
                }
                ComponentItem::Resource(_ty) => bail!("resources not supported yet"),
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
