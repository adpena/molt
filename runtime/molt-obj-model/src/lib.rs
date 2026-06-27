//! Core object representation for Molt.
//! Uses NaN-boxing to represent primitives and heap pointers in 64 bits.

use std::backtrace::Backtrace;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

pub use molt_codegen_abi::{INT_MAX_INLINE as INLINE_INT_MAX, INT_MIN_INLINE as INLINE_INT_MIN};
use molt_codegen_abi::{
    box_bool_bits, box_float_bits, box_int_bits, box_none_bits, box_pending_bits, box_ptr_bits,
    canonical_addr_from_masked_bits, fits_inline_int, is_bool_bits, is_float_bits, is_int_bits,
    is_none_bits, is_pending_bits, is_ptr_bits, ptr_payload_bits, unbox_bool_bits,
    unbox_inline_int_bits,
};

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct MoltObject(u64);

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
        // Bit-mix hash to distribute aligned pointers across shards.
        // Allocators return 16-byte aligned addresses, so naive modular
        // hashing (addr % 64) clusters into 4 of 64 shards. This
        // multiply-shift distributes evenly regardless of alignment.
        let mixed = addr.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58;
        let idx = mixed as usize & (PTR_REGISTRY_SHARDS - 1);
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
    if let Some(registry) = PTR_REGISTRY.get() {
        let shard = registry.shard(addr);
        let mut guard = shard.write().expect("pointer registry lock poisoned");
        if guard.remove(&addr).is_some() {
            return Some(addr);
        }
    }
    Some(addr)
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
        Self(box_float_bits(f) as u64)
    }

    /// Construct an INLINE NaN-boxed int. CONTRACT: `i` must fit the 47-bit
    /// inline window `[-2^46, 2^46)`. Values outside it need a heap BigInt —
    /// use the runtime's `int_bits_from_i128` (or `molt_int_from_i64` at the
    /// C ABI), which dispatches inline-vs-BigInt correctly. The masking below
    /// would otherwise silently truncate mod 2^47 — the silent-integer
    /// miscompile class (this exact stub truncated the overflow_peel
    /// accumulator's exit boxing before `molt_int_from_i64` was fixed).
    #[inline(always)]
    pub fn from_int(i: i64) -> Self {
        debug_assert!(
            fits_inline_int(i),
            "MoltObject::from_int: {i} is outside the 47-bit inline window; \
             callers must route non-inline values through int_bits_from_i128"
        );
        Self(box_int_bits(i) as u64)
    }

    #[inline(always)]
    pub fn try_from_int(i: i64) -> Option<Self> {
        fits_inline_int(i).then(|| Self::from_int(i))
    }

    #[inline(always)]
    pub fn try_from_uint(i: u64) -> Option<Self> {
        (i <= INLINE_INT_MAX as u64).then(|| Self::from_int(i as i64))
    }

    #[inline(always)]
    pub fn from_bool(b: bool) -> Self {
        let val = if b { 1 } else { 0 };
        Self(box_bool_bits(val) as u64)
    }

    #[inline(always)]
    pub fn none() -> Self {
        Self(box_none_bits() as u64)
    }

    #[inline(always)]
    pub fn pending() -> Self {
        Self(box_pending_bits() as u64)
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
            Self(box_ptr_bits(addr) as u64)
        }
        #[cfg(not(debug_assertions))]
        {
            let addr = ptr as u64;
            Self(box_ptr_bits(addr) as u64)
        }
    }

    #[inline(always)]
    pub fn is_float(&self) -> bool {
        is_float_bits(self.0)
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
        is_int_bits(self.0)
    }

    #[inline(always)]
    pub fn is_bool(&self) -> bool {
        is_bool_bits(self.0)
    }

    #[inline(always)]
    pub fn as_bool(&self) -> Option<bool> {
        if self.is_bool() {
            Some(unbox_bool_bits(self.0) == 1)
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn is_none(&self) -> bool {
        is_none_bits(self.0)
    }

    #[inline(always)]
    pub fn is_pending(&self) -> bool {
        is_pending_bits(self.0)
    }

    #[inline(always)]
    pub fn is_ptr(&self) -> bool {
        is_ptr_bits(self.0)
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> Option<*mut u8> {
        if self.is_ptr() {
            let masked = ptr_payload_bits(self.0);
            let addr = canonical_addr_from_masked_bits(masked);
            // The NaN-boxed canonical address is the SINGLE source of truth for a
            // `TAG_PTR` value's identity, on every build profile. Recovering it via
            // `with_exposed_provenance_mut` is exactly what release does, and it is
            // address-equivalent to the registered slot for runtime-minted pointers
            // (`register_ptr` keys on the same `expose_provenance()` address that
            // `from_ptr` boxes).
            //
            // The provenance registry is a debug-only PROVENANCE CHECKER, not the
            // arbiter of whether a pointer is recoverable. Compiled code legitimately
            // mints `TAG_PTR` values WITHOUT calling `from_ptr` — e.g. the native
            // backend inlines `ObjectNewBoundStack` and NaN-boxes the Cranelift
            // `stack_addr` directly (function_compiler.rs `box_ptr_value`). Those
            // pointers are canonical and valid but never enter the registry, so a
            // registry MISS must not turn `as_ptr()` into `None` — doing so made
            // stack-allocated temp receivers (`Left().who()`) decode to a null
            // receiver under the dev profile only, diverging from release.
            //
            // So: when the registry has a slot for this address, return the
            // provenance-carrying registered pointer (preserving the debug-build
            // safety check for runtime-minted objects); otherwise fall back to the
            // canonical reconstruction, identical to release. This keeps `as_ptr()`
            // profile-independent while retaining the registry's checking value.
            #[cfg(not(debug_assertions))]
            {
                Some(std::ptr::with_exposed_provenance_mut(addr as usize))
            }
            #[cfg(debug_assertions)]
            {
                resolve_ptr(addr)
                    .or_else(|| Some(std::ptr::with_exposed_provenance_mut(addr as usize)))
            }
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_int(&self) -> Option<i64> {
        if self.is_int() {
            Some(unbox_inline_int_bits(self.0))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_int_unchecked(&self) -> i64 {
        unbox_inline_int_bits(self.0)
    }

    /// Returns `true` if this object is immortal — i.e. it must never be
    /// refcounted because it represents an eternal constant.
    ///
    /// Immortal objects (mirroring CPython 3.12+ PEP 683):
    /// - `None`
    /// - `True` / `False`
    /// - Small integers in `[-5, 256]` (the CPython interned range)
    ///
    /// For `None` and bools the check is a single tag comparison (O(1)).
    /// For integers, after confirming the tag, a range check on the
    /// sign-extended 47-bit payload determines immortality.
    #[inline(always)]
    pub fn is_immortal(&self) -> bool {
        // None and Bool are unconditionally immortal — single comparison each.
        if self.is_none() || self.is_bool() {
            return true;
        }

        // Small integers in [-5, 256].
        if self.is_int() {
            let i = self.as_int_unchecked();
            return (-5..=256).contains(&i);
        }

        false
    }
}

/// C ABI entry point for the compiler to check immortality before
/// emitting IncRef/DecRef calls. Returns 1 if immortal, 0 otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn molt_is_immortal(bits: u64) -> u64 {
    if MoltObject::from_bits(bits).is_immortal() {
        1
    } else {
        0
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
    //!   Values outside `[-(2^46), 2^46 - 1]` must be routed through heap BigInt
    //!   constructors instead of `from_int`.
    //!   The sign extension logic in `as_int()` recovers the original value for
    //!   any input within the valid range.
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
    fn test_try_from_int_rejects_non_inline_values() {
        assert_eq!(
            MoltObject::try_from_int(INLINE_INT_MIN).and_then(|obj| obj.as_int()),
            Some(INLINE_INT_MIN)
        );
        assert_eq!(
            MoltObject::try_from_int(INLINE_INT_MAX).and_then(|obj| obj.as_int()),
            Some(INLINE_INT_MAX)
        );
        assert!(MoltObject::try_from_int(INLINE_INT_MIN - 1).is_none());
        assert!(MoltObject::try_from_int(INLINE_INT_MAX + 1).is_none());
    }

    #[test]
    fn test_try_from_uint_rejects_non_inline_values() {
        assert_eq!(
            MoltObject::try_from_uint(0).and_then(|obj| obj.as_int()),
            Some(0)
        );
        assert_eq!(
            MoltObject::try_from_uint(INLINE_INT_MAX as u64).and_then(|obj| obj.as_int()),
            Some(INLINE_INT_MAX)
        );
        assert!(MoltObject::try_from_uint(INLINE_INT_MAX as u64 + 1).is_none());
    }

    #[test]
    fn test_negative_int() {
        let obj = MoltObject::from_int(-1);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(-1));
    }

    #[test]
    fn none_is_immortal() {
        assert!(MoltObject::none().is_immortal());
    }

    #[test]
    fn bools_are_immortal() {
        assert!(MoltObject::from_bool(true).is_immortal());
        assert!(MoltObject::from_bool(false).is_immortal());
    }

    #[test]
    fn small_ints_are_immortal() {
        // Boundary: -5 is the lowest immortal int
        assert!(MoltObject::from_int(-5).is_immortal());
        assert!(MoltObject::from_int(-4).is_immortal());
        assert!(MoltObject::from_int(-1).is_immortal());
        assert!(MoltObject::from_int(0).is_immortal());
        assert!(MoltObject::from_int(1).is_immortal());
        assert!(MoltObject::from_int(100).is_immortal());
        assert!(MoltObject::from_int(255).is_immortal());
        assert!(MoltObject::from_int(256).is_immortal());
    }

    #[test]
    fn out_of_range_ints_are_not_immortal() {
        assert!(!MoltObject::from_int(-6).is_immortal());
        assert!(!MoltObject::from_int(257).is_immortal());
        assert!(!MoltObject::from_int(1000).is_immortal());
        assert!(!MoltObject::from_int(-1000).is_immortal());
    }

    #[test]
    fn floats_are_not_immortal() {
        assert!(!MoltObject::from_float(0.0).is_immortal());
        assert!(!MoltObject::from_float(1.0).is_immortal());
        assert!(!MoltObject::from_float(std::f64::consts::PI).is_immortal());
    }

    #[test]
    fn pointers_are_not_immortal() {
        let boxed = Box::new(42u8);
        let ptr = Box::into_raw(boxed);
        let obj = MoltObject::from_ptr(ptr);
        assert!(!obj.is_immortal());
        release_ptr(ptr);
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }

    #[test]
    fn molt_is_immortal_c_abi() {
        assert_eq!(super::molt_is_immortal(MoltObject::none().bits()), 1);
        assert_eq!(
            super::molt_is_immortal(MoltObject::from_bool(true).bits()),
            1
        );
        assert_eq!(super::molt_is_immortal(MoltObject::from_int(0).bits()), 1);
        assert_eq!(super::molt_is_immortal(MoltObject::from_int(257).bits()), 0);
        assert_eq!(
            super::molt_is_immortal(MoltObject::from_float(1.0).bits()),
            0
        );
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

    #[test]
    fn as_ptr_recovers_address_for_compiled_minted_box() {
        // Compiled native code (e.g. the inlined `ObjectNewBoundStack` lowering)
        // NaN-boxes a pointer directly without going through `from_ptr`, so the
        // pointer never enters the debug provenance registry. `as_ptr()` must
        // still recover the canonical address — a registry MISS is NOT `None`.
        // This is the profile-independence contract the call_method_ic receiver
        // (a stack-allocated temp receiver) depends on.
        reset_ptr_registry();
        let boxed = Box::new(0xABu8);
        let ptr = Box::into_raw(boxed);
        let addr = ptr.expose_provenance() as u64;
        // Build the TAG_PTR box exactly as `box_ptr_value` does in codegen,
        // bypassing `from_ptr`/`register_ptr`.
        let obj = MoltObject(box_ptr_bits(addr) as u64);
        assert!(obj.is_ptr());
        let recovered = obj.as_ptr().expect("unregistered TAG_PTR must recover");
        assert_eq!(recovered.expose_provenance() as u64, addr);
        unsafe {
            drop(Box::from_raw(ptr));
        }
        reset_ptr_registry();
    }

    #[test]
    fn release_ptr_removes_registered_entry() {
        reset_ptr_registry();
        let boxed = Box::new(123u8);
        let ptr = Box::into_raw(boxed);
        let addr = register_ptr(ptr);
        assert_eq!(resolve_ptr(addr), Some(ptr));

        assert_eq!(release_ptr(ptr), Some(addr));
        assert_eq!(resolve_ptr(addr), None);

        unsafe {
            drop(Box::from_raw(ptr));
        }
        reset_ptr_registry();
    }
}
