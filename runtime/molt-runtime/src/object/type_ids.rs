pub(crate) const TYPE_ID_STRING: u32 = 200;
pub(crate) const TYPE_ID_OBJECT: u32 = 100;
pub(crate) const TYPE_ID_LIST: u32 = 201;
pub(crate) const TYPE_ID_BYTES: u32 = 202;
pub(crate) const TYPE_ID_LIST_BUILDER: u32 = 203;
pub(crate) const TYPE_ID_DICT: u32 = 204;
pub(crate) const TYPE_ID_DICT_BUILDER: u32 = 205;
pub(crate) const TYPE_ID_TUPLE: u32 = 206;
pub(crate) const TYPE_ID_DICT_KEYS_VIEW: u32 = 207;
pub(crate) const TYPE_ID_DICT_VALUES_VIEW: u32 = 208;
pub(crate) const TYPE_ID_DICT_ITEMS_VIEW: u32 = 209;
pub(crate) const TYPE_ID_ITER: u32 = 210;
pub(crate) const TYPE_ID_BYTEARRAY: u32 = 211;
pub(crate) const TYPE_ID_RANGE: u32 = 212;
pub(crate) const TYPE_ID_SLICE: u32 = 213;
pub(crate) const TYPE_ID_EXCEPTION: u32 = 214;
pub(crate) const TYPE_ID_DATACLASS: u32 = 215;
pub(crate) const TYPE_ID_BUFFER2D: u32 = 216;
pub(crate) const TYPE_ID_CONTEXT_MANAGER: u32 = 217;
pub(crate) const TYPE_ID_FILE_HANDLE: u32 = 218;
pub(crate) const TYPE_ID_MEMORYVIEW: u32 = 219;
pub(crate) const TYPE_ID_INTARRAY: u32 = 220;
pub(crate) const TYPE_ID_FUNCTION: u32 = 221;
pub(crate) const TYPE_ID_BOUND_METHOD: u32 = 222;
pub(crate) const TYPE_ID_MODULE: u32 = 223;
pub(crate) const TYPE_ID_TYPE: u32 = 224;
pub(crate) const TYPE_ID_GENERATOR: u32 = 225;
pub(crate) const TYPE_ID_CLASSMETHOD: u32 = 226;
pub(crate) const TYPE_ID_STATICMETHOD: u32 = 227;
pub(crate) const TYPE_ID_PROPERTY: u32 = 228;
pub(crate) const TYPE_ID_SUPER: u32 = 229;
pub(crate) const TYPE_ID_SET: u32 = 230;
pub(crate) const TYPE_ID_SET_BUILDER: u32 = 231;
pub(crate) const TYPE_ID_FROZENSET: u32 = 232;
pub(crate) const TYPE_ID_BIGINT: u32 = 233;
pub(crate) const TYPE_ID_COMPLEX: u32 = 234;
pub(crate) const TYPE_ID_ENUMERATE: u32 = 235;
pub(crate) const TYPE_ID_CALLARGS: u32 = 236;
pub(crate) const TYPE_ID_NOT_IMPLEMENTED: u32 = 237;
pub(crate) const TYPE_ID_CALL_ITER: u32 = 238;
pub(crate) const TYPE_ID_REVERSED: u32 = 239;
pub(crate) const TYPE_ID_ZIP: u32 = 240;
pub(crate) const TYPE_ID_MAP: u32 = 241;
pub(crate) const TYPE_ID_FILTER: u32 = 242;
pub(crate) const TYPE_ID_CODE: u32 = 243;
pub(crate) const TYPE_ID_ELLIPSIS: u32 = 244;
pub(crate) const TYPE_ID_GENERIC_ALIAS: u32 = 245;
pub(crate) const TYPE_ID_ASYNC_GENERATOR: u32 = 246;
pub(crate) const TYPE_ID_UNION: u32 = 247;

pub(crate) const TYPE_TAG_ANY: i64 = 0;
pub(crate) const TYPE_TAG_INT: i64 = 1;
pub(crate) const TYPE_TAG_FLOAT: i64 = 2;
pub(crate) const TYPE_TAG_BOOL: i64 = 3;
pub(crate) const TYPE_TAG_NONE: i64 = 4;
pub(crate) const TYPE_TAG_STR: i64 = 5;
pub(crate) const TYPE_TAG_BYTES: i64 = 6;
pub(crate) const TYPE_TAG_BYTEARRAY: i64 = 7;
pub(crate) const TYPE_TAG_LIST: i64 = 8;
pub(crate) const TYPE_TAG_TUPLE: i64 = 9;
pub(crate) const TYPE_TAG_DICT: i64 = 10;
pub(crate) const TYPE_TAG_RANGE: i64 = 11;
pub(crate) const TYPE_TAG_SLICE: i64 = 12;
pub(crate) const TYPE_TAG_DATACLASS: i64 = 13;
pub(crate) const TYPE_TAG_BUFFER2D: i64 = 14;
pub(crate) const TYPE_TAG_MEMORYVIEW: i64 = 15;
pub(crate) const TYPE_TAG_INTARRAY: i64 = 16;
pub(crate) const TYPE_TAG_SET: i64 = 17;
pub(crate) const TYPE_TAG_FROZENSET: i64 = 18;
pub(crate) const TYPE_TAG_COMPLEX: i64 = 19;

pub(crate) const BUILTIN_TAG_OBJECT: i64 = 100;
pub(crate) const BUILTIN_TAG_TYPE: i64 = 101;
pub(crate) const BUILTIN_TAG_BASE_EXCEPTION: i64 = 102;
pub(crate) const BUILTIN_TAG_EXCEPTION: i64 = 103;
pub(crate) const BUILTIN_TAG_CLASSMETHOD: i64 = 226;
pub(crate) const BUILTIN_TAG_STATICMETHOD: i64 = 227;
pub(crate) const BUILTIN_TAG_PROPERTY: i64 = 228;
pub(crate) const BUILTIN_TAG_SUPER: i64 = 229;

// ---------------------------------------------------------------------------
// Size-class infrastructure for compact header size encoding
// ---------------------------------------------------------------------------

/// Predefined size classes (in bytes) for object allocations.
/// Index 0 is reserved for "oversized" (exact size stored in cold header).
/// Indices 1..=N map to common allocation sizes up to 64 KB.
pub(crate) const SIZE_CLASS_TABLE: &[usize] = &[
    0, // 0: sentinel / oversized
    8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 128, 144, 160, 176, 192, 208,
    224, 240, 256, 288, 320, 352, 384, 416, 448, 480, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
    1152, 1280, 1408, 1536, 1664, 1792, 1920, 2048, 2304, 2560, 2816, 3072, 3328, 3584, 3840, 4096,
    4608, 5120, 5632, 6144, 6656, 7168, 7680, 8192, 9216, 10240, 11264, 12288, 13312, 14336, 15360,
    16384, 20480, 24576, 28672, 32768, 40960, 49152, 57344, 65536,
];

/// Map an allocation size (in bytes) to a `u16` size-class index.
///
/// Returns 0 (oversized sentinel) when `size` exceeds the largest class.
/// Otherwise returns the smallest class index whose value >= `size`.
pub(crate) fn size_class_for(size: usize) -> u16 {
    // Linear scan is fine: the table has < 90 entries and this is called
    // once per allocation, not on the hot refcount path.
    for (i, &class_size) in SIZE_CLASS_TABLE.iter().enumerate().skip(1) {
        if class_size >= size {
            return i as u16;
        }
    }
    0 // oversized
}
