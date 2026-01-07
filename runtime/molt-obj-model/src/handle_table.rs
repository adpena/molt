use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

const HANDLE_SHARD_BITS: u64 = 4;
const HANDLE_GEN_BITS: u64 = 16;
const HANDLE_INDEX_BITS: u64 = 48 - HANDLE_SHARD_BITS - HANDLE_GEN_BITS;
const HANDLE_INDEX_MASK: u64 = (1u64 << HANDLE_INDEX_BITS) - 1;
const HANDLE_GEN_MASK: u64 = (1u64 << HANDLE_GEN_BITS) - 1;
const HANDLE_SHARD_MASK: u64 = (1u64 << HANDLE_SHARD_BITS) - 1;
const HANDLE_SHARDS: usize = 1usize << (HANDLE_SHARD_BITS as usize);

#[derive(Copy, Clone)]
struct HandlePtr(*mut u8);

unsafe impl Send for HandlePtr {}
unsafe impl Sync for HandlePtr {}

#[derive(Copy, Clone)]
struct HandleSlot {
    ptr: HandlePtr,
    gen: u16,
}

struct HandleTable {
    slots: Vec<HandleSlot>,
    free: Vec<u32>,
    ptr_to_handle: HashMap<usize, u64>,
}

impl HandleTable {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            ptr_to_handle: HashMap::new(),
        }
    }
}

static HANDLE_TABLE: OnceLock<Vec<RwLock<HandleTable>>> = OnceLock::new();

fn table() -> &'static [RwLock<HandleTable>] {
    HANDLE_TABLE
        .get_or_init(|| (0..HANDLE_SHARDS).map(|_| RwLock::new(HandleTable::new())).collect())
        .as_slice()
}

fn encode_handle(index: u32, gen: u16, shard: u8) -> u64 {
    ((shard as u64) << (HANDLE_INDEX_BITS + HANDLE_GEN_BITS))
        | ((gen as u64) << HANDLE_INDEX_BITS)
        | (index as u64)
}

fn decode_handle(handle: u64) -> Option<(u8, u32, u16)> {
    if handle == 0 {
        return None;
    }
    let index = (handle & HANDLE_INDEX_MASK) as u32;
    let gen = ((handle >> HANDLE_INDEX_BITS) & HANDLE_GEN_MASK) as u16;
    let shard = ((handle >> (HANDLE_INDEX_BITS + HANDLE_GEN_BITS)) & HANDLE_SHARD_MASK) as u8;
    if gen == 0 {
        return None;
    }
    Some((shard, index, gen))
}

fn next_gen(gen: u16) -> u16 {
    let next = gen.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

fn shard_for_addr(addr: usize) -> usize {
    let mut x = addr as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as usize) & (HANDLE_SHARDS - 1)
}

pub fn register_ptr(ptr: *mut u8) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    let addr = ptr as usize;
    let shard = shard_for_addr(addr);
    let table = table();
    let mut table = table[shard].write().unwrap();
    if let Some(existing) = table.ptr_to_handle.get(&addr).copied() {
        return existing;
    }
    let (index, gen) = if let Some(index) = table.free.pop() {
        let slot = &mut table.slots[index as usize];
        let gen = next_gen(slot.gen);
        slot.gen = gen;
        slot.ptr = HandlePtr(ptr);
        (index, gen)
    } else {
        let index = table.slots.len() as u32;
        table.slots.push(HandleSlot {
            ptr: HandlePtr(ptr),
            gen: 1,
        });
        (index, 1)
    };
    let handle = encode_handle(index, gen, shard as u8);
    debug_assert!(handle <= super::POINTER_MASK);
    table.ptr_to_handle.insert(addr, handle);
    handle
}

pub fn resolve_ptr(handle: u64) -> Option<*mut u8> {
    let (shard, index, gen) = decode_handle(handle)?;
    let table = table();
    let table = table[shard as usize].read().unwrap();
    let slot = table.slots.get(index as usize)?;
    if slot.gen != gen || slot.ptr.0.is_null() {
        return None;
    }
    Some(slot.ptr.0)
}

pub fn unregister_ptr(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let addr = ptr as usize;
    let shard = shard_for_addr(addr);
    let table = table();
    let mut table = table[shard].write().unwrap();
    let Some(handle) = table.ptr_to_handle.remove(&addr) else {
        return;
    };
    let Some((handle_shard, index, gen)) = decode_handle(handle) else {
        return;
    };
    if handle_shard as usize != shard {
        return;
    }
    let Some(slot) = table.slots.get_mut(index as usize) else {
        return;
    };
    if slot.gen != gen || slot.ptr.0 != ptr {
        return;
    }
    slot.ptr = HandlePtr(std::ptr::null_mut());
    slot.gen = next_gen(slot.gen);
    table.free.push(index);
}
