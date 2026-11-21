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
pub(crate) const TYPE_TAG_BUFFER2D: i64 = 14;
pub(crate) const TYPE_TAG_MEMORYVIEW: i64 = 15;
// Tag 16 is retired with the `intarray` heap kind and is never reused.
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

// The size-class table and its lookup are `molt_codegen_abi`'s: compiled
// allocation uses the same classes.
pub(crate) use molt_codegen_abi::object_lifecycle::{SIZE_CLASS_TABLE, size_class_for};

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
        for retired in [205, 231] {
            assert!(!is_valid_heap_type_id(retired));
            assert!(heap_kind_descriptor(retired).is_none());
            assert!(heap_drop_policy(retired).is_none());
        }
        assert_eq!(TYPE_ID_TUPLE, 206);
        assert_eq!(TYPE_ID_FROZENSET, 232);
        for active in ALL_HEAP_TYPE_IDS {
            assert_eq!(heap_kind_descriptor(active).unwrap().type_id, active);
        }
    }
}
