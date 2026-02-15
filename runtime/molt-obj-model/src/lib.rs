//! Core object representation for Molt.
//! Uses NaN-boxing to represent primitives and heap pointers in 64 bits.

use std::backtrace::Backtrace;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltObject(u64);

const QNAN: u64 = 0x7ff8_0000_0000_0000;
const TAG_INT: u64 = 0x0001_0000_0000_0000;
const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
const TAG_NONE: u64 = 0x0003_0000_0000_0000;
const TAG_PTR: u64 = 0x0004_0000_0000_0000;
const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
const TAG_MASK: u64 = 0x0007_0000_0000_0000;
const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_SIGN_BIT: u64 = 1 << 46;
const INT_WIDTH: u64 = 47;
const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;

const PTR_REGISTRY_SHARDS: usize = 64;

struct PtrRegistry {
    shards: Vec<RwLock<HashMap<u64, PtrSlot>>>,
}

impl PtrRegistry {
    fn new() -> Self {
        let mut shards = Vec::with_capacity(PTR_REGISTRY_SHARDS);
        for _ in 0..PTR_REGISTRY_SHARDS {
            shards.push(RwLock::new(HashMap::new()));
        }
        Self { shards }
    }

    fn shard(&self, addr: u64) -> &RwLock<HashMap<u64, PtrSlot>> {
        let idx = (addr as usize) % PTR_REGISTRY_SHARDS;
        &self.shards[idx]
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct PtrSlot(*mut u8);

// Raw pointers are guarded by the registry lock; it is safe to share slots.
unsafe impl Send for PtrSlot {}
unsafe impl Sync for PtrSlot {}

static PTR_REGISTRY: OnceLock<PtrRegistry> = OnceLock::new();
static PTR_REG_COUNT: AtomicU64 = AtomicU64::new(0);
static PTR_REG_BACKTRACE_PRINTED: AtomicU64 = AtomicU64::new(0);

fn ptr_registry() -> &'static PtrRegistry {
    PTR_REGISTRY.get_or_init(PtrRegistry::new)
}

fn trace_ptr_registry() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PTR_REGISTRY").ok().as_deref(),
            Some("1")
        )
    })
}

fn canonical_addr_from_masked(masked: u64) -> u64 {
    let signed = ((masked << 16) as i64) >> 16;
    signed as u64
}

pub fn register_ptr(ptr: *mut u8) -> u64 {
    if ptr.is_null() {
        return 0;
    }
    let count = PTR_REG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if trace_ptr_registry() {
        if count.is_multiple_of(100_000) {
            eprintln!("ptr_registry register count={count}");
        }
        if count >= 1_000_000
            && PTR_REG_BACKTRACE_PRINTED
                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            eprintln!(
                "ptr_registry backtrace at count={count}:\n{}",
                Backtrace::capture()
            );
        }
    }
    let slot = PtrSlot(ptr);
    let addr = ptr.expose_provenance() as u64;
    let shard = ptr_registry().shard(addr);
    if let Ok(guard) = shard.read()
        && guard.get(&addr).copied() == Some(slot)
    {
        return addr;
    }
    let mut guard = shard.write().expect("pointer registry lock poisoned");
    guard.insert(addr, slot);
    addr
}

pub fn resolve_ptr(addr: u64) -> Option<*mut u8> {
    if addr == 0 {
        return None;
    }
    let shard = ptr_registry().shard(addr);
    let guard = shard.read().expect("pointer registry lock poisoned");
    guard.get(&addr).map(|slot| slot.0)
}

pub fn release_ptr(ptr: *mut u8) -> Option<u64> {
    if ptr.is_null() {
        return None;
    }
    let addr = ptr.expose_provenance() as u64;
    let shard = ptr_registry().shard(addr);
    let mut guard = shard.write().expect("pointer registry lock poisoned");
    guard.remove(&addr).map(|_| addr)
}

pub fn reset_ptr_registry() {
    if let Some(registry) = PTR_REGISTRY.get() {
        for shard in &registry.shards {
            if let Ok(mut guard) = shard.write() {
                guard.clear();
            }
        }
    }
    PTR_REG_COUNT.store(0, Ordering::Relaxed);
    PTR_REG_BACKTRACE_PRINTED.store(0, Ordering::Relaxed);
}

impl MoltObject {
    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub fn bits(self) -> u64 {
        self.0
    }

    pub fn from_float(f: f64) -> Self {
        if f.is_nan() {
            Self(CANONICAL_NAN_BITS)
        } else {
            Self(f.to_bits())
        }
    }

    pub fn from_int(i: i64) -> Self {
        // Simple 47-bit integer for MVP
        let val = (i as u64) & INT_MASK;
        Self(QNAN | TAG_INT | val)
    }

    pub fn from_bool(b: bool) -> Self {
        let val = if b { 1 } else { 0 };
        Self(QNAN | TAG_BOOL | val)
    }

    pub fn none() -> Self {
        Self(QNAN | TAG_NONE)
    }

    pub fn pending() -> Self {
        Self(QNAN | TAG_PENDING)
    }

    pub fn from_ptr(ptr: *mut u8) -> Self {
        let addr = register_ptr(ptr);
        let high = addr >> 48;
        debug_assert!(
            high == 0 || high == 0xffff,
            "Non-canonical pointer for MoltObject"
        );
        let masked = addr & POINTER_MASK;
        Self(QNAN | TAG_PTR | masked)
    }

    pub fn is_float(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    pub fn as_float(&self) -> Option<f64> {
        if self.is_float() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    pub fn is_int(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_INT)
    }

    pub fn is_bool(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
    }

    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.0 & 0x1) == 1)
        } else {
            None
        }
    }

    pub fn is_none(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_NONE)
    }

    pub fn is_pending(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PENDING)
    }

    pub fn is_ptr(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    }

    pub fn as_ptr(&self) -> Option<*mut u8> {
        if self.is_ptr() {
            let masked = self.0 & POINTER_MASK;
            let addr = canonical_addr_from_masked(masked);
            resolve_ptr(addr)
        } else {
            None
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            let val = self.0 & INT_MASK;
            // Sign-extend if needed (assuming 47-bit signed)
            if (val & INT_SIGN_BIT) != 0 {
                Some((val as i64) - ((1u64 << INT_WIDTH) as i64))
            } else {
                Some(val as i64)
            }
        } else {
            None
        }
    }

    pub fn as_int_unchecked(&self) -> i64 {
        let val = self.0 & INT_MASK;
        if (val & INT_SIGN_BIT) != 0 {
            (val as i64) - ((1u64 << INT_WIDTH) as i64)
        } else {
            val as i64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float() {
        let obj = MoltObject::from_float(std::f64::consts::PI);
        assert!(obj.is_float());
    }

    #[test]
    fn test_int() {
        let obj = MoltObject::from_int(42);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(42));
    }

    #[test]
    fn test_negative_int() {
        let obj = MoltObject::from_int(-1);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(-1));
    }

    #[test]
    fn test_ptr_roundtrip() {
        let boxed = Box::new(123u8);
        let ptr = Box::into_raw(boxed);
        let obj = MoltObject::from_ptr(ptr);
        assert!(obj.is_ptr());
        assert_eq!(obj.as_ptr(), Some(ptr));
        release_ptr(ptr);
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}
