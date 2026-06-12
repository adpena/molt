//! NaN-box tag / header / list-storage layout constants shared by the native
//! Cranelift JIT and the LLVM backend (moved verbatim from the inline
//! `native_backend_consts` module in lib.rs; `pub(super)` still targets the
//! crate root, which re-globs these via `use native_backend_consts::*`).

pub(super) const QNAN: u64 = 0x7ff8_0000_0000_0000;
pub(super) const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
pub(super) const TAG_INT: u64 = 0x0001_0000_0000_0000;
pub(super) const TAG_BOOL: u64 = 0x0002_0000_0000_0000;
pub(super) const TAG_NONE: u64 = 0x0003_0000_0000_0000;
pub(super) const TAG_PTR: u64 = 0x0004_0000_0000_0000;
pub(super) const TAG_PENDING: u64 = 0x0005_0000_0000_0000;
pub(super) const TAG_MASK: u64 = 0x0007_0000_0000_0000;
pub(super) const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

// ListIntStorage (#[repr(C)]) field offsets — MUST match
// runtime/molt-runtime/src/object/layout.rs.
// Layout: [data: *mut i64 @0, len: usize @8, cap: usize @16].
pub(super) const LIST_INT_STORAGE_DATA_OFFSET: i32 = 0;
pub(super) const LIST_INT_STORAGE_LEN_OFFSET: i32 = 8;
pub(super) const INT_WIDTH: u64 = 47;
pub(super) const INT_MASK: u64 = (1u64 << INT_WIDTH) - 1;
pub(super) const INT_SHIFT: i64 = (64 - INT_WIDTH) as i64;
pub(super) const GENERATOR_CONTROL_BYTES: i32 = 48;
pub(super) const TASK_KIND_FUTURE: i64 = 0;
pub(super) const TASK_KIND_GENERATOR: i64 = 1;
pub(super) const TASK_KIND_COROUTINE: i64 = 2;
// FUNC_DEFAULT_* constants moved to the runtime (molt_call_func_dispatch).
// Kept as dead_code in case the WASM backend needs them during outlining.
#[allow(dead_code)]
pub(super) const FUNC_DEFAULT_NONE: i64 = 1;
#[allow(dead_code)]
pub(super) const FUNC_DEFAULT_DICT_POP: i64 = 2;
#[allow(dead_code)]
pub(super) const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
pub(super) const HEADER_SIZE_BYTES: i32 = 24;
// MoltHeader layout (24 bytes total):
//   offset  0: type_id    (u32)
//   offset  4: ref_count  (u32 / AtomicU32)
//   offset  8: flags      (u32)
//   offset 12: size_class (u16)
//   offset 16: cold_idx   (u32)
//   offset 20: reserved   (u32)
// Data pointer = header_ptr + 24, so offsets from data_ptr are negative.
// NOTE: HEADER_STATE_OFFSET removed — state lives in cold header now;
// the native JIT uses molt_obj_get_state/molt_obj_set_state C API calls.
pub(super) const HEADER_TYPE_ID_OFFSET: i32 = -HEADER_SIZE_BYTES; // type_id @ header+0
pub(super) const HEADER_REFCOUNT_OFFSET: i32 = -(HEADER_SIZE_BYTES - 4);
pub(super) const HEADER_FLAGS_OFFSET: i32 = -(HEADER_SIZE_BYTES - 8);
pub(super) const HEADER_FLAG_IMMORTAL: u64 = 1 << 15;
/// TYPE_ID_LIST_BOOL (250) — used by the JIT to inline list_bool access.
pub(super) const JIT_TYPE_ID_LIST_BOOL: i64 = 250;
