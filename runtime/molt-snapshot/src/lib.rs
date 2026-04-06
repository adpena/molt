//! WASM execution state snapshot and resume.
//!
//! Enables pausing Molt-compiled WASM execution at external call boundaries
//! (database queries, HTTP requests, AI model calls), serializing the full
//! execution state, and resuming later — possibly on a different machine.
//!
//! The serialization format uses a versioned wire protocol with SHA-256
//! integrity checking, inspired by Monty's postcard-based snapshot system.

use sha2::{Digest, Sha256};

/// Wire format version. Increment on breaking changes.
const SNAPSHOT_VERSION: u16 = 1;

/// Execution state at a yield point.
#[derive(Debug, Clone)]
pub struct ExecutionSnapshot {
    /// Wire format version for forward compatibility.
    pub version: u16,
    /// WASM linear memory contents at the yield point.
    pub memory: Vec<u8>,
    /// WASM global variable values (NaN-boxed i64s).
    pub globals: Vec<u64>,
    /// WASM table entries (function indices).
    pub table: Vec<u32>,
    /// Current program counter (function index + instruction offset).
    pub pc: ProgramCounter,
    /// The external call that caused the yield.
    pub pending_call: PendingExternalCall,
    /// Resource tracker state at yield time.
    pub resource_state: ResourceSnapshot,
}

/// Program counter at yield point.
#[derive(Debug, Clone)]
pub struct ProgramCounter {
    /// WASM function index.
    pub func_index: u32,
    /// Byte offset within the function body.
    pub instruction_offset: u32,
    /// Call stack depth at yield.
    pub call_depth: u32,
}

/// The external function call that caused execution to yield.
#[derive(Debug, Clone)]
pub struct PendingExternalCall {
    /// Name of the external function (e.g., "fetch_data", "query_db").
    pub function_name: String,
    /// Serialized arguments (NaN-boxed values).
    pub args: Vec<u64>,
    /// Call ID for correlation.
    pub call_id: u64,
}

/// Resource tracker state preserved across snapshots.
#[derive(Debug, Clone, Default)]
pub struct ResourceSnapshot {
    pub allocation_count: usize,
    pub memory_used: usize,
    pub elapsed_ms: u64,
}

/// Result of resuming from a snapshot.
#[derive(Debug)]
pub enum ResumeResult {
    /// Execution completed with a return value.
    Complete { return_value: u64 },
    /// Execution yielded again at another external call.
    Yielded(ExecutionSnapshot),
    /// Execution failed with an error.
    Error { message: String },
}

/// Errors during snapshot operations.
#[derive(Debug)]
pub enum SnapshotError {
    /// Version mismatch between snapshot and runtime.
    VersionMismatch { expected: u16, found: u16 },
    /// SHA-256 integrity check failed.
    IntegrityError { expected: [u8; 32], found: [u8; 32] },
    /// Snapshot data is truncated or malformed.
    MalformedData { message: String },
    /// The snapshot references functions/memory not present in the current module.
    IncompatibleModule { message: String },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionMismatch { expected, found } => {
                write!(
                    f,
                    "snapshot version mismatch: expected {expected}, found {found}"
                )
            }
            Self::IntegrityError { .. } => {
                write!(f, "snapshot integrity check failed (SHA-256 mismatch)")
            }
            Self::MalformedData { message } => write!(f, "malformed snapshot: {message}"),
            Self::IncompatibleModule { message } => write!(f, "incompatible module: {message}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl ExecutionSnapshot {
    /// Serialize the snapshot to bytes with version header and SHA-256 integrity.
    ///
    /// Wire format:
    /// ```text
    /// [version: u16 LE] [sha256: 32 bytes] [payload...]
    /// ```
    pub fn serialize(&self) -> Vec<u8> {
        let payload = self.serialize_payload();
        let hash = Sha256::digest(&payload);

        let mut buf = Vec::with_capacity(2 + 32 + payload.len());
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&hash);
        buf.extend_from_slice(&payload);
        buf
    }

    /// Deserialize a snapshot from bytes, verifying version and integrity.
    pub fn deserialize(data: &[u8]) -> Result<Self, SnapshotError> {
        if data.len() < 34 {
            return Err(SnapshotError::MalformedData {
                message: format!("snapshot too short: {} bytes (minimum 34)", data.len()),
            });
        }

        let version = u16::from_le_bytes([data[0], data[1]]);
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotError::VersionMismatch {
                expected: SNAPSHOT_VERSION,
                found: version,
            });
        }

        let expected_hash: [u8; 32] = data[2..34].try_into().unwrap();
        let payload = &data[34..];
        let actual_hash: [u8; 32] = Sha256::digest(payload).into();

        if expected_hash != actual_hash {
            return Err(SnapshotError::IntegrityError {
                expected: expected_hash,
                found: actual_hash,
            });
        }

        Self::deserialize_payload(payload, version)
    }

    fn serialize_payload(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Memory
        write_u32(&mut buf, self.memory.len() as u32);
        buf.extend_from_slice(&self.memory);

        // Globals
        write_u32(&mut buf, self.globals.len() as u32);
        for &g in &self.globals {
            buf.extend_from_slice(&g.to_le_bytes());
        }

        // Table
        write_u32(&mut buf, self.table.len() as u32);
        for &t in &self.table {
            buf.extend_from_slice(&t.to_le_bytes());
        }

        // Program counter
        buf.extend_from_slice(&self.pc.func_index.to_le_bytes());
        buf.extend_from_slice(&self.pc.instruction_offset.to_le_bytes());
        buf.extend_from_slice(&self.pc.call_depth.to_le_bytes());

        // Pending call
        write_str(&mut buf, &self.pending_call.function_name);
        write_u32(&mut buf, self.pending_call.args.len() as u32);
        for &a in &self.pending_call.args {
            buf.extend_from_slice(&a.to_le_bytes());
        }
        buf.extend_from_slice(&self.pending_call.call_id.to_le_bytes());

        // Resource state
        buf.extend_from_slice(&(self.resource_state.allocation_count as u64).to_le_bytes());
        buf.extend_from_slice(&(self.resource_state.memory_used as u64).to_le_bytes());
        buf.extend_from_slice(&self.resource_state.elapsed_ms.to_le_bytes());

        buf
    }

    fn deserialize_payload(data: &[u8], version: u16) -> Result<Self, SnapshotError> {
        let mut cursor = 0;

        let memory_len = read_u32(data, &mut cursor)? as usize;
        if cursor + memory_len > data.len() {
            return Err(SnapshotError::MalformedData {
                message: "memory truncated".into(),
            });
        }
        let memory = data[cursor..cursor + memory_len].to_vec();
        cursor += memory_len;

        let globals_len = read_u32(data, &mut cursor)? as usize;
        let mut globals = Vec::with_capacity(globals_len);
        for _ in 0..globals_len {
            globals.push(read_u64(data, &mut cursor)?);
        }

        let table_len = read_u32(data, &mut cursor)? as usize;
        let mut table = Vec::with_capacity(table_len);
        for _ in 0..table_len {
            table.push(read_u32(data, &mut cursor)?);
        }

        let pc = ProgramCounter {
            func_index: read_u32(data, &mut cursor)?,
            instruction_offset: read_u32(data, &mut cursor)?,
            call_depth: read_u32(data, &mut cursor)?,
        };

        let function_name = read_str(data, &mut cursor)?;
        let args_len = read_u32(data, &mut cursor)? as usize;
        let mut args = Vec::with_capacity(args_len);
        for _ in 0..args_len {
            args.push(read_u64(data, &mut cursor)?);
        }
        let call_id = read_u64(data, &mut cursor)?;

        let pending_call = PendingExternalCall {
            function_name,
            args,
            call_id,
        };

        let resource_state = ResourceSnapshot {
            allocation_count: read_u64(data, &mut cursor)? as usize,
            memory_used: read_u64(data, &mut cursor)? as usize,
            elapsed_ms: read_u64(data, &mut cursor)?,
        };

        Ok(ExecutionSnapshot {
            version,
            memory,
            globals,
            table,
            pc,
            pending_call,
            resource_state,
        })
    }
}

// Serialization helpers
fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn write_str(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32, SnapshotError> {
    if *cursor + 4 > data.len() {
        return Err(SnapshotError::MalformedData {
            message: "truncated u32".into(),
        });
    }
    let v = u32::from_le_bytes(data[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    Ok(v)
}

fn read_u64(data: &[u8], cursor: &mut usize) -> Result<u64, SnapshotError> {
    if *cursor + 8 > data.len() {
        return Err(SnapshotError::MalformedData {
            message: "truncated u64".into(),
        });
    }
    let v = u64::from_le_bytes(data[*cursor..*cursor + 8].try_into().unwrap());
    *cursor += 8;
    Ok(v)
}

fn read_str(data: &[u8], cursor: &mut usize) -> Result<String, SnapshotError> {
    let len = read_u32(data, cursor)? as usize;
    if *cursor + len > data.len() {
        return Err(SnapshotError::MalformedData {
            message: "truncated string".into(),
        });
    }
    let s = String::from_utf8(data[*cursor..*cursor + len].to_vec()).map_err(|e| {
        SnapshotError::MalformedData {
            message: format!("invalid UTF-8: {e}"),
        }
    })?;
    *cursor += len;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> ExecutionSnapshot {
        ExecutionSnapshot {
            version: SNAPSHOT_VERSION,
            memory: vec![0u8; 64],
            globals: vec![42, 0x7ff8_0001_0000_0000],
            table: vec![0, 1, 2],
            pc: ProgramCounter {
                func_index: 5,
                instruction_offset: 128,
                call_depth: 3,
            },
            pending_call: PendingExternalCall {
                function_name: "fetch_data".into(),
                args: vec![0x7ff8_0001_0000_002a],
                call_id: 12345,
            },
            resource_state: ResourceSnapshot {
                allocation_count: 100,
                memory_used: 4096,
                elapsed_ms: 500,
            },
        }
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let snap = sample_snapshot();
        let bytes = snap.serialize();
        let restored = ExecutionSnapshot::deserialize(&bytes).unwrap();
        assert_eq!(restored.version, SNAPSHOT_VERSION);
        assert_eq!(restored.memory.len(), 64);
        assert_eq!(restored.globals, vec![42, 0x7ff8_0001_0000_0000]);
        assert_eq!(restored.table, vec![0, 1, 2]);
        assert_eq!(restored.pc.func_index, 5);
        assert_eq!(restored.pc.instruction_offset, 128);
        assert_eq!(restored.pending_call.function_name, "fetch_data");
        assert_eq!(restored.pending_call.call_id, 12345);
        assert_eq!(restored.resource_state.allocation_count, 100);
    }

    #[test]
    fn integrity_check_catches_corruption() {
        let snap = sample_snapshot();
        let mut bytes = snap.serialize();
        // Corrupt one byte in the payload
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let err = ExecutionSnapshot::deserialize(&bytes).unwrap_err();
        assert!(matches!(err, SnapshotError::IntegrityError { .. }));
    }

    #[test]
    fn version_mismatch_detected() {
        let snap = sample_snapshot();
        let mut bytes = snap.serialize();
        bytes[0] = 99; // bogus version
        let err = ExecutionSnapshot::deserialize(&bytes).unwrap_err();
        assert!(matches!(
            err,
            SnapshotError::VersionMismatch {
                expected: 1,
                found: _
            }
        ));
    }

    #[test]
    fn truncated_data_detected() {
        let err = ExecutionSnapshot::deserialize(&[1, 0]).unwrap_err();
        assert!(matches!(err, SnapshotError::MalformedData { .. }));
    }

    #[test]
    fn empty_snapshot_roundtrips() {
        let snap = ExecutionSnapshot {
            version: SNAPSHOT_VERSION,
            memory: vec![],
            globals: vec![],
            table: vec![],
            pc: ProgramCounter {
                func_index: 0,
                instruction_offset: 0,
                call_depth: 0,
            },
            pending_call: PendingExternalCall {
                function_name: String::new(),
                args: vec![],
                call_id: 0,
            },
            resource_state: ResourceSnapshot::default(),
        };
        let bytes = snap.serialize();
        let restored = ExecutionSnapshot::deserialize(&bytes).unwrap();
        assert_eq!(restored.memory.len(), 0);
        assert_eq!(restored.globals.len(), 0);
    }
}
