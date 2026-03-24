//! TIR serialization for the incremental compilation cache.
//!
//! Provides two capabilities:
//!
//! 1. `content_hash` — a fast hash of a [`TirFunction`] used as the cache key.
//! 2. `serialize_ops` / `deserialize_ops` — round-trip [`OpIR`] slices to/from
//!    compact JSON bytes for on-disk storage in the [`crate::tir::cache`].
//!
//! `OpIR` derives `serde::Serialize` + `serde::Deserialize` unconditionally, so
//! `serde_json` is sufficient — no extra features are required.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::function::TirFunction;

// ---------------------------------------------------------------------------
// Content hash (cache key)
// ---------------------------------------------------------------------------

/// Compute a content hash of a TIR function for cache key purposes.
///
/// Hashes: function name, parameter count, block structure (block IDs,
/// op counts, and op discriminants + operand counts). This captures all
/// semantically meaningful changes while remaining fast to compute.
///
/// Not cryptographically strong — collision resistance is not required;
/// only false-negative avoidance matters (i.e., a changed function must
/// produce a different hash with overwhelming probability).
///
/// # Stability warning
/// `DefaultHasher` is not guaranteed to produce the same output across
/// different Rust versions. If the compiler is upgraded, existing on-disk
/// cache entries may silently produce hash misses, causing full recompilation.
/// For production use, replace `DefaultHasher` with a stable hasher such as
/// FNV (`fnv` crate) or AHash with a fixed seed to ensure cross-version
/// cache stability.
pub fn content_hash(func: &TirFunction) -> u64 {
    let mut hasher = DefaultHasher::new();
    func.name.hash(&mut hasher);
    func.param_types.len().hash(&mut hasher);
    // Sort block IDs for determinism regardless of HashMap iteration order.
    let mut block_ids: Vec<u32> = func.blocks.keys().map(|b| b.0).collect();
    block_ids.sort_unstable();
    for bid in block_ids {
        use super::blocks::BlockId;
        bid.hash(&mut hasher);
        if let Some(block) = func.blocks.get(&BlockId(bid)) {
            block.ops.len().hash(&mut hasher);
            for op in &block.ops {
                std::mem::discriminant(&op.opcode).hash(&mut hasher);
                op.operands.len().hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

// ---------------------------------------------------------------------------
// SimpleIR op serialization
// ---------------------------------------------------------------------------

/// Serialize a slice of [`crate::ir::OpIR`] to JSON bytes for cache storage.
///
/// Returns an empty `Vec` on serialization failure (which should be
/// impossible given that `OpIR` is entirely composed of primitive types).
pub fn serialize_ops(ops: &[crate::ir::OpIR]) -> Vec<u8> {
    serde_json::to_vec(ops).unwrap_or_default()
}

/// Deserialize [`crate::ir::OpIR`] from JSON bytes previously written by
/// [`serialize_ops`].
///
/// Returns `None` on any parse error, which causes the caller to skip the
/// cache and run the TIR pipeline fresh.
pub fn deserialize_ops(bytes: &[u8]) -> Option<Vec<crate::ir::OpIR>> {
    if bytes.is_empty() {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::OpIR;

    fn sample_ops() -> Vec<OpIR> {
        vec![
            OpIR {
                kind: "load_int".to_string(),
                value: Some(42),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["a".to_string(), "b".to_string()]),
                out: Some("c".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("c".to_string()),
                ..OpIR::default()
            },
        ]
    }

    #[test]
    fn test_serialize_roundtrip() {
        let ops = sample_ops();
        let bytes = serialize_ops(&ops);
        assert!(!bytes.is_empty(), "serialization must produce non-empty bytes");

        let restored = deserialize_ops(&bytes).expect("deserialization must succeed");
        assert_eq!(
            restored.len(),
            ops.len(),
            "op count must be preserved across round-trip"
        );
        for (orig, got) in ops.iter().zip(restored.iter()) {
            assert_eq!(orig.kind, got.kind, "op kind must survive round-trip");
            assert_eq!(orig.value, got.value, "integer value must survive round-trip");
            assert_eq!(orig.args, got.args, "args must survive round-trip");
            assert_eq!(orig.out, got.out, "out must survive round-trip");
        }
    }

    #[test]
    fn test_serialize_empty_ops() {
        let bytes = serialize_ops(&[]);
        // Empty JSON array is still non-empty bytes: b"[]"
        assert!(!bytes.is_empty());
        let restored = deserialize_ops(&bytes).expect("empty array round-trip");
        assert!(restored.is_empty());
    }

    #[test]
    fn test_deserialize_empty_bytes_returns_none() {
        assert!(
            deserialize_ops(&[]).is_none(),
            "empty byte slice must return None (no placeholder)"
        );
    }

    #[test]
    fn test_deserialize_invalid_bytes_returns_none() {
        assert!(deserialize_ops(b"not json").is_none());
        assert!(deserialize_ops(b"{\"not\": \"an array\"}").is_none());
    }

    #[test]
    fn test_op_with_float_value() {
        let ops = vec![OpIR {
            kind: "load_float".to_string(),
            f_value: Some(3.14),
            ..OpIR::default()
        }];
        let bytes = serialize_ops(&ops);
        let restored = deserialize_ops(&bytes).unwrap();
        assert_eq!(restored[0].f_value, Some(3.14));
    }

    #[test]
    fn test_op_with_bytes_payload() {
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let ops = vec![OpIR {
            kind: "const_bytes".to_string(),
            bytes: Some(payload.clone()),
            ..OpIR::default()
        }];
        let bytes = serialize_ops(&ops);
        let restored = deserialize_ops(&bytes).unwrap();
        assert_eq!(restored[0].bytes, Some(payload));
    }
}
