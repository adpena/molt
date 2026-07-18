pub(crate) use super::heap_kinds_generated::*;

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
/// Index 0 is reserved for oversized allocations whose exact size lives in the
/// immutable aux sidecar.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_type_id_validator_covers_runtime_object_ids() {
        assert!(is_valid_heap_type_id(TYPE_ID_OBJECT));
        assert!(is_valid_heap_type_id(TYPE_ID_STRING));
        assert!(is_valid_heap_type_id(TYPE_ID_FLOAT));
        assert!(is_valid_heap_type_id(TYPE_ID_LIST_BOOL));
        assert!(is_valid_heap_type_id(TYPE_ID_GLOB_ITER));
        assert!(is_valid_heap_type_id(TYPE_ID_WEAK_CONTAINER_STATE));

        assert!(!is_valid_heap_type_id(0));
        assert!(!is_valid_heap_type_id(TYPE_ID_OBJECT - 1));
        assert!(!is_valid_heap_type_id(MAX_HEAP_TYPE_ID + 1));
    }
}
