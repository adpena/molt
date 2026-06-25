use std::iter::ExactSizeIterator;
use wasm_encoder::{TypeSection, ValType};

pub(crate) const GEN_CONTROL_SIZE: i32 = 48;
pub(crate) const TASK_KIND_FUTURE: i64 = 0;
pub(crate) const TASK_KIND_GENERATOR: i64 = 1;
pub(crate) const TASK_KIND_COROUTINE: i64 = 2;
pub(crate) const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;
pub(crate) const SIMPLE_I64_ARITY5_RET_I64_TYPE: u32 = 12;

/// Poll/async function names that occupy the prefix slots of the indirect
/// function table (right after the sentinel slot at index 0).  Defined once
/// so the wrapper-generation loop, the index-lookup block, and the table
/// element initialisation all stay in sync automatically.
pub(crate) const POLL_TABLE_FUNCS: &[&str] = &[
    "async_sleep_poll",
    "anext_default_poll",
    "asyncgen_poll",
    "promise_poll",
    "io_wait",
    "thread_poll",
    "process_poll",
    "ws_wait",
    "asyncio_wait_for_poll",
    "asyncio_wait_poll",
    "asyncio_gather_poll",
    "asyncio_socket_reader_read_poll",
    "asyncio_socket_reader_readline_poll",
    "asyncio_stream_reader_read_poll",
    "asyncio_stream_reader_readline_poll",
    "asyncio_stream_send_all_poll",
    "asyncio_sock_recv_poll",
    "asyncio_sock_connect_poll",
    "asyncio_sock_accept_poll",
    "asyncio_sock_recv_into_poll",
    "asyncio_sock_sendall_poll",
    "asyncio_sock_recvfrom_poll",
    "asyncio_sock_recvfrom_into_poll",
    "asyncio_sock_sendto_poll",
    "asyncio_timer_handle_poll",
    "asyncio_fd_watcher_poll",
    "asyncio_server_accept_loop_poll",
    "asyncio_ready_runner_poll",
    "contextlib_asyncgen_enter_poll",
    "contextlib_asyncgen_exit_poll",
    "contextlib_async_exitstack_exit_poll",
    "contextlib_async_exitstack_enter_context_poll",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReservedRuntimeCallableSpec {
    pub(crate) index: u32,
    pub(crate) runtime_name: &'static str,
    pub(crate) import_name: &'static str,
    pub(crate) arity: usize,
}

pub(crate) const RESERVED_RUNTIME_CALLABLE_SPECS: &[ReservedRuntimeCallableSpec] = &{
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            [
                $(
                    ReservedRuntimeCallableSpec {
                        index: $idx,
                        runtime_name: stringify!($sym),
                        import_name: $import,
                        arity: $arity,
                    },
                )+
            ]
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ))
};
pub(crate) const RESERVED_RUNTIME_CALLABLE_COUNT: u32 =
    RESERVED_RUNTIME_CALLABLE_SPECS.len() as u32;

// ---------------------------------------------------------------------------
// WASM Exception Handling (WASM_OPTIMIZATION_PLAN.md Section 3.6)
//
// Native WASM exception handling replaces the host-imported exception
// mechanism (exception_push/exception_pending/exception_pop) with the
// standardized WASM exception handling instructions (try_table/throw/catch).
//
// The exception tag carries a single i64 payload: the exception object
// handle.  This matches type index 1 in the static type section:
// (i64) -> ().
//
// Current host-call exception model:
//   try block entry:  call exception_push   (push handler frame)
//   after each call:  call exception_pending (poll for raised exception)
//                     br_if to handler      (branch if pending != 0)
//   try block exit:   call exception_pop    (pop handler frame)
//   raise:            call raise            (set pending + unwind)
//
// Native WASM EH model (target):
//   try block entry:  try_table with catch clause
//   after each call:  (eliminated -- WASM catches automatically)
//   try block exit:   end (implicit)
//   raise:            throw $molt_exception <handle>
//
// Estimated impact: 20-40% speedup for exception-heavy code; 5-10%
// binary size reduction from eliminating exception_pending checks.
//
// Enabled by default; set MOLT_WASM_NATIVE_EH=0 to disable.
// ---------------------------------------------------------------------------

/// Type index for the exception tag payload: (i64) -> ()
/// This is type 1 in the static type section.
pub(crate) const TAG_EXCEPTION_FUNC_TYPE: u32 = 1;

/// Tag index for the molt exception tag (first and only tag in the module).
pub(crate) const TAG_EXCEPTION_INDEX: u32 = 0;

// ---------------------------------------------------------------------------
// Multi-value return type indices (WASM 2.0 multi-value proposal)
//
// These type indices are reserved in the static type section for functions
// that return 2-3 i64 values instead of allocating a tuple on the heap.
// This enables the optimization described in WASM_OPTIMIZATION_PLAN.md §3.1:
// eliminate 1 alloc + N field_get calls per multi-return call site.
//
// Builtins that always return a known-size tuple (e.g. divmod -> 2 values,
// dict items iteration -> 2 values) can be migrated to use these signatures
// once both the host import and call-site lowering are updated.
// ---------------------------------------------------------------------------

/// First dynamic type index; must equal the count of all statically-defined types.
///
/// Static signatures currently occupy indices 0..=50 inclusive (types 41-50 are
/// the fused method-dispatch IC signatures `call_method_icN` /
/// `call_super_method_icN`). Dynamic user arity signatures and wrapper
/// signatures must start after that fixed set.
pub(crate) const STATIC_TYPE_COUNT: u32 = 51;

pub(crate) trait TypeSectionExt {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator;
}

impl TypeSectionExt for TypeSection {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator,
    {
        self.ty().function(params, results);
    }
}

// Constant folding pass is now shared via crate::fold_constants in passes.rs.

pub(crate) fn canonical_static_import_type_idx(name: &str, registry_type_idx: u32) -> u32 {
    match name {
        // The runtime import transaction ABI is the five-carrier importlib
        // primitive `(name, globals, locals, fromlist, level) -> object`. The
        // runtime-callable wrapper generator uses the same arity authority, so
        // the imported function type must match or the fifth local remains on
        // the wrapper stack after `call`.
        "importlib_import_transaction" => SIMPLE_I64_ARITY5_RET_I64_TYPE,
        _ => registry_type_idx,
    }
}
