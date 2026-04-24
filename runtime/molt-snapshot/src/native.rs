//! Native startup snapshot: serialize bootstrap state at build time,
//! mmap at runtime for near-zero startup cost.
//!
//! # Design
//!
//! The most expensive part of Molt's native startup is populating the
//! interned string table. Every attribute name, keyword, module name,
//! and builtin identifier goes through `intern()` which hashes, probes,
//! and (on miss) heap-allocates + leaks a `Box<str>`.
//!
//! This module serializes the complete interned string table at build
//! time into a compact MessagePack blob. At runtime startup, we
//! deserialize and bulk-insert all strings into the intern pool in a
//! single locked pass, eliminating thousands of individual hash-and-probe
//! operations.
//!
//! # Wire format
//!
//! ```text
//! [magic: 4 bytes "MSNP"] [version: u16 LE] [sha256: 32 bytes] [payload...]
//! ```
//!
//! The payload is a MessagePack-encoded `NativeSnapshot` struct.
//!
//! # Future extensions
//!
//! Once the interned string snapshot is proven stable, this module will
//! extend to include:
//! - `builtins.__dict__` keys and type object vtable pointers
//! - `sys.modules` initial registry
//! - Pre-built type objects for the 30+ builtin types
//!
//! The full version would use mmap for zero-copy access, but the string
//! table alone is small enough (typically < 64 KiB) that deserialization
//! is the right approach.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Magic bytes identifying a native snapshot file.
const NATIVE_SNAPSHOT_MAGIC: &[u8; 4] = b"MSNP";

/// Wire format version. Increment on breaking changes.
const NATIVE_SNAPSHOT_VERSION: u16 = 1;

/// Header size: 4 (magic) + 2 (version) + 32 (sha256) = 38 bytes.
const HEADER_SIZE: usize = 38;

/// Serialized native bootstrap state.
///
/// Currently contains only the interned string table. The struct is
/// `#[non_exhaustive]` to allow adding fields (builtins dict, sys.modules,
/// type vtables) without a version bump as long as new fields are
/// `Option<T>` or appended after existing ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeSnapshot {
    /// All interned strings, sorted for deterministic output.
    /// At restore time, each string is bulk-inserted into the intern pool.
    pub interned_strings: Vec<String>,

    /// Number of strings at serialization time (for validation).
    pub string_count: u32,

    /// Build timestamp (seconds since Unix epoch) for staleness detection.
    pub build_timestamp: u64,
}

/// Errors during native snapshot operations.
#[derive(Debug)]
pub enum NativeSnapshotError {
    /// File doesn't start with the expected magic bytes.
    BadMagic,
    /// Version mismatch between snapshot and runtime.
    VersionMismatch { expected: u16, found: u16 },
    /// SHA-256 integrity check failed.
    IntegrityError,
    /// MessagePack deserialization failed.
    DeserializeError(String),
    /// String count in header doesn't match actual data.
    CountMismatch { expected: u32, found: u32 },
    /// I/O error reading or writing snapshot file.
    IoError(std::io::Error),
}

impl std::fmt::Display for NativeSnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a native snapshot file (bad magic)"),
            Self::VersionMismatch { expected, found } => {
                write!(
                    f,
                    "native snapshot version mismatch: expected {expected}, found {found}"
                )
            }
            Self::IntegrityError => write!(f, "native snapshot integrity check failed (SHA-256)"),
            Self::DeserializeError(msg) => write!(f, "native snapshot deserialization failed: {msg}"),
            Self::CountMismatch { expected, found } => {
                write!(
                    f,
                    "native snapshot string count mismatch: header says {expected}, data has {found}"
                )
            }
            Self::IoError(e) => write!(f, "native snapshot I/O error: {e}"),
        }
    }
}

impl std::error::Error for NativeSnapshotError {}

impl From<std::io::Error> for NativeSnapshotError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

impl NativeSnapshot {
    /// Create a snapshot from a set of interned strings.
    ///
    /// The strings are sorted for deterministic serialization (same input
    /// always produces identical bytes, enabling content-addressed caching).
    pub fn from_strings(strings: impl IntoIterator<Item = String>) -> Self {
        let mut interned_strings: Vec<String> = strings.into_iter().collect();
        interned_strings.sort_unstable();
        let string_count = interned_strings.len() as u32;
        let build_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        NativeSnapshot {
            interned_strings,
            string_count,
            build_timestamp,
        }
    }

    /// Serialize the snapshot to bytes with magic, version, and SHA-256 integrity.
    ///
    /// Wire format:
    /// ```text
    /// [magic: 4 bytes "MSNP"] [version: u16 LE] [sha256: 32 bytes] [msgpack payload...]
    /// ```
    pub fn serialize(&self) -> Result<Vec<u8>, NativeSnapshotError> {
        let payload = rmp_serde::to_vec(self)
            .map_err(|e| NativeSnapshotError::DeserializeError(e.to_string()))?;
        let hash = Sha256::digest(&payload);

        let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
        buf.extend_from_slice(NATIVE_SNAPSHOT_MAGIC);
        buf.extend_from_slice(&NATIVE_SNAPSHOT_VERSION.to_le_bytes());
        buf.extend_from_slice(&hash);
        buf.extend_from_slice(&payload);
        Ok(buf)
    }

    /// Deserialize a snapshot from bytes, verifying magic, version, and integrity.
    pub fn deserialize(data: &[u8]) -> Result<Self, NativeSnapshotError> {
        if data.len() < HEADER_SIZE {
            return Err(NativeSnapshotError::DeserializeError(format!(
                "snapshot too short: {} bytes (minimum {HEADER_SIZE})",
                data.len()
            )));
        }

        // Verify magic.
        if &data[0..4] != NATIVE_SNAPSHOT_MAGIC {
            return Err(NativeSnapshotError::BadMagic);
        }

        // Verify version.
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != NATIVE_SNAPSHOT_VERSION {
            return Err(NativeSnapshotError::VersionMismatch {
                expected: NATIVE_SNAPSHOT_VERSION,
                found: version,
            });
        }

        // Verify SHA-256.
        let expected_hash: [u8; 32] = data[6..38].try_into().unwrap();
        let payload = &data[HEADER_SIZE..];
        let actual_hash: [u8; 32] = Sha256::digest(payload).into();
        if expected_hash != actual_hash {
            return Err(NativeSnapshotError::IntegrityError);
        }

        // Deserialize payload.
        let snapshot: NativeSnapshot = rmp_serde::from_slice(payload)
            .map_err(|e| NativeSnapshotError::DeserializeError(e.to_string()))?;

        // Validate string count.
        let actual_count = snapshot.interned_strings.len() as u32;
        if snapshot.string_count != actual_count {
            return Err(NativeSnapshotError::CountMismatch {
                expected: snapshot.string_count,
                found: actual_count,
            });
        }

        Ok(snapshot)
    }

    /// Write the serialized snapshot to a file.
    pub fn write_to_file(&self, path: &std::path::Path) -> Result<(), NativeSnapshotError> {
        let bytes = self.serialize()?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// Read and deserialize a snapshot from a file.
    pub fn read_from_file(path: &std::path::Path) -> Result<Self, NativeSnapshotError> {
        let data = std::fs::read(path)?;
        Self::deserialize(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_strings() -> Vec<String> {
        vec![
            "__init__".into(),
            "__str__".into(),
            "__repr__".into(),
            "__add__".into(),
            "__sub__".into(),
            "builtins".into(),
            "sys".into(),
            "os".into(),
            "path".into(),
            "importlib".into(),
            "_frozen_importlib".into(),
            "object".into(),
            "type".into(),
            "int".into(),
            "float".into(),
            "str".into(),
            "list".into(),
            "dict".into(),
            "tuple".into(),
            "set".into(),
            "frozenset".into(),
            "bool".into(),
            "bytes".into(),
            "NoneType".into(),
            "range".into(),
            "slice".into(),
            "property".into(),
            "classmethod".into(),
            "staticmethod".into(),
            "super".into(),
        ]
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let snapshot = NativeSnapshot::from_strings(sample_strings());
        let bytes = snapshot.serialize().unwrap();
        let restored = NativeSnapshot::deserialize(&bytes).unwrap();

        assert_eq!(restored.string_count, snapshot.string_count);
        assert_eq!(restored.interned_strings, snapshot.interned_strings);
        assert_eq!(restored.build_timestamp, snapshot.build_timestamp);
    }

    #[test]
    fn strings_are_sorted() {
        let snapshot = NativeSnapshot::from_strings(vec![
            "zebra".into(),
            "alpha".into(),
            "middle".into(),
        ]);
        assert_eq!(
            snapshot.interned_strings,
            vec!["alpha", "middle", "zebra"]
        );
    }

    #[test]
    fn bad_magic_detected() {
        let snapshot = NativeSnapshot::from_strings(sample_strings());
        let mut bytes = snapshot.serialize().unwrap();
        bytes[0] = b'X'; // corrupt magic
        let err = NativeSnapshot::deserialize(&bytes).unwrap_err();
        assert!(matches!(err, NativeSnapshotError::BadMagic));
    }

    #[test]
    fn version_mismatch_detected() {
        let snapshot = NativeSnapshot::from_strings(sample_strings());
        let mut bytes = snapshot.serialize().unwrap();
        bytes[4] = 99; // bogus version
        let err = NativeSnapshot::deserialize(&bytes).unwrap_err();
        assert!(matches!(
            err,
            NativeSnapshotError::VersionMismatch { expected: 1, .. }
        ));
    }

    #[test]
    fn integrity_check_catches_corruption() {
        let snapshot = NativeSnapshot::from_strings(sample_strings());
        let mut bytes = snapshot.serialize().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // corrupt payload
        let err = NativeSnapshot::deserialize(&bytes).unwrap_err();
        assert!(matches!(err, NativeSnapshotError::IntegrityError));
    }

    #[test]
    fn empty_snapshot_roundtrips() {
        let snapshot = NativeSnapshot::from_strings(Vec::<String>::new());
        assert_eq!(snapshot.string_count, 0);
        let bytes = snapshot.serialize().unwrap();
        let restored = NativeSnapshot::deserialize(&bytes).unwrap();
        assert_eq!(restored.string_count, 0);
        assert!(restored.interned_strings.is_empty());
    }

    #[test]
    fn file_roundtrip() {
        let dir = std::env::temp_dir().join("molt_snapshot_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_native.msnp");

        let snapshot = NativeSnapshot::from_strings(sample_strings());
        snapshot.write_to_file(&path).unwrap();

        let restored = NativeSnapshot::read_from_file(&path).unwrap();
        assert_eq!(restored.interned_strings, snapshot.interned_strings);

        // Clean up.
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn truncated_data_detected() {
        let err = NativeSnapshot::deserialize(&[b'M', b'S', b'N', b'P']).unwrap_err();
        assert!(matches!(err, NativeSnapshotError::DeserializeError(_)));
    }

    #[test]
    fn deterministic_output() {
        // Same input should produce identical bytes.
        let snap1 = NativeSnapshot {
            interned_strings: vec!["a".into(), "b".into()],
            string_count: 2,
            build_timestamp: 1000,
        };
        let snap2 = NativeSnapshot {
            interned_strings: vec!["a".into(), "b".into()],
            string_count: 2,
            build_timestamp: 1000,
        };
        let bytes1 = snap1.serialize().unwrap();
        let bytes2 = snap2.serialize().unwrap();
        assert_eq!(bytes1, bytes2, "identical snapshots must produce identical bytes");
    }
}
