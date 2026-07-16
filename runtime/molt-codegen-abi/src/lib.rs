#![no_std]

#[cfg(all(feature = "free-threaded", target_arch = "wasm32"))]
compile_error!(
    "Molt free-threaded mode is not supported on wasm32 yet: declare and prove a shared-memory + atomics host capability before enabling it"
);

#[cfg(all(
    feature = "free-threaded",
    not(target_arch = "wasm32"),
    not(target_has_atomic = "32")
))]
compile_error!("Molt native free-threaded mode requires 32-bit atomic operations");

#[cfg(test)]
extern crate std;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatingSystem {
    Windows,
    Macos,
    Linux,
    Wasm,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Wasm32,
    Wasm64,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endianness {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerWidth {
    Bits32,
    Bits64,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostPlatform {
    pub os: OperatingSystem,
    pub arch: Architecture,
    pub pointer_width: PointerWidth,
    pub endianness: Endianness,
}

impl HostPlatform {
    pub const fn current() -> Self {
        Self {
            os: current_os(),
            arch: current_arch(),
            pointer_width: current_pointer_width(),
            endianness: current_endianness(),
        }
    }

    pub const fn supports_nanbox_word_abi(self) -> bool {
        matches!(self.pointer_width, PointerWidth::Bits64)
    }
}

pub const HOST_PLATFORM: HostPlatform = HostPlatform::current();

pub const POINTER_PAYLOAD_BITS: u32 = 48;
pub const INLINE_INT_PAYLOAD_BITS: u32 = 47;
pub const TAG_FIELD_SHIFT: i64 = 48;
pub const PTR_SIGN_EXT_SHIFT: i64 = 16;
pub const SPECIAL_TAG_BASE: i64 = 0x7ff9;
pub const SPECIAL_TAG_LIMIT: i64 = 5;

pub const QNAN: u64 = 0x7ff8_0000_0000_0000;
pub const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
pub const TAG_INT: u64 = 0x0001_0000_0000_0000;
pub const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
pub const TAG_NONE: u64 = 0x0003_0000_0000_0000;
pub const TAG_PTR: u64 = 0x0004_0000_0000_0000;
pub const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
pub const TAG_MASK: u64 = 0x0007_0000_0000_0000;
pub const POINTER_MASK: u64 = (1u64 << POINTER_PAYLOAD_BITS) - 1;

pub const INT_WIDTH: u64 = INLINE_INT_PAYLOAD_BITS as u64;
pub const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
pub const INT_SIGN_BIT: u64 = 1u64 << (INT_WIDTH - 1);
pub const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;
pub const INT_MIN_INLINE: i64 = -(1_i64 << (INT_WIDTH - 1));
pub const INT_MAX_INLINE: i64 = (1_i64 << (INT_WIDTH - 1)) - 1;
pub const INLINE_INT_BIAS: i64 = 1_i64 << (INT_WIDTH - 1);
pub const INLINE_INT_LIMIT: i64 = 1_i64 << INT_WIDTH;

pub const QNAN_TAG_MASK_I64: i64 = (QNAN | TAG_MASK) as i64;
pub const QNAN_TAG_INT_I64: i64 = (QNAN | TAG_INT) as i64;
pub const QNAN_TAG_BOOL_I64: i64 = (QNAN | TAG_BOOL) as i64;
pub const QNAN_TAG_NONE_I64: i64 = (QNAN | TAG_NONE) as i64;
pub const QNAN_TAG_PTR_I64: i64 = (QNAN | TAG_PTR) as i64;
pub const QNAN_TAG_PENDING_I64: i64 = (QNAN | TAG_PENDING) as i64;

// ListIntStorage (#[repr(C)]) field offsets. Must match
// runtime/molt-runtime/src/object/layout.rs.
pub const LIST_INT_STORAGE_DATA_OFFSET: i32 = 0;
pub const LIST_INT_STORAGE_LEN_OFFSET: i32 = 8;

pub const GENERATOR_CONTROL_BYTES: i32 = 48;
pub const TASK_KIND_FUTURE: i64 = 0;
pub const TASK_KIND_GENERATOR: i64 = 1;
pub const TASK_KIND_COROUTINE: i64 = 2;

pub const FUNC_DEFAULT_NONE: i64 = 1;
pub const FUNC_DEFAULT_DICT_POP: i64 = 2;
pub const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;

pub const HEADER_SIZE_BYTES: i32 = 24;
pub const HEADER_ALLOC_ALIGN_BYTES: usize = 8;
pub const HEADER_TYPE_ID_OFFSET: i32 = -HEADER_SIZE_BYTES;
pub const HEADER_REFCOUNT_OFFSET: i32 = -(HEADER_SIZE_BYTES - 4);
pub const HEADER_FLAGS_OFFSET: i32 = -(HEADER_SIZE_BYTES - 8);
pub const HEADER_AUX_KIND_OFFSET: i32 = -10;
pub const HEADER_AUX_OFFSET: i32 = -8;

pub const HEADER_AUX_KIND_NONE: u16 = 0;
pub const HEADER_AUX_KIND_CLASS_INLINE: u16 = 1;
pub const HEADER_AUX_KIND_STATE_INLINE: u16 = 2;
pub const HEADER_AUX_KIND_SIDECAR: u16 = 3;

pub const HEADER_CLASS_WORD_BORROWED: u64 = 1;
pub const HEADER_CLASS_WORD_TAG_MASK: u64 = 0x7;
pub const HEADER_CLASS_WORD_BITS_MASK: u64 = !HEADER_CLASS_WORD_TAG_MASK;

pub const HEADER_FLAG_HAS_PTRS: u32 = 1;
pub const HEADER_FLAG_IMMORTAL: u32 = 1 << 15;
pub const HEADER_FLAG_CONTAINS_REFS: u32 = 1 << 19;
/// Two-phase lifecycle publication bit. Objects remain invisible to collector
/// snapshots until initialization clears this bit with Release ordering.
pub const HEADER_FLAG_GC_UNPUBLISHED: u32 = 1 << 29;
pub const IMMORTAL_REFCOUNT: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedObjectAbiFacts {
    pub header_size: i32,
    pub header_align: usize,
    pub type_id_offset: i32,
    pub refcount_offset: i32,
    pub flags_offset: i32,
    pub aux_kind_offset: i32,
    pub aux_offset: i32,
    pub aux_kind_class_inline: u16,
    pub aux_kind_none: u16,
    pub aux_kind_state_inline: u16,
    pub aux_kind_sidecar: u16,
    pub class_word_borrowed: u64,
    pub class_word_tag_mask: u64,
    pub class_word_bits_mask: u64,
    pub flag_has_ptrs: u32,
    pub flag_immortal: u32,
    pub flag_contains_refs: u32,
    pub flag_gc_unpublished: u32,
    pub qnan: u64,
    pub canonical_nan: u64,
    pub tag_mask: u64,
    pub tag_ptr: u64,
    pub pointer_mask: u64,
    pub pointer_payload_bits: u32,
    pub tag_field_shift: i64,
    pub ptr_sign_ext_shift: i64,
    pub special_tag_base: i64,
    pub special_tag_limit: i64,
    pub immortal_refcount: u32,
    pub generator_control_bytes: i32,
    pub list_int_storage_data_offset: i32,
    pub list_int_storage_len_offset: i32,
    pub inline_int_bias: i64,
    pub inline_int_limit: i64,
    pub int_mask: u64,
    pub int_min_inline: i64,
    pub int_max_inline: i64,
    pub int_shift: i64,
    pub int_sign_bit: u64,
    pub int_width: u64,
    pub qnan_tag_mask: i64,
    pub qnan_tag_int: i64,
    pub qnan_tag_bool: i64,
    pub qnan_tag_none: i64,
    pub qnan_tag_ptr: i64,
    pub qnan_tag_pending: i64,
    pub tag_int: u64,
    pub tag_bool: u64,
    pub tag_none: u64,
    pub tag_pending: u64,
    pub task_kind_future: i64,
    pub task_kind_generator: i64,
    pub task_kind_coroutine: i64,
    pub type_id_object: u32,
    pub type_id_function: u32,
    pub type_id_type: u32,
    pub type_id_list_bool: u32,
}

impl GeneratedObjectAbiFacts {
    pub const fn words(self) -> [u64; 57] {
        [
            self.header_size as i64 as u64,
            self.header_align as u64,
            self.type_id_offset as i64 as u64,
            self.refcount_offset as i64 as u64,
            self.flags_offset as i64 as u64,
            self.aux_kind_offset as i64 as u64,
            self.aux_offset as i64 as u64,
            self.aux_kind_class_inline as u64,
            self.aux_kind_none as u64,
            self.aux_kind_state_inline as u64,
            self.aux_kind_sidecar as u64,
            self.class_word_borrowed,
            self.class_word_tag_mask,
            self.class_word_bits_mask,
            self.flag_has_ptrs as u64,
            self.flag_immortal as u64,
            self.flag_contains_refs as u64,
            self.flag_gc_unpublished as u64,
            self.qnan,
            self.canonical_nan,
            self.tag_mask,
            self.tag_ptr,
            self.pointer_mask,
            self.pointer_payload_bits as u64,
            self.tag_field_shift as u64,
            self.ptr_sign_ext_shift as u64,
            self.special_tag_base as u64,
            self.special_tag_limit as u64,
            self.immortal_refcount as u64,
            self.generator_control_bytes as i64 as u64,
            self.list_int_storage_data_offset as i64 as u64,
            self.list_int_storage_len_offset as i64 as u64,
            self.inline_int_bias as u64,
            self.inline_int_limit as u64,
            self.int_mask,
            self.int_min_inline as u64,
            self.int_max_inline as u64,
            self.int_shift as u64,
            self.int_sign_bit,
            self.int_width,
            self.qnan_tag_mask as u64,
            self.qnan_tag_int as u64,
            self.qnan_tag_bool as u64,
            self.qnan_tag_none as u64,
            self.qnan_tag_ptr as u64,
            self.qnan_tag_pending as u64,
            self.tag_int,
            self.tag_bool,
            self.tag_none,
            self.tag_pending,
            self.task_kind_future as u64,
            self.task_kind_generator as u64,
            self.task_kind_coroutine as u64,
            self.type_id_object as u64,
            self.type_id_function as u64,
            self.type_id_type as u64,
            self.type_id_list_bool as u64,
        ]
    }

    pub const fn fingerprint(self) -> u64 {
        fingerprint_words(self.words())
    }
}

pub const fn fingerprint_words<const N: usize>(facts: [u64; N]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut fact_index = 0;
    while fact_index < facts.len() {
        let bytes = facts[fact_index].to_le_bytes();
        let mut byte_index = 0;
        while byte_index < bytes.len() {
            hash = (hash ^ bytes[byte_index] as u64).wrapping_mul(0x0000_0100_0000_01b3);
            byte_index += 1;
        }
        fact_index += 1;
    }
    hash
}

pub const GENERATED_OBJECT_ABI_FACTS: GeneratedObjectAbiFacts = GeneratedObjectAbiFacts {
    header_size: HEADER_SIZE_BYTES,
    header_align: HEADER_ALLOC_ALIGN_BYTES,
    type_id_offset: HEADER_TYPE_ID_OFFSET,
    refcount_offset: HEADER_REFCOUNT_OFFSET,
    flags_offset: HEADER_FLAGS_OFFSET,
    aux_kind_offset: HEADER_AUX_KIND_OFFSET,
    aux_offset: HEADER_AUX_OFFSET,
    aux_kind_class_inline: HEADER_AUX_KIND_CLASS_INLINE,
    aux_kind_none: HEADER_AUX_KIND_NONE,
    aux_kind_state_inline: HEADER_AUX_KIND_STATE_INLINE,
    aux_kind_sidecar: HEADER_AUX_KIND_SIDECAR,
    class_word_borrowed: HEADER_CLASS_WORD_BORROWED,
    class_word_tag_mask: HEADER_CLASS_WORD_TAG_MASK,
    class_word_bits_mask: HEADER_CLASS_WORD_BITS_MASK,
    flag_has_ptrs: HEADER_FLAG_HAS_PTRS,
    flag_immortal: HEADER_FLAG_IMMORTAL,
    flag_contains_refs: HEADER_FLAG_CONTAINS_REFS,
    flag_gc_unpublished: HEADER_FLAG_GC_UNPUBLISHED,
    qnan: QNAN,
    canonical_nan: CANONICAL_NAN_BITS,
    tag_mask: TAG_MASK,
    tag_ptr: TAG_PTR,
    pointer_mask: POINTER_MASK,
    pointer_payload_bits: POINTER_PAYLOAD_BITS,
    tag_field_shift: TAG_FIELD_SHIFT,
    ptr_sign_ext_shift: PTR_SIGN_EXT_SHIFT,
    special_tag_base: SPECIAL_TAG_BASE,
    special_tag_limit: SPECIAL_TAG_LIMIT,
    immortal_refcount: IMMORTAL_REFCOUNT,
    generator_control_bytes: GENERATOR_CONTROL_BYTES,
    list_int_storage_data_offset: LIST_INT_STORAGE_DATA_OFFSET,
    list_int_storage_len_offset: LIST_INT_STORAGE_LEN_OFFSET,
    inline_int_bias: INLINE_INT_BIAS,
    inline_int_limit: INLINE_INT_LIMIT,
    int_mask: INT_MASK,
    int_min_inline: INT_MIN_INLINE,
    int_max_inline: INT_MAX_INLINE,
    int_shift: INT_SHIFT,
    int_sign_bit: INT_SIGN_BIT,
    int_width: INT_WIDTH,
    qnan_tag_mask: QNAN_TAG_MASK_I64,
    qnan_tag_int: QNAN_TAG_INT_I64,
    qnan_tag_bool: QNAN_TAG_BOOL_I64,
    qnan_tag_none: QNAN_TAG_NONE_I64,
    qnan_tag_ptr: QNAN_TAG_PTR_I64,
    qnan_tag_pending: QNAN_TAG_PENDING_I64,
    tag_int: TAG_INT,
    tag_bool: TAG_BOOL,
    tag_none: TAG_NONE,
    tag_pending: TAG_PENDING,
    task_kind_future: TASK_KIND_FUTURE,
    task_kind_generator: TASK_KIND_GENERATOR,
    task_kind_coroutine: TASK_KIND_COROUTINE,
    type_id_object: TYPE_ID_OBJECT,
    type_id_function: TYPE_ID_FUNCTION,
    type_id_type: TYPE_ID_TYPE,
    type_id_list_bool: TYPE_ID_LIST_BOOL,
};

/// Ratchet binding every hardcoded generated-code header fact. A layout/offset
/// change fails compilation until this value and both link symbols are bumped.
pub const GENERATED_OBJECT_ABI_FINGERPRINT_V1: u64 = 0x5fce_853b_ad8a_c502;
const _: () = assert!(
    GENERATED_OBJECT_ABI_FACTS.fingerprint() == GENERATED_OBJECT_ABI_FINGERPRINT_V1,
    "native generated-object ABI changed: bump the fingerprint revision and link symbols",
);
pub const GENERATED_OBJECT_ABI_GIL_SYMBOL: &str =
    "molt_generated_object_abi_5fce853bad8ac502_gil_v1";
pub const GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL: &str =
    "molt_generated_object_abi_5fce853bad8ac502_free_threaded_v1";
/// Compile-time authority consumed by runtime storage and generated native
/// access. Cargo feature unification may enable this through any dependency;
/// consumers must branch on this value rather than a crate-local feature.
pub const MOLT_FLAGS_ATOMIC: bool =
    cfg!(all(not(target_arch = "wasm32"), feature = "free-threaded"));
pub const GENERATED_OBJECT_ABI_SYMBOL: &str = if MOLT_FLAGS_ATOMIC {
    GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL
} else {
    GENERATED_OBJECT_ABI_GIL_SYMBOL
};

/// ABI-stable header flag storage: a zero-overhead `Cell` in deterministic
/// default GIL mode and on wasm32, and AtomicU32 only in explicit native
/// `free-threaded` builds.
/// Runtime helpers own semantic memory orderings; this type owns the target
/// representation and primitive operations.
#[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
#[repr(transparent)]
pub struct MoltFlags(core::sync::atomic::AtomicU32);

#[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
#[repr(transparent)]
pub struct MoltFlags(core::cell::Cell<u32>);

impl MoltFlags {
    #[inline(always)]
    pub const fn new(value: u32) -> Self {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            Self(core::sync::atomic::AtomicU32::new(value))
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            Self(core::cell::Cell::new(value))
        }
    }

    #[inline(always)]
    pub const fn new_unpublished(value: u32) -> Self {
        Self::new(value | HEADER_FLAG_GC_UNPUBLISHED)
    }

    #[inline(always)]
    pub fn load(&self, order: core::sync::atomic::Ordering) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.load(order)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            self.0.get()
        }
    }

    #[inline(always)]
    pub fn store(&self, value: u32, order: core::sync::atomic::Ordering) {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.store(value, order);
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            self.0.set(value);
        }
    }

    #[inline(always)]
    pub fn fetch_or(&self, value: u32, order: core::sync::atomic::Ordering) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.fetch_or(value, order)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            let previous = self.0.get();
            self.0.set(previous | value);
            previous
        }
    }

    #[inline(always)]
    pub fn fetch_and(&self, value: u32, order: core::sync::atomic::Ordering) -> u32 {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.fetch_and(value, order)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = order;
            let previous = self.0.get();
            self.0.set(previous & value);
            previous
        }
    }

    #[inline(always)]
    pub fn compare_exchange(
        &self,
        current: u32,
        new: u32,
        success: core::sync::atomic::Ordering,
        failure: core::sync::atomic::Ordering,
    ) -> Result<u32, u32> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
        {
            self.0.compare_exchange(current, new, success, failure)
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "free-threaded")))]
        {
            let _ = (success, failure);
            let observed = self.0.get();
            if observed == current {
                self.0.set(new);
                Ok(observed)
            } else {
                Err(observed)
            }
        }
    }

    /// Apply `(old | set) & !clear` as one coherent metadata transition with
    /// relaxed ordering. This is for flag facts whose payload visibility is
    /// guarded by object publication or another lock.
    #[inline(always)]
    pub fn update_relaxed(&self, set: u32, clear: u32) -> u32 {
        self.update_ordered(
            set,
            clear,
            core::sync::atomic::Ordering::Relaxed,
            core::sync::atomic::Ordering::Relaxed,
        )
    }

    /// Apply one state-machine transition with acquire/release synchronization.
    /// This class is reserved for flags that themselves publish or consume
    /// cross-thread state.
    #[inline(always)]
    pub fn update_synchronized(&self, set: u32, clear: u32) -> u32 {
        self.update_ordered(
            set,
            clear,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        )
    }

    #[inline(always)]
    fn update_ordered(
        &self,
        set: u32,
        clear: u32,
        success: core::sync::atomic::Ordering,
        failure: core::sync::atomic::Ordering,
    ) -> u32 {
        let mut observed = self.load(failure);
        loop {
            let updated = (observed | set) & !clear;
            if updated == observed {
                return observed;
            }
            match self.compare_exchange(observed, updated, success, failure) {
                Ok(previous) => return previous,
                Err(actual) => observed = actual,
            }
        }
    }

    /// Publish every initialized field to collectors and lock-free readers.
    #[inline(always)]
    pub fn publish_initialized(&self) {
        let _ = self.fetch_and(
            !HEADER_FLAG_GC_UNPUBLISHED,
            core::sync::atomic::Ordering::Release,
        );
    }

    /// Acquire the initialization publication before a collector reads edges.
    #[inline(always)]
    pub fn is_published(&self) -> bool {
        self.load(core::sync::atomic::Ordering::Acquire) & HEADER_FLAG_GC_UNPUBLISHED == 0
    }
}

const _: () = {
    assert!(core::mem::size_of::<MoltFlags>() == core::mem::size_of::<u32>());
    assert!(core::mem::align_of::<MoltFlags>() == core::mem::align_of::<u32>());
};

pub const TYPE_ID_OBJECT: u32 = 100;
pub const TYPE_ID_FUNCTION: u32 = 221;
pub const TYPE_ID_TYPE: u32 = 224;
pub const TYPE_ID_LIST_BOOL: u32 = 250;
pub const JIT_TYPE_ID_LIST_BOOL: i64 = TYPE_ID_LIST_BOOL as i64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanBoxConsts {
    pub qnan_tag_mask: i64,
    pub qnan_tag_int: i64,
    pub qnan_tag_ptr: i64,
    pub int_shift: i64,
    pub pointer_mask: i64,
    pub qnan_tag_bool: i64,
    pub int_width: i64,
    pub shift_48: i64,
    pub special_base: i64,
    pub special_limit: i64,
    pub int_tag_16: i64,
    pub int_mask: i64,
    pub shift_16: i64,
    pub canonical_nan: i64,
}

impl NanBoxConsts {
    pub const fn new() -> Self {
        Self {
            qnan_tag_mask: QNAN_TAG_MASK_I64,
            qnan_tag_int: QNAN_TAG_INT_I64,
            qnan_tag_ptr: QNAN_TAG_PTR_I64,
            int_shift: INT_SHIFT,
            pointer_mask: POINTER_MASK as i64,
            qnan_tag_bool: QNAN_TAG_BOOL_I64,
            int_width: INT_WIDTH as i64,
            shift_48: TAG_FIELD_SHIFT,
            special_base: SPECIAL_TAG_BASE,
            special_limit: SPECIAL_TAG_LIMIT,
            int_tag_16: ((QNAN | TAG_INT) >> 48) as i64,
            int_mask: INT_MASK as i64,
            shift_16: PTR_SIGN_EXT_SHIFT,
            canonical_nan: CANONICAL_NAN_BITS as i64,
        }
    }
}

impl Default for NanBoxConsts {
    fn default() -> Self {
        Self::new()
    }
}

pub fn box_int_bits(val: i64) -> i64 {
    let masked = (val as u64) & INT_MASK;
    (QNAN | TAG_INT | masked) as i64
}

pub fn box_float_bits(val: f64) -> i64 {
    if val.is_nan() {
        CANONICAL_NAN_BITS as i64
    } else {
        val.to_bits() as i64
    }
}

pub const fn box_bool_bits(val: i64) -> i64 {
    let bit = if val != 0 { 1u64 } else { 0u64 };
    (QNAN | TAG_BOOL | bit) as i64
}

pub const fn box_none_bits() -> i64 {
    QNAN_TAG_NONE_I64
}

pub const fn box_pending_bits() -> i64 {
    QNAN_TAG_PENDING_I64
}

pub const fn box_ptr_bits(addr: u64) -> i64 {
    (QNAN | TAG_PTR | (addr & POINTER_MASK)) as i64
}

pub const fn pending_bits() -> i64 {
    box_pending_bits()
}

pub const fn fits_inline_int(val: i64) -> bool {
    val >= INT_MIN_INLINE && val <= INT_MAX_INLINE
}

pub const fn tag_bits(bits: u64) -> u64 {
    bits & (QNAN | TAG_MASK)
}

pub const fn ptr_payload_bits(bits: u64) -> u64 {
    bits & POINTER_MASK
}

pub const fn canonical_addr_from_masked_bits(masked: u64) -> u64 {
    let signed = ((masked << PTR_SIGN_EXT_SHIFT) as i64) >> PTR_SIGN_EXT_SHIFT;
    signed as u64
}

pub const fn unbox_inline_int_bits(bits: u64) -> i64 {
    let val = bits & INT_MASK;
    if (val & INT_SIGN_BIT) != 0 {
        (val as i64) | !(INT_MASK as i64)
    } else {
        val as i64
    }
}

pub const fn unbox_bool_bits(bits: u64) -> i64 {
    (bits & 1) as i64
}

pub fn unbox_int_or_bool_bits(bits: u64) -> Option<i64> {
    if is_int_bits(bits) {
        Some(unbox_inline_int_bits(bits))
    } else if is_bool_bits(bits) {
        Some(unbox_bool_bits(bits))
    } else {
        None
    }
}

pub const fn is_float_bits(bits: u64) -> bool {
    (bits & QNAN) != QNAN
}

pub const fn is_int_bits(bits: u64) -> bool {
    tag_bits(bits) == QNAN_TAG_INT_I64 as u64
}

pub const fn is_bool_bits(bits: u64) -> bool {
    tag_bits(bits) == QNAN_TAG_BOOL_I64 as u64
}

pub const fn is_none_bits(bits: u64) -> bool {
    tag_bits(bits) == QNAN_TAG_NONE_I64 as u64
}

pub const fn is_pending_bits(bits: u64) -> bool {
    tag_bits(bits) == QNAN_TAG_PENDING_I64 as u64
}

pub const fn is_ptr_bits(bits: u64) -> bool {
    tag_bits(bits) == QNAN_TAG_PTR_I64 as u64
}

pub const fn is_special_bits(bits: u64) -> bool {
    let tag16 = (bits >> TAG_FIELD_SHIFT) as i64;
    let adjusted = tag16 - SPECIAL_TAG_BASE;
    adjusted >= 0 && adjusted < SPECIAL_TAG_LIMIT
}

pub fn stable_ic_site_id(func_name: &str, op_idx: usize, lane: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for b in func_name.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for b in lane.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^= op_idx as u64;
    hash = hash.wrapping_mul(FNV_PRIME);
    (hash & ((1u64 << 46) - 1)).max(1) as i64
}

const fn current_os() -> OperatingSystem {
    #[cfg(target_os = "windows")]
    {
        return OperatingSystem::Windows;
    }
    #[cfg(target_os = "macos")]
    {
        return OperatingSystem::Macos;
    }
    #[cfg(target_os = "linux")]
    {
        return OperatingSystem::Linux;
    }
    #[cfg(target_family = "wasm")]
    {
        return OperatingSystem::Wasm;
    }
    #[allow(unreachable_code)]
    OperatingSystem::Unknown
}

const fn current_arch() -> Architecture {
    #[cfg(target_arch = "x86_64")]
    {
        return Architecture::X86_64;
    }
    #[cfg(target_arch = "aarch64")]
    {
        return Architecture::Aarch64;
    }
    #[cfg(target_arch = "wasm32")]
    {
        return Architecture::Wasm32;
    }
    #[cfg(target_arch = "wasm64")]
    {
        return Architecture::Wasm64;
    }
    #[allow(unreachable_code)]
    Architecture::Unknown
}

const fn current_pointer_width() -> PointerWidth {
    #[cfg(target_pointer_width = "64")]
    {
        return PointerWidth::Bits64;
    }
    #[cfg(target_pointer_width = "32")]
    {
        return PointerWidth::Bits32;
    }
    #[allow(unreachable_code)]
    PointerWidth::Unknown
}

const fn current_endianness() -> Endianness {
    #[cfg(target_endian = "little")]
    {
        return Endianness::Little;
    }
    #[cfg(target_endian = "big")]
    {
        return Endianness::Big;
    }
    #[allow(unreachable_code)]
    Endianness::Little
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nanbox_constants_are_coherent() {
        assert_eq!(POINTER_MASK, 0x0000_FFFF_FFFF_FFFF);
        assert_eq!(INT_MASK, (1u64 << 47) - 1);
        assert_eq!(INT_MIN_INLINE, -(1_i64 << 46));
        assert_eq!(INT_MAX_INLINE, (1_i64 << 46) - 1);
        assert_eq!(INLINE_INT_BIAS, 1_i64 << 46);
        assert_eq!(INLINE_INT_LIMIT, 1_i64 << 47);
        assert_eq!(QNAN_TAG_INT_I64, (QNAN | TAG_INT) as i64);
        assert_eq!(NanBoxConsts::new().int_mask, INT_MASK as i64);
    }

    #[test]
    fn header_fields_match_the_packed_header_contract() {
        assert_eq!(core::mem::size_of::<MoltFlags>(), 4);
        assert_eq!(core::mem::align_of::<MoltFlags>(), 4);
        assert_eq!(
            MOLT_FLAGS_ATOMIC,
            cfg!(all(not(target_arch = "wasm32"), feature = "free-threaded"))
        );
        assert_eq!(HEADER_TYPE_ID_OFFSET, -HEADER_SIZE_BYTES);
        assert_eq!(HEADER_REFCOUNT_OFFSET, -(HEADER_SIZE_BYTES - 4));
        assert_eq!(HEADER_FLAGS_OFFSET, -(HEADER_SIZE_BYTES - 8));
        assert_eq!(HEADER_AUX_KIND_OFFSET, -(HEADER_SIZE_BYTES - 14));
        assert_eq!(HEADER_AUX_OFFSET, -(HEADER_SIZE_BYTES - 16));
        assert_eq!(HEADER_CLASS_WORD_BORROWED & HEADER_CLASS_WORD_TAG_MASK, 1);
        assert_eq!(HEADER_CLASS_WORD_BITS_MASK & HEADER_CLASS_WORD_TAG_MASK, 0);
        assert_eq!(
            [
                HEADER_AUX_KIND_NONE,
                HEADER_AUX_KIND_CLASS_INLINE,
                HEADER_AUX_KIND_STATE_INLINE,
                HEADER_AUX_KIND_SIDECAR,
            ],
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn every_generated_object_fact_is_fingerprint_significant() {
        let canonical = GENERATED_OBJECT_ABI_FACTS.words();
        assert_eq!(
            fingerprint_words(canonical),
            GENERATED_OBJECT_ABI_FINGERPRINT_V1
        );
        for index in 0..canonical.len() {
            let mut changed = canonical;
            changed[index] ^= 1;
            assert_ne!(
                fingerprint_words(changed),
                GENERATED_OBJECT_ABI_FINGERPRINT_V1,
                "generated-object ABI word {index} is not fingerprinted"
            );
        }
    }

    #[test]
    fn generated_object_link_symbols_embed_fingerprint_and_revision() {
        let fingerprint = std::format!("{:016x}", GENERATED_OBJECT_ABI_FINGERPRINT_V1);
        for symbol in [
            GENERATED_OBJECT_ABI_GIL_SYMBOL,
            GENERATED_OBJECT_ABI_FREE_THREADED_SYMBOL,
        ] {
            assert!(symbol.contains(&fingerprint), "{symbol}");
            assert!(symbol.ends_with("_v1"), "{symbol}");
        }
    }

    #[test]
    fn molt_flags_primitives_preserve_disjoint_bits() {
        use core::sync::atomic::Ordering;

        let flags = MoltFlags::new(HEADER_FLAG_HAS_PTRS);
        assert_eq!(
            flags.fetch_or(HEADER_FLAG_IMMORTAL, Ordering::AcqRel),
            HEADER_FLAG_HAS_PTRS
        );
        assert_eq!(
            flags.fetch_and(!HEADER_FLAG_HAS_PTRS, Ordering::AcqRel),
            HEADER_FLAG_HAS_PTRS | HEADER_FLAG_IMMORTAL
        );
        assert_eq!(flags.load(Ordering::Acquire), HEADER_FLAG_IMMORTAL);
    }

    #[test]
    fn explicit_ordering_classes_preserve_the_same_word_transition() {
        use core::sync::atomic::Ordering;

        let relaxed = MoltFlags::new(HEADER_FLAG_HAS_PTRS);
        let synchronized = MoltFlags::new(HEADER_FLAG_HAS_PTRS);
        assert_eq!(
            relaxed.update_relaxed(HEADER_FLAG_IMMORTAL, HEADER_FLAG_HAS_PTRS),
            HEADER_FLAG_HAS_PTRS
        );
        assert_eq!(
            synchronized.update_synchronized(HEADER_FLAG_IMMORTAL, HEADER_FLAG_HAS_PTRS),
            HEADER_FLAG_HAS_PTRS
        );
        assert_eq!(relaxed.load(Ordering::Relaxed), HEADER_FLAG_IMMORTAL);
        assert_eq!(synchronized.load(Ordering::Acquire), HEADER_FLAG_IMMORTAL);
    }

    #[test]
    fn two_phase_publication_is_explicit() {
        let flags = MoltFlags::new_unpublished(HEADER_FLAG_HAS_PTRS);
        assert!(!flags.is_published());
        flags.publish_initialized();
        assert!(flags.is_published());
        assert_ne!(
            flags.load(core::sync::atomic::Ordering::Acquire) & HEADER_FLAG_HAS_PTRS,
            0
        );
    }

    #[test]
    fn box_int_uses_inline_int_payload_width() {
        assert_eq!(box_int_bits(-1), (QNAN | TAG_INT | INT_MASK) as i64);
        assert_ne!(box_int_bits(-1), (QNAN | TAG_INT | POINTER_MASK) as i64);
        assert_eq!(
            unbox_inline_int_bits(box_int_bits(-1) as u64),
            -1,
            "signed 47-bit payload must round-trip"
        );
        assert!(fits_inline_int(INT_MIN_INLINE));
        assert!(fits_inline_int(INT_MAX_INLINE));
        assert!(!fits_inline_int(INT_MIN_INLINE - 1));
        assert!(!fits_inline_int(INT_MAX_INLINE + 1));
    }

    #[test]
    fn tag_predicates_decode_shared_bits() {
        assert!(is_int_bits(box_int_bits(42) as u64));
        assert!(is_bool_bits(box_bool_bits(1) as u64));
        assert!(is_none_bits(box_none_bits() as u64));
        assert!(is_pending_bits(box_pending_bits() as u64));
        assert_eq!(unbox_int_or_bool_bits(box_bool_bits(1) as u64), Some(1));
        assert!(is_float_bits(1.25f64.to_bits()));
        assert!(!is_float_bits(box_int_bits(0) as u64));
    }

    #[test]
    fn stable_site_id_is_deterministic_nonzero_and_inline() {
        let a = stable_ic_site_id("f", 12, "call_guarded");
        let b = stable_ic_site_id("f", 12, "call_guarded");
        let c = stable_ic_site_id("f", 12, "call_method");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!((1..=INT_MAX_INLINE).contains(&a));
    }

    #[test]
    fn host_platform_is_explicit() {
        assert_ne!(HOST_PLATFORM.pointer_width, PointerWidth::Unknown);
        assert_ne!(HOST_PLATFORM.arch, Architecture::Unknown);
    }
}
