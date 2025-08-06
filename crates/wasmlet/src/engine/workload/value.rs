use anyhow::bail;
use tracing::debug;
use wasmtime::component::Val;
use wasmtime::AsContextMut;

use crate::engine::{is_host_resource_type, resource_types, ResourceView};

pub fn lower<T: ResourceView + 'static, U: ResourceView + 'static>(
    store: &mut impl AsContextMut<Data = T>,
    target_store: &mut impl AsContextMut<Data = U>,
    v: &Val,
) -> anyhow::Result<Val> {
    match v {
        &Val::Bool(v) => Ok(Val::Bool(v)),
        &Val::S8(v) => Ok(Val::S8(v)),
        &Val::U8(v) => Ok(Val::U8(v)),
        &Val::S16(v) => Ok(Val::S16(v)),
        &Val::U16(v) => Ok(Val::U16(v)),
        &Val::S32(v) => Ok(Val::S32(v)),
        &Val::U32(v) => Ok(Val::U32(v)),
        &Val::S64(v) => Ok(Val::S64(v)),
        &Val::U64(v) => Ok(Val::U64(v)),
        &Val::Float32(v) => Ok(Val::Float32(v)),
        &Val::Float64(v) => Ok(Val::Float64(v)),
        &Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v.clone())),
        Val::List(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, target_store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .iter()
                .map(|(name, v)| {
                    let v = lower(store, target_store, v)?;
                    Ok((name.clone(), v))
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, target_store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lower(store, target_store, v)?;
                Ok(Val::Variant(k.clone(), Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k.clone(), None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v.clone())),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lower(store, target_store, v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lower(store, target_store, v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lower(store, target_store, v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v.clone())),
        &Val::Resource(mut any) => {
            let mut store = store.as_context_mut();
            if let Ok(res) = any.try_into_resource::<resource_types::Pollable>(&mut store) {
                debug!("lowering pollable");
                let table = store.data_mut().table();

                let mut target_store = target_store.as_context_mut();
                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = table.delete(res)?;
                    let res = target_table.push(res)?;
                    any = res.try_into_resource_any(target_store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) = any.try_into_resource::<resource_types::InputStream>(&mut store)
            {
                debug!("lowering input stream");
                let table = store.data_mut().table();

                let mut target_store = target_store.as_context_mut();
                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = table.delete(res)?;
                    let res = target_table.push(res)?;
                    any = res.try_into_resource_any(target_store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) =
                any.try_into_resource::<resource_types::OutputStream>(&mut store)
            {
                debug!("lowering output stream");
                let table = store.data_mut().table();

                let mut target_store = target_store.as_context_mut();
                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = table.delete(res)?;
                    let res = target_table.push(res)?;
                    any = res.try_into_resource_any(target_store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) = any.try_into_resource::<resource_types::TcpSocket>(&mut store) {
                debug!("lowering tcp socket");
                let table = store.data_mut().table();

                let mut target_store = target_store.as_context_mut();
                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = table.delete(res)?;
                    let res = target_table.push(res)?;
                    any = res.try_into_resource_any(target_store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
                // TODO: support more host resource types
            } else if let Ok(res) = any.try_into_resource(&mut store) {
                debug!("lowering guest resource");
                let table = store.data_mut().table();
                if res.owned() {
                    any = table.delete(res)?;
                } else {
                    any = table.get(&res).cloned()?;
                }
            } else {
                debug_assert!(!is_host_resource_type(any.ty()));
            }
            Ok(Val::Resource(any))
        }
    }
}

pub fn lift<T: ResourceView + 'static, U: ResourceView + 'static>(
    store: &mut impl AsContextMut<Data = T>,
    target_store: &mut impl AsContextMut<Data = U>,
    v: Val,
) -> anyhow::Result<Val> {
    match v {
        Val::Bool(v) => Ok(Val::Bool(v)),
        Val::S8(v) => Ok(Val::S8(v)),
        Val::U8(v) => Ok(Val::U8(v)),
        Val::S16(v) => Ok(Val::S16(v)),
        Val::U16(v) => Ok(Val::U16(v)),
        Val::S32(v) => Ok(Val::S32(v)),
        Val::U32(v) => Ok(Val::U32(v)),
        Val::S64(v) => Ok(Val::S64(v)),
        Val::U64(v) => Ok(Val::U64(v)),
        Val::Float32(v) => Ok(Val::Float32(v)),
        Val::Float64(v) => Ok(Val::Float64(v)),
        Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v)),
        Val::List(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, target_store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .into_iter()
                .map(|(name, v)| {
                    let v = lift(store, target_store, v)?;
                    Ok((name, v))
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, target_store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lift(store, target_store, *v)?;
                Ok(Val::Variant(k, Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k, None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v.clone())),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lift(store, target_store, *v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lift(store, target_store, *v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lift(store, target_store, *v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v)),
        Val::Resource(mut any) => {
            let mut store = store.as_context_mut();
            let mut target_store = target_store.as_context_mut();
            if let Ok(res) = any.try_into_resource::<resource_types::Pollable>(&mut target_store) {
                debug!("lifting pollable");
                let table = store.data_mut().table();

                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = target_table.delete(res)?;
                    let res = table.push(res)?;
                    any = res.try_into_resource_any(store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) =
                any.try_into_resource::<resource_types::InputStream>(&mut target_store)
            {
                debug!("lifting input stream");
                let table = store.data_mut().table();

                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = target_table.delete(res)?;
                    let res = table.push(res)?;
                    any = res.try_into_resource_any(store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) =
                any.try_into_resource::<resource_types::OutputStream>(&mut target_store)
            {
                debug!("lifting output stream");
                let table = store.data_mut().table();

                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = target_table.delete(res)?;
                    let res = table.push(res)?;
                    any = res.try_into_resource_any(store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else if let Ok(res) =
                any.try_into_resource::<resource_types::TcpSocket>(&mut target_store)
            {
                debug!("lifting tcp socket");
                let table = store.data_mut().table();

                let target_table = target_store.data_mut().table();
                if res.owned() {
                    let res = target_table.delete(res)?;
                    let res = table.push(res)?;
                    any = res.try_into_resource_any(store)?;
                } else {
                    let _res = table.get(&res)?;
                    bail!("TODO")
                }
            } else {
                // TODO: support more host resource types
                debug!("lifting guest resource");
                debug_assert!(!is_host_resource_type(any.ty()));
                let res = store.data_mut().table().push(any)?;
                any = res.try_into_resource_any(store)?;
            }
            Ok(Val::Resource(any))
        }
    }
}
