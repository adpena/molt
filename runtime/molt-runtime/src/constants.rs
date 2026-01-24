use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};

use crate::object::PtrSlot;

// Keep in sync with MOLT_BIND_KIND_OPEN in src/molt/frontend/__init__.py.
pub(crate) const BIND_KIND_OPEN: i64 = 1;

#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_BASE: u64 = 256;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_ASYNC_SLEEP: u64 = WASM_TABLE_BASE + 1;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_ANEXT_DEFAULT_POLL: u64 = WASM_TABLE_BASE + 2;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_ASYNCGEN_POLL: u64 = WASM_TABLE_BASE + 3;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_PROMISE_POLL: u64 = WASM_TABLE_BASE + 4;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_IO_WAIT: u64 = WASM_TABLE_BASE + 5;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_THREAD_POLL: u64 = WASM_TABLE_BASE + 6;
#[cfg(target_arch = "wasm32")]
pub(crate) const WASM_TABLE_IDX_PROCESS_POLL: u64 = WASM_TABLE_BASE + 7;

pub(crate) const INLINE_INT_MIN_I128: i128 = -(1_i128 << 46);
pub(crate) const INLINE_INT_MAX_I128: i128 = (1_i128 << 46) - 1;
pub(crate) const MAX_SMALL_LIST: usize = 16;
pub(crate) const ITER_EXHAUSTED: usize = usize::MAX;

pub(crate) const FUNC_DEFAULT_NONE: i64 = 1;
pub(crate) const FUNC_DEFAULT_DICT_POP: i64 = 2;
pub(crate) const FUNC_DEFAULT_DICT_UPDATE: i64 = 3;
pub(crate) const FUNC_DEFAULT_REPLACE_COUNT: i64 = 4;
pub(crate) const FUNC_DEFAULT_NEG_ONE: i64 = 5;
pub(crate) const FUNC_DEFAULT_ZERO: i64 = 6;
pub(crate) const FUNC_DEFAULT_MISSING: i64 = 7;

pub(crate) const GEN_SEND_OFFSET: usize = 0;
pub(crate) const GEN_THROW_OFFSET: usize = 8;
pub(crate) const GEN_CLOSED_OFFSET: usize = 16;
pub(crate) const GEN_EXC_DEPTH_OFFSET: usize = 24;
pub(crate) const GEN_CONTROL_SIZE: usize = 48;

pub(crate) const ASYNCGEN_GEN_OFFSET: usize = 0;
pub(crate) const ASYNCGEN_RUNNING_OFFSET: usize = 8;
pub(crate) const ASYNCGEN_PENDING_OFFSET: usize = 16;
pub(crate) const ASYNCGEN_CONTROL_SIZE: usize = 24;
pub(crate) const ASYNCGEN_OP_ANEXT: i64 = 0;
pub(crate) const ASYNCGEN_OP_ASEND: i64 = 1;
pub(crate) const ASYNCGEN_OP_ATHROW: i64 = 2;
pub(crate) const ASYNCGEN_OP_ACLOSE: i64 = 3;

pub(crate) const TASK_KIND_FUTURE: u64 = 0;
pub(crate) const TASK_KIND_GENERATOR: u64 = 1;

pub(crate) static CALL_DISPATCH_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static STRUCT_FIELD_STORE_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ATTR_LOOKUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static HANDLE_RESOLVE_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAYOUT_GUARD_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAYOUT_GUARD_FAIL: AtomicU64 = AtomicU64::new(0);
pub(crate) static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_POLL_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_PENDING_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_WAKEUP_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static ASYNC_SLEEP_REGISTER_COUNT: AtomicU64 = AtomicU64::new(0);

pub(crate) static ASYNCGEN_REGISTRY: OnceLock<Mutex<HashSet<PtrSlot>>> = OnceLock::new();
pub(crate) static FN_PTR_CODE: OnceLock<Mutex<HashMap<u64, u64>>> = OnceLock::new();
