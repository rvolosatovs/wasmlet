use anyhow::Context as _;
use tracing::warn;
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};

use crate::getenv;

fn new_pooling_config(instances: u32) -> PoolingAllocationConfig {
    let mut config = PoolingAllocationConfig::default();
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_UNUSED_WASM_SLOTS") {
        config.max_unused_warm_slots(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_DECOMMIT_BATCH_SIZE") {
        config.decommit_batch_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT") {
        config.async_stack_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT") {
        config.linear_memory_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TABLE_KEEP_RESIDENT") {
        config.table_keep_resident(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES") {
        config.total_component_instances(v);
    } else {
        config.total_component_instances(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_COMPONENT_INSTANCE_SIZE") {
        config.max_component_instance_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_CORE_INSTANCES_PER_COMPONENT") {
        config.max_core_instances_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORIES_PER_COMPONENT") {
        config.max_memories_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_TABLES_PER_COMPONENT") {
        config.max_tables_per_component(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_MEMORIES") {
        config.total_memories(v);
    } else {
        config.total_memories(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_TABLES") {
        config.total_tables(v);
    } else {
        config.total_tables(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_STACKS") {
        config.total_stacks(v);
    } else {
        config.total_stacks(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_CORE_INSTANCES") {
        config.total_core_instances(v);
    } else {
        config.total_core_instances(instances);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_CORE_INSTANCE_SIZE") {
        config.max_core_instance_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_TABLES_PER_MODULE") {
        config.max_tables_per_module(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TABLE_ELEMENTS") {
        config.table_elements(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORIES_PER_MODULE") {
        config.max_memories_per_module(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_MAX_MEMORY_SIZE") {
        config.max_memory_size(v);
    }
    // TODO: Add memory protection key support
    if let Some(v) = getenv("WEX_WASMTIME_POOLING_TOTAL_GC_HEAPS") {
        config.total_gc_heaps(v);
    } else {
        config.total_gc_heaps(instances);
    }
    config
}

// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default() -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    if let Some(v) = getenv("WEX_WASMTIME_POOLING") {
        return Ok(v);
    }
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}

pub fn new_engine(max_instances: u32) -> anyhow::Result<wasmtime::Engine> {
    let mut config = wasmtime::Config::default();
    config.async_support(true);
    config.epoch_interruption(true);
    config.wasm_component_model(true);
    if let Ok(true) = use_pooling_allocator_by_default() {
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(new_pooling_config(
            max_instances,
        )));
    } else {
        config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
    }
    if let Some(v) = getenv("WEX_WASMTIME_ASYNC_STACK_SIZE") {
        config.async_stack_size(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_COREDUMP_ON_TRAP") {
        config.coredump_on_trap(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_DEBUG_INFO") {
        config.debug_info(v);
    }
    if let Some(v) = getenv("WEX_WASMTIME_MAX_WASM_STACK") {
        config.max_wasm_stack(v);
    }
    match wasmtime::Engine::new(&config).context("failed to construct engine") {
        Ok(engine) => Ok(engine),
        Err(err) => {
            warn!(
                ?err,
                "failed to construct engine, fallback to on-demand allocator"
            );
            config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
            wasmtime::Engine::new(&config).context("failed to construct engine")
        }
    }
}
