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
    #[cfg(not(debug_assertions))]
    {
        Some(ptr as u64)
    }
    #[cfg(debug_assertions)]
    {
        let addr = ptr.expose_provenance() as u64;
        let shard = ptr_registry().shard(addr);
        let mut guard = shard.write().expect("pointer registry lock poisoned");
        guard.remove(&addr).map(|_| addr)
    }
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
    #[inline(always)]
    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    #[inline(always)]
    pub fn bits(self) -> u64 {
        self.0
    }

    #[inline(always)]
    pub fn from_float(f: f64) -> Self {
        if f.is_nan() {
            Self(CANONICAL_NAN_BITS)
        } else {
            Self(f.to_bits())
        }
    }

    #[inline(always)]
    pub fn from_int(i: i64) -> Self {
        // Simple 47-bit integer for MVP
        let val = (i as u64) & INT_MASK;
        Self(QNAN | TAG_INT | val)
    }

    #[inline(always)]
    pub fn from_bool(b: bool) -> Self {
        let val = if b { 1 } else { 0 };
        Self(QNAN | TAG_BOOL | val)
    }

    #[inline(always)]
    pub fn none() -> Self {
        Self(QNAN | TAG_NONE)
    }

    #[inline(always)]
    pub fn pending() -> Self {
        Self(QNAN | TAG_PENDING)
    }

    #[inline(always)]
    pub fn from_ptr(ptr: *mut u8) -> Self {
        // In release builds, skip the registry — the NaN-box encoding already
        // stores the canonical 48-bit address directly. The registry exists
        // only for provenance safety checking in debug/dev builds.
        #[cfg(debug_assertions)]
        {
            let addr = register_ptr(ptr);
            let high = addr >> 48;
            debug_assert!(
                high == 0 || high == 0xffff,
                "Non-canonical pointer for MoltObject"
            );
            let masked = addr & POINTER_MASK;
            Self(QNAN | TAG_PTR | masked)
        }
        #[cfg(not(debug_assertions))]
        {
            let addr = ptr as u64;
            let masked = addr & POINTER_MASK;
            Self(QNAN | TAG_PTR | masked)
        }
    }

    #[inline(always)]
    pub fn is_float(&self) -> bool {
        (self.0 & QNAN) != QNAN
    }

    #[inline(always)]
    pub fn as_float(&self) -> Option<f64> {
        if self.is_float() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn is_int(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_INT)
    }

    #[inline(always)]
    pub fn is_bool(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_BOOL)
    }

    #[inline(always)]
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some((self.0 & 0x1) == 1)
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn is_none(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_NONE)
    }

    #[inline(always)]
    pub fn is_pending(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PENDING)
    }

    #[inline(always)]
    pub fn is_ptr(&self) -> bool {
        (self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_PTR)
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> Option<*mut u8> {
        if self.is_ptr() {
            let masked = self.0 & POINTER_MASK;
            let addr = canonical_addr_from_masked(masked);
            // In release builds, bypass the registry — reconstruct the pointer
            // directly from the NaN-boxed canonical address. The registry lookup
            // only exists for provenance safety checking in debug/dev builds.
            #[cfg(not(debug_assertions))]
            {
                Some(std::ptr::with_exposed_provenance_mut(addr as usize))
            }
            #[cfg(debug_assertions)]
            {
                resolve_ptr(addr)
            }
        } else {
            None
        }
    }

    #[inline(always)]
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

    #[inline(always)]
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
mod bit_layout_contract {
    //! # NaN-Boxing Bit Layout Contract
    //!
    //! All `MoltObject` instances encode their type and payload in a single `u64`
    //! using the NaN-boxing technique.  The 64-bit IEEE 754 double format reserves
    //! a range of bit patterns for NaN values; Molt repurposes that space to store
    //! non-float types.
    //!
    //! ## Encoding Scheme
    //!
    //! | Type        | Bit pattern                                         |
    //! |-------------|-----------------------------------------------------|
    //! | **Float**   | Raw `f64` bits.  NaN inputs are canonicalized to     |
    //! |             | `CANONICAL_NAN_BITS` (`0x7ff0_0000_0000_0001`).      |
    //! | **Int**     | `QNAN \| TAG_INT \| (sign-extended 47-bit payload)`. |
    //! |             | Range: `[-(2^46), 2^46 - 1]`.                        |
    //! | **Bool**    | `QNAN \| TAG_BOOL \| (0 or 1)`.                     |
    //! | **None**    | `QNAN \| TAG_NONE \| 0`.                            |
    //! | **Pending** | `QNAN \| TAG_PENDING \| 0`.                         |
    //! | **Ptr**     | `QNAN \| TAG_PTR \| (48-bit masked address)`.        |
    //!
    //! Tag constants occupy bits 48..50 (`TAG_MASK = 0x0007_0000_0000_0000`):
    //!   - `TAG_INT     = 0x0001_...`
    //!   - `TAG_BOOL    = 0x0002_...`
    //!   - `TAG_NONE    = 0x0003_...`
    //!   - `TAG_PTR     = 0x0004_...`
    //!   - `TAG_PENDING = 0x0005_...`
    //!
    //! ## Float Detection
    //!
    //! A value is a float if and only if its QNAN prefix bits are **not** all set:
    //! `(bits & QNAN) != QNAN`.  This means every non-NaN `f64` is stored verbatim
    //! and all tagged types (int, bool, none, ptr, pending) are stored in the NaN
    //! space with QNAN forced on.
    //!
    //! ## Cross-Architecture Guarantees
    //!
    //! - **NaN canonicalization** ensures deterministic float representation across
    //!   CPUs.  Any NaN input (signaling, quiet, positive, negative) maps to the
    //!   single `CANONICAL_NAN_BITS` pattern.
    //! - **48-bit pointer mask** (`POINTER_MASK = 0x0000_FFFF_FFFF_FFFF`) with
    //!   sign extension via `canonical_addr_from_masked()` handles canonical
    //!   addressing on x86-64, where the upper 16 bits must match bit 47.
    //! - **Integer range** is bounded by the 47-bit inline representation width.
    //!   Values outside `[-(2^46), 2^46 - 1]` are silently truncated by `from_int`.
    //!   The sign extension logic in `as_int()` recovers the original value for any
    //!   input within the valid range.
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
