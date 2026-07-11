#![no_std]

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
pub const HEADER_COLD_IDX_OFFSET: i32 = -(HEADER_SIZE_BYTES - 16);

pub const HEADER_FLAG_HAS_PTRS: u32 = 1;
pub const HEADER_FLAG_SKIP_CLASS_DECREF: u32 = 1 << 1;
pub const HEADER_FLAG_IMMORTAL: u32 = 1 << 15;
pub const HEADER_FLAG_CONTAINS_REFS: u32 = 1 << 19;

pub const TYPE_ID_OBJECT: u32 = 100;
pub const TYPE_ID_FUNCTION: u32 = 221;
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
