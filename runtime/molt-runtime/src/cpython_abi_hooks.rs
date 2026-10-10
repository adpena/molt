//! Concrete implementations of the `molt-lang-cpython-abi` `RuntimeHooks` vtable.
//!
//! Each hook acquires the GIL internally via `with_gil` — re-entrant and safe
//! whether called from within Molt's execution frame or from a bare C extension.

mod contextvars;
mod extension_init;
mod gc_control;
mod slice;
mod unicode;
#[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
pub(crate) use extension_init::execute_prepared_extension;
#[cfg(test)]
use extension_init::molt_cpython_abi_pyinit_module_to_bits;
pub use extension_init::molt_cpython_abi_run_static_extension_init;
use extension_init::{hook_alloc_extension_module, hook_initialize_extension};

use std::sync::{Condvar, Mutex, Once};

use std::ffi::CStr;
use std::os::raw::c_int;
use std::ptr;

#[cfg(test)]
use molt_cpython_abi::abi_types::{
    METH_FASTCALL, METH_KEYWORDS, METH_METHOD, METH_NOARGS, Py_ssize_t,
};
use molt_cpython_abi::abi_types::{
    MoltTypeTag, PyModuleDef, PyModuleDef_Type, PyObject, PyTypeObject,
};
use molt_cpython_abi::api::cfunction::CFunctionConvention;
use molt_cpython_abi::api::errors::with_preserved_error;
use molt_cpython_abi::hooks::{AttributeAccess, AttributeMutation, DecodedHandleResult};
use molt_cpython_abi::{
    BorrowedHandleResult, EXCEPTION_SNAPSHOT_ARGS, EXCEPTION_SNAPSHOT_CAUSE,
    EXCEPTION_SNAPSHOT_CONTEXT, EXCEPTION_SNAPSHOT_DICT, EXCEPTION_SNAPSHOT_NOTES,
    EXCEPTION_SNAPSHOT_TRACEBACK, ExceptionSnapshot, MoltBufferView as AbiMoltBufferView,
    OwnedHandleResult, RuntimeHooks,
};
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};

use crate::builtins::containers::{dict_len, dict_next_entry, list_len, tuple_len};
use crate::builtins::numbers::{
    INT_BYTES_INVALID, bigint_from_bytes, bigint_from_f64_trunc, bigint_num_bits,
    bigint_ptr_from_bits, bigint_ref, int_bits_from_bigint, int_bits_from_i64, int_bits_from_i128,
    to_bigint,
};
#[cfg(test)]
use crate::concurrency::GilGuard;
use crate::concurrency::gil::with_gil;
use crate::concurrency::{GilReleaseGuard, RuntimeExecutionGuard, gil_owned_by_current_thread};
use crate::object::builders::{
    alloc_bytes, alloc_dict_with_pairs, alloc_function_obj, alloc_list_filled,
    alloc_list_with_capacity, alloc_module_obj, alloc_string, alloc_tuple_uninitialized,
};
use crate::object::layout::{
    function_set_call_target_ptr, function_set_trampoline_ptr, module_dict_bits,
};
use crate::object::ops::{dict_get_in_place, dict_set_in_place};
use crate::object::type_ids::{
    TYPE_ID_BIGINT, TYPE_ID_BYTES, TYPE_ID_COMPLEX, TYPE_ID_DICT, TYPE_ID_FROZENSET, TYPE_ID_LIST,
    TYPE_ID_LIST_BOOL, TYPE_ID_LIST_INT, TYPE_ID_MODULE, TYPE_ID_SET, TYPE_ID_STRING,
    TYPE_ID_TUPLE,
};
use crate::object::{
    HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE, bytes_data, bytes_len, dec_ref_bits, header_from_obj_ptr,
    inc_ref_bits, object_type_id, string_bytes, string_len,
};
use molt_cpython_abi::api::object::{PY_GIL_STATE_LOCKED, PY_GIL_STATE_UNLOCKED};

// ─── Hook implementations ─────────────────────────────────────────────────

fn abi_buffer_view_from_runtime(view: crate::MoltBufferView) -> AbiMoltBufferView {
    unsafe { std::mem::transmute::<crate::MoltBufferView, AbiMoltBufferView>(view) }
}

#[derive(Clone, Copy)]
struct AbiGilEnsureState {
    depth: usize,
    first_state: c_int,
    encoded_lane: u64,
}

thread_local! {
    static ABI_GIL_ENSURE_STATE: std::cell::Cell<AbiGilEnsureState> = const {
        std::cell::Cell::new(AbiGilEnsureState {
            depth: 0,
            first_state: 0,
            encoded_lane: 0,
        })
    };
    static ABI_GIL_RELEASE_STATE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[inline]
fn owned_result_from_pending(bits: u64) -> OwnedHandleResult {
    with_gil(|py| {
        if crate::exception_pending(&py) {
            // Every caller transfers an owned object result; failure sentinels
            // and inline values have no heap owner. A successful allocation may
            // coexist with a prior error. Retire that result without allowing
            // its finalizers to replace either raised-error channel.
            if MoltObject::from_bits(bits).is_ptr() {
                drop(crate::builtins::exceptions::ExceptionValue::adopt(
                    &py, bits,
                ));
            }
            OwnedHandleResult::error()
        } else {
            OwnedHandleResult::ok(bits)
        }
    })
}

fn gil_ensure_unit() -> c_int {
    let was_held = gil_owned_by_current_thread();
    let gil_state = if was_held {
        PY_GIL_STATE_LOCKED
    } else {
        PY_GIL_STATE_UNLOCKED
    };
    ABI_GIL_ENSURE_STATE.with(|slot| {
        let mut state = slot.get();
        if state.depth == 0 {
            state.first_state = gil_state;
            // The outermost Ensure is an embedding lifetime boundary, not an
            // ordinary call boundary. Its matching final Release must clear
            // runtime worker TLS and any runtime-created CPython thread state;
            // nested Ensure/Release pairs preserve the outer custody exactly.
            state.encoded_lane =
                RuntimeExecutionGuard::enter_with_boundary_created_cleanup().into_encoded_lane();
        } else {
            assert!(
                gil_owned_by_current_thread(),
                "nested PyGILState_Ensure lost outer execution custody"
            );
        }
        state.depth = state
            .depth
            .checked_add(1)
            .expect("PyGILState_Ensure nesting depth overflow");
        slot.set(state);
    });
    gil_state
}

fn gil_leave_unit(release_state: c_int) {
    let encoded_lane = ABI_GIL_ENSURE_STATE.with(|slot| {
        let mut state = slot.get();
        assert!(
            state.depth > 0,
            "PyGILState_Release without matching PyGILState_Ensure"
        );
        let expected = if state.depth == 1 {
            state.first_state
        } else {
            PY_GIL_STATE_LOCKED
        };
        assert_eq!(
            release_state, expected,
            "PyGILState_Release state does not match its Ensure"
        );
        let encoded_lane = (state.depth == 1).then_some(state.encoded_lane);
        state.depth -= 1;
        if state.depth == 0 {
            state.first_state = 0;
            state.encoded_lane = 0;
        }
        slot.set(state);
        encoded_lane
    });
    if let Some(encoded_lane) = encoded_lane {
        // SAFETY: the scalar TLS state consumes the one unmatched outer
        // execution guard created by `gil_ensure_unit` on this thread.
        drop(unsafe { RuntimeExecutionGuard::from_encoded_lane(encoded_lane) });
    }
}

fn gil_save_thread() {
    ABI_GIL_RELEASE_STATE.with(|slot| {
        assert!(
            slot.get().is_none(),
            "nested PyEval_SaveThread is not a valid custody transition"
        );
    });
    assert!(
        gil_owned_by_current_thread(),
        "PyEval_SaveThread requires current-thread GIL custody"
    );
    ABI_GIL_RELEASE_STATE.with(|slot| {
        debug_assert!(slot.get().is_none());
        slot.set(Some(GilReleaseGuard::suspend().into_encoded_state()));
    });
}

fn gil_restore_thread() {
    let encoded = ABI_GIL_RELEASE_STATE.with(|slot| {
        slot.take()
            .expect("PyEval_RestoreThread without matching PyEval_SaveThread")
    });
    // SAFETY: the single-slot state consumes its unmatched same-thread token.
    drop(unsafe { GilReleaseGuard::from_encoded_state(encoded) });
}

unsafe extern "C" fn hook_gil_ensure() -> c_int {
    gil_ensure_unit()
}

unsafe extern "C" fn hook_gil_leave(state: c_int) {
    gil_leave_unit(state);
}

unsafe extern "C" fn hook_gil_release() {
    gil_save_thread();
}

unsafe extern "C" fn hook_gil_restore() {
    gil_restore_thread();
}

unsafe extern "C" fn hook_gil_check() -> c_int {
    c_int::from(gil_owned_by_current_thread())
}

unsafe extern "C" fn hook_runtime_is_initialized() -> c_int {
    c_int::from(crate::state::runtime_state::runtime_is_initialized())
}

unsafe extern "C" fn hook_thread_state_drop_enter() -> u64 {
    crate::concurrency::execution::enter_retained_thread_state_drop()
}

unsafe extern "C" fn hook_thread_state_drop_leave(token: u64) {
    unsafe { crate::concurrency::execution::leave_retained_thread_state_drop(token) };
}

unsafe extern "C" fn hook_attached_runtime_context() -> u32 {
    use molt_cpython_abi::hooks::AttachedRuntimeContextKind;

    let attached = unsafe { hook_attached_execution_context() };
    if attached == AttachedRuntimeContextKind::Detached as u32 {
        return attached;
    }
    // An attached isolate on the process-main thread does not own the
    // process-static C pending-call ring. The lifecycle remains the owner.
    if !crate::state::runtime_state::runtime_state_for_gil()
        .is_some_and(crate::state::runtime_state::owns_process_cpython_state)
    {
        return AttachedRuntimeContextKind::Detached as u32;
    }
    attached
}

unsafe fn hook_attached_execution_context() -> u32 {
    use molt_cpython_abi::hooks::AttachedRuntimeContextKind;

    if !crate::state::runtime_state::runtime_is_initialized() {
        return AttachedRuntimeContextKind::Detached as u32;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if crate::concurrency::execution::current_thread_has_runtime_execution_custody() {
            AttachedRuntimeContextKind::WasmSingleThread as u32
        } else {
            AttachedRuntimeContextKind::Detached as u32
        }
    }
    #[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
    {
        if unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null() {
            AttachedRuntimeContextKind::Detached as u32
        } else {
            AttachedRuntimeContextKind::NativeFreeThreaded as u32
        }
    }
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "free-threaded")))]
    {
        if !unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null() {
            AttachedRuntimeContextKind::NativeGil as u32
        } else {
            AttachedRuntimeContextKind::Detached as u32
        }
    }
}

unsafe extern "C" fn hook_check_signals() -> c_int {
    with_gil(|py| crate::builtins::signal_ext::signal_check_signals(&py))
}

unsafe extern "C" fn hook_set_interrupt(signum: c_int) -> c_int {
    // Async-signal-safe: no GIL, TLS, lock, or allocation on this lane.
    crate::builtins::signal_ext::signal_set_interrupt(i64::from(signum))
}

unsafe extern "C" fn hook_interrupt_occurred() -> c_int {
    with_gil(|py| c_int::from(crate::builtins::signal_ext::signal_interrupt_occurred(&py)))
}

unsafe extern "C" fn hook_notify_pending_calls() {
    crate::builtins::signal_ext::notify_pending_calls();
}

unsafe extern "C" fn hook_pending_call_error(reason: u32) {
    use molt_cpython_abi::hooks::PendingCallErrorKind;

    if with_gil(|_py| crate::exception_pending(&_py)) {
        drop(molt_cpython_abi::api::errors::take_current_error());
        return;
    }
    if transfer_pending_cpython_exception() {
        return;
    }
    let message = PendingCallErrorKind::from_abi(reason)
        .map(|kind| {
            kind.message()
                .to_str()
                .expect("static UTF-8 pending-call error")
        })
        .unwrap_or("invalid pending-call failure reason");
    with_gil(|_py| {
        let _ = crate::builtins::exceptions::raise_exception::<u64>(&_py, "SystemError", message);
    });
}

fn runtime_buffer_view_from_abi(view: AbiMoltBufferView) -> crate::MoltBufferView {
    unsafe { std::mem::transmute::<AbiMoltBufferView, crate::MoltBufferView>(view) }
}

unsafe extern "C" fn hook_alloc_str(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_string(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            // NaN-box the heap pointer so the bridge round-trip via
            // PyObject* -> trailing-bits read recovers a value the runtime's
            // `obj.as_ptr()` recognises as a heap pointer (see
            // `MoltObject::from_ptr` for the canonical encoding).
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_bytes(data: *const u8, len: usize) -> u64 {
    if data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_gil(|_py| {
        let ptr = alloc_bytes(&_py, bytes);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_bytearray(data: *const u8, len: usize) -> u64 {
    with_gil(|py| {
        if len >= isize::MAX as usize {
            crate::builtins::exceptions::record_memory_error_without_allocation(&py);
            return 0;
        }
        let ptr = if data.is_null() {
            crate::object::builders::alloc_bytearray_with_len(&py, len)
        } else {
            crate::object::builders::alloc_bytearray(&py, unsafe {
                std::slice::from_raw_parts(data, len)
            })
        };
        if ptr.is_null() {
            if !crate::exception_pending(&py) {
                crate::builtins::exceptions::record_memory_error_without_allocation(&py);
            }
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_numeric_identity_new(bits: u64) -> OwnedHandleResult {
    with_gil(|py| {
        let value = MoltObject::from_bits(bits);
        let result = if let Some(integer) = value.as_int() {
            crate::builtins::numbers::bigint_bits(&py, BigInt::from(integer))
        } else if let Some(float) = value.as_float() {
            crate::object::ops::alloc_heap_float(&py, float)
        } else {
            return OwnedHandleResult::error();
        };
        owned_result_from_pending(result)
    })
}

unsafe extern "C" fn hook_float_payload(bits: u64, out: *mut f64) -> c_int {
    let Some(ptr) = MoltObject::from_bits(bits).as_ptr() else {
        return -1;
    };
    if out.is_null() || unsafe { object_type_id(ptr) } != crate::TYPE_ID_FLOAT {
        return -1;
    }
    unsafe { out.write(crate::object::ops::heap_float_value(ptr)) };
    0
}

unsafe extern "C" fn hook_int_from_i64(value: i64) -> u64 {
    with_gil(|_py| int_bits_from_i64(&_py, value))
}

unsafe extern "C" fn hook_int_from_u64(value: u64) -> u64 {
    with_gil(|_py| int_bits_from_i128(&_py, value as i128))
}

unsafe extern "C" fn hook_int_from_bytes(
    data: *const u8,
    len: usize,
    little_endian: c_int,
    signed: c_int,
) -> u64 {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if data.is_null() && len != 0 {
        return 0;
    }
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let value = bigint_from_bytes(bytes, little_endian != 0, signed != 0);
    with_gil(|_py| int_bits_from_bigint(&_py, value))
}

unsafe extern "C" fn hook_int_from_digits(
    digits: *const u8,
    len: usize,
    base: u32,
    negative: c_int,
) -> u64 {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if digits.is_null() && len != 0 || !(2..=36).contains(&base) {
        return 0;
    }
    let digits = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(digits, len) }
    };
    let sign = if negative != 0 {
        Sign::Minus
    } else {
        Sign::Plus
    };
    let Some(value) = BigInt::from_radix_be(sign, digits, base) else {
        return 0;
    };
    with_gil(|_py| int_bits_from_bigint(&_py, value))
}

unsafe extern "C" fn hook_int_from_f64_trunc(value: f64) -> u64 {
    if !value.is_finite() {
        return 0;
    }
    with_gil(|_py| int_bits_from_bigint(&_py, bigint_from_f64_trunc(value)))
}

unsafe extern "C" fn hook_int_sign(bits: u64) -> c_int {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(bits);
        if let Some(value) = obj.as_int() {
            return value.signum() as c_int;
        }
        if let Some(value) = obj.as_bool() {
            return value as c_int;
        }
        bigint_ptr_from_bits(bits)
            .map(|ptr| unsafe { bigint_ref(ptr) }.sign())
            .map_or(0, |sign| match sign {
                Sign::Minus => -1,
                Sign::NoSign => 0,
                Sign::Plus => 1,
            })
    })
}

unsafe extern "C" fn hook_int_to_bytes(
    bits: u64,
    data: *mut u8,
    len: usize,
    little_endian: c_int,
    signed: c_int,
) -> c_int {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if len > isize::MAX as usize || data.is_null() && len != 0 {
        return INT_BYTES_INVALID;
    }
    with_gil(|_py| {
        let out = if len == 0 {
            &mut [][..]
        } else {
            unsafe { std::slice::from_raw_parts_mut(data, len) }
        };
        let Some(payload) = crate::builtins::numbers::index_integral_payload_bits(bits) else {
            return INT_BYTES_INVALID;
        };
        crate::builtins::numbers::integral_payload_to_bytes(
            payload,
            out,
            little_endian != 0,
            signed != 0,
        )
    })
}

unsafe extern "C" fn hook_int_num_bits(bits: u64, out: *mut usize) -> c_int {
    #[cfg(all(
        feature = "l7-attestation-probe",
        not(target_arch = "wasm32"),
        not(miri)
    ))]
    crate::attestation_probe::record_numeric_hook();
    if out.is_null() {
        return -1;
    }
    with_gil(|_py| {
        let inline;
        let value = if let Some(ptr) = bigint_ptr_from_bits(bits) {
            unsafe { bigint_ref(ptr) }
        } else {
            let Some(value) = to_bigint(MoltObject::from_bits(bits)) else {
                return -1;
            };
            inline = value;
            &inline
        };
        let Some(num_bits) = bigint_num_bits(value) else {
            return -1;
        };
        unsafe { *out = num_bits };
        0
    })
}

unsafe extern "C" fn hook_int_max_str_digits() -> usize {
    with_gil(|_py| crate::builtins::sys_ext::current_int_max_str_digits(&_py))
}

unsafe extern "C" fn hook_alloc_list() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_list_with_capacity(&_py, &[], 8);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_alloc_list_presized(len: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_list_filled(&_py, len, MoltObject::none());
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[inline]
fn is_list_type_id(type_id: u32) -> bool {
    matches!(type_id, TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
}

unsafe fn list_item_bits(ptr: *mut u8, i: usize) -> Option<u64> {
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_LIST => unsafe { crate::object::seq_access::item(ptr, i) },
        TYPE_ID_LIST_INT => unsafe { crate::object::layout::list_int_vec_ref(ptr) }
            .as_slice()
            .get(i)
            .copied()
            .map(|value| MoltObject::from_int(value).bits()),
        TYPE_ID_LIST_BOOL => unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
            .as_slice()
            .get(i)
            .copied()
            .map(|value| MoltObject::from_bool(value != 0).bits()),
        _ => None,
    }
}

unsafe extern "C" fn hook_list_append(
    list_bits: u64,
    item_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> i32 {
    // Keep representation selection, promotion, allocation accounting, and
    // element refcounting in the runtime's single list-append authority.
    if crate::object::ops_list::molt_list_append_with_projection(list_bits, item_bits, item_ptr) {
        0
    } else {
        -1
    }
}

unsafe extern "C" fn hook_list_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if !is_list_type_id(unsafe { object_type_id(ptr) }) {
        return 0;
    }
    unsafe { list_len(ptr) }
}

unsafe extern "C" fn hook_list_item(bits: u64, i: usize) -> BorrowedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return BorrowedHandleResult::missing(),
    };
    unsafe { list_item_bits(ptr, i) }
        .map(BorrowedHandleResult::ok)
        .unwrap_or_else(BorrowedHandleResult::missing)
}

/// Indexed list store backing `PyList_SetItem`/`PyList_SET_ITEM`. Writes the
/// previous occupant's bits into `*out_old` (so the ABI can release the CPython
/// stolen-ref / `Py_SETREF` old reference) and returns 1 on success, 0 when `i`
/// is out of range or the object is not a list. O(1), allocation-free.
unsafe extern "C" fn hook_list_set(list_bits: u64, i: usize, val_bits: u64) -> OwnedHandleResult {
    let obj = MoltObject::from_bits(list_bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return OwnedHandleResult::error(),
    };
    with_gil(|_py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return OwnedHandleResult::error();
        }
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return OwnedHandleResult::error();
        }
        unsafe {
            crate::object::list_mutation::replace_one_transferring_displaced(&_py, ptr, i, val_bits)
        }
        .map(OwnedHandleResult::ok)
        .unwrap_or_else(OwnedHandleResult::error)
    })
}

/// Insert before (clamped) index `where_` — routes to the runtime `PyList_Insert`
/// (`ins1`) authority so the shift semantics are the single source of truth.
unsafe extern "C" fn hook_list_insert(
    list_bits: u64,
    where_: isize,
    item_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> i32 {
    let Some(ptr) = MoltObject::from_bits(list_bits).as_ptr() else {
        return -1;
    };
    with_gil(|py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return -1;
        }
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&py, ptr) };
        if unsafe {
            crate::object::ops_list::insert_at_native_index_with_projection(
                &py,
                ptr,
                where_ as i64,
                item_bits,
                item_ptr,
            )
        } {
            0
        } else {
            -1
        }
    })
}

/// Sort in place — routes to the runtime `PyList_Sort` (comparison authority).
unsafe extern "C" fn hook_list_sort(list_bits: u64) -> i32 {
    crate::c_api::PyList_Sort(list_bits)
}

/// Reverse in place — routes to the runtime `PyList_Reverse` authority.
unsafe extern "C" fn hook_list_reverse(list_bits: u64) -> i32 {
    crate::c_api::PyList_Reverse(list_bits)
}

/// Replace `list[ilow:ihigh]` with the elements of `itemlist_bits` (a list/tuple,
/// or 0 to delete the slice), growing/shrinking the backing vector via
/// `Vec::splice`. The replacement is cloned before the mutable borrow so a
/// self-slice (`a[i:j] = a`) is safe. Replacement iteration and callbacks have
/// already completed before the caller snapshots the destination storage.
unsafe extern "C" fn hook_list_set_slice(
    list_bits: u64,
    ilow: isize,
    ihigh: isize,
    replacement: *const u64,
    replacement_len: usize,
    future_pointers: *const *mut molt_cpython_abi::abi_types::PyObject,
    future_len: usize,
) -> i32 {
    let ptr = match MoltObject::from_bits(list_bits).as_ptr() {
        Some(p) => p,
        None => return -1,
    };
    with_gil(|_py| {
        if !is_list_type_id(unsafe { object_type_id(ptr) }) {
            return -1;
        }
        let replacement = if replacement_len == 0 {
            &[][..]
        } else if replacement.is_null() {
            let _ =
                crate::raise_exception::<u64>(&_py, "SystemError", "NULL list slice replacement");
            return -1;
        } else {
            unsafe { std::slice::from_raw_parts(replacement, replacement_len) }
        };
        unsafe { crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr) };
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return -1;
        }
        let n = unsafe { list_len(ptr) } as isize;
        let low = ilow.clamp(0, n) as usize;
        let high = ihigh.clamp(low as isize, n) as usize;
        let exact_projection = if future_pointers.is_null() {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts(future_pointers, future_len) })
        };
        if unsafe {
            crate::object::list_mutation::replace_range_with_projection(
                &_py,
                ptr,
                low,
                high,
                replacement,
                exact_projection,
            )
        } {
            0
        } else {
            -1
        }
    })
}

unsafe extern "C" fn hook_alloc_tuple(n: usize) -> u64 {
    with_gil(|_py| {
        let ptr = alloc_tuple_uninitialized(&_py, n);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_tuple_set(
    bits: u64,
    i: usize,
    val_bits: u64,
    exact_pointer: *mut PyObject,
) -> OwnedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return OwnedHandleResult::error(),
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return OwnedHandleResult::error();
    }
    with_gil(|_py| {
        let val_bits = if exact_pointer.is_null() {
            crate::missing_bits(&_py)
        } else {
            val_bits
        };
        match unsafe { crate::object::seq_access::replace_capi_item(&_py, ptr, i, val_bits) } {
            Some(old_bits) if old_bits == crate::missing_bits(&_py) => OwnedHandleResult::missing(),
            Some(old_bits) => OwnedHandleResult::ok(old_bits),
            None => OwnedHandleResult::error(),
        }
    })
}

unsafe extern "C" fn hook_tuple_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return 0;
    }
    unsafe { tuple_len(ptr) }
}

unsafe extern "C" fn hook_tuple_item(bits: u64, i: usize) -> BorrowedHandleResult {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return BorrowedHandleResult::missing(),
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return BorrowedHandleResult::missing();
    }
    with_gil(|py| {
        unsafe { crate::object::seq_access::initialized_tuple_item(&py, ptr, i) }
            .map(BorrowedHandleResult::ok)
            .unwrap_or_else(BorrowedHandleResult::missing)
    })
}

unsafe extern "C" fn hook_mappingproxy_new(mapping: u64) -> OwnedHandleResult {
    with_gil(|py| {
        owned_result_from_pending(crate::builtins::types::mappingproxy_from_mapping(
            &py, mapping,
        ))
    })
}

unsafe extern "C" fn hook_alloc_dict() -> u64 {
    with_gil(|_py| {
        let ptr = alloc_dict_with_pairs(&_py, &[]);
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

unsafe extern "C" fn hook_dict_resolve(bits: u64, merge_source: u8) -> BorrowedHandleResult {
    with_gil(|py| {
        let result = if merge_source == 0 {
            crate::object::ops::dict_backing_bits(&py, bits)
        } else {
            unsafe { crate::object::mapping_merge::direct_dict(&py, bits) }
        };
        match result {
            Ok(Some(bits)) => BorrowedHandleResult::ok(bits),
            Ok(None) => BorrowedHandleResult::missing(),
            Err(()) => BorrowedHandleResult::error(),
        }
    })
}

unsafe extern "C" fn hook_dict_mutate(
    dict_bits: u64,
    key_bits: u64,
    val_bits: u64,
    delete: u8,
    publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
    context: *mut std::ffi::c_void,
) -> i32 {
    with_gil(|py| {
        let Some(pointer) = MoltObject::from_bits(dict_bits).as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(pointer) } != TYPE_ID_DICT {
            return -1;
        }
        let retired = if delete != 0 {
            match unsafe { crate::object::ops::dict_del_deferred(&py, pointer, key_bits) } {
                Some(retired) => retired,
                None => return if crate::exception_pending(&py) { -1 } else { 1 },
            }
        } else {
            match unsafe { crate::object::ops::dict_set_deferred(&py, pointer, key_bits, val_bits) }
            {
                Ok(retired) => retired,
                Err(()) => return -1,
            }
        };
        let status = publish.map_or(0, |publish| unsafe { publish(context) });
        with_preserved_error(|| drop(retired));
        status
    })
}

unsafe extern "C" fn hook_dict_get(
    dict_bits: u64,
    key_bits: u64,
    hash_source: molt_cpython_abi::hooks::DictHashSource,
    hash: i64,
) -> BorrowedHandleResult {
    with_gil(|_py| {
        let obj = MoltObject::from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return BorrowedHandleResult::missing();
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
            return BorrowedHandleResult::missing();
        }
        let value = match hash_source {
            molt_cpython_abi::hooks::DictHashSource::Compute => unsafe {
                dict_get_in_place(&_py, ptr, key_bits)
            },
            molt_cpython_abi::hooks::DictHashSource::Supplied => unsafe {
                crate::object::ops::dict_get_with_hash_in_place(&_py, ptr, key_bits, hash as u64)
            },
        };
        if crate::exception_pending(&_py) {
            BorrowedHandleResult::error()
        } else {
            value.map_or_else(BorrowedHandleResult::missing, BorrowedHandleResult::ok)
        }
    })
}

unsafe extern "C" fn hook_dict_pop(dict_bits: u64, key_bits: u64) -> OwnedHandleResult {
    with_gil(|py| {
        let missing = crate::missing_bits(&py);
        let value = crate::object::ops_dict::molt_dict_pop(
            dict_bits,
            key_bits,
            missing,
            MoltObject::from_int(1).bits(),
        );
        if crate::exception_pending(&py) {
            return OwnedHandleResult::error();
        }
        if value == missing {
            dec_ref_bits(&py, value);
            OwnedHandleResult::missing()
        } else {
            OwnedHandleResult::ok(value)
        }
    })
}

unsafe extern "C" fn hook_dict_len(bits: u64) -> usize {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return 0,
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return 0;
    }
    unsafe { dict_len(ptr) }
}

/// Allocation-free physical dictionary cursor. A complete walk visits each
/// physical row at most once; output publication occurs only for a live row.
unsafe extern "C" fn hook_dict_next(
    dict_bits: u64,
    position: *mut usize,
    out_key: *mut u64,
    out_val: *mut u64,
) -> i32 {
    if position.is_null() {
        return 0;
    }
    with_gil(|_py| unsafe {
        let Some(ptr) = MoltObject::from_bits(dict_bits).as_ptr() else {
            return 0;
        };
        if object_type_id(ptr) != TYPE_ID_DICT {
            return 0;
        }
        let mut cursor = *position;
        let Some(row) = dict_next_entry(ptr, &mut cursor) else {
            return 0;
        };
        *position = cursor;
        if !out_key.is_null() {
            *out_key = row.key;
        }
        if !out_val.is_null() {
            *out_val = row.value;
        }
        1
    })
}

unsafe extern "C" fn hook_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_STRING {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { string_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { string_bytes(ptr) }
        }
    }
}

unsafe extern "C" fn hook_bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let obj = MoltObject::from_bits(bits);
    match obj.as_ptr() {
        None => {
            if !out_len.is_null() {
                unsafe {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
        Some(ptr) => {
            if unsafe { object_type_id(ptr) } != TYPE_ID_BYTES {
                if !out_len.is_null() {
                    unsafe {
                        *out_len = 0;
                    }
                }
                return std::ptr::null();
            }
            let len = unsafe { bytes_len(ptr) };
            if !out_len.is_null() {
                unsafe {
                    *out_len = len;
                }
            }
            unsafe { bytes_data(ptr) }
        }
    }
}

unsafe extern "C" fn hook_bytearray_data(bits: u64, out_len: *mut usize) -> *mut u8 {
    let mut len = 0;
    let data = unsafe { crate::c_api::molt_bytearray_as_ptr(bits, &mut len) };
    if !out_len.is_null() {
        unsafe { *out_len = len as usize }
    };
    data
}

unsafe extern "C" fn hook_bytearray_resize(bits: u64, len: usize) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        if crate::object::buffer_exports::bytearray_resize(py, bits, len) {
            0
        } else {
            -1
        }
    })
}

unsafe extern "C" fn hook_buffer_supports(bits: u64) -> i32 {
    crate::with_gil_entry_nopanic!(py, {
        i32::from(crate::object::buffer_exports::supports_buffer(py, bits))
    })
}

unsafe extern "C" fn hook_buffer_acquire(bits: u64, out_view: *mut AbiMoltBufferView) -> i32 {
    if out_view.is_null() {
        return -1;
    }
    let mut view = crate::MoltBufferView::default();
    let rc = unsafe { crate::c_api::molt_buffer_acquire(bits, &mut view as *mut _) };
    if rc != 0 {
        return rc;
    }
    unsafe {
        *out_view = abi_buffer_view_from_runtime(view);
    }
    0
}

unsafe extern "C" fn hook_buffer_release(view: *mut AbiMoltBufferView) -> i32 {
    if view.is_null() {
        return -1;
    }
    let mut runtime_view = unsafe { runtime_buffer_view_from_abi(*view) };
    let rc = unsafe { crate::c_api::molt_buffer_release(&mut runtime_view as *mut _) };
    unsafe {
        *view = AbiMoltBufferView::default();
    }
    rc
}

unsafe extern "C" fn hook_descriptor_protocol(
    bits: u64,
) -> molt_cpython_abi::hooks::DescriptorProtocol {
    use molt_cpython_abi::hooks::DescriptorProtocol;
    with_gil(|py| unsafe {
        let get = crate::builtins::attr::descriptor_has_get(&py, bits);
        if crate::exception_pending(&py) {
            return DescriptorProtocol::Error;
        }
        let set = crate::builtins::attr::descriptor_is_data(&py, bits);
        if crate::exception_pending(&py) {
            DescriptorProtocol::Error
        } else {
            DescriptorProtocol::from_slots(get, set)
        }
    })
}

unsafe extern "C" fn hook_descriptor_get(
    descriptor: u64,
    receiver: *const u64,
    owner: *const u64,
) -> OwnedHandleResult {
    with_gil(|py| unsafe {
        let get = crate::builtins::attr::descriptor_has_get(&py, descriptor);
        if crate::exception_pending(&py) {
            return OwnedHandleResult::error();
        }
        if !get {
            return OwnedHandleResult::missing();
        }
        let result = crate::builtins::attr::descriptor_bind(
            &py,
            descriptor,
            owner.as_ref().copied(),
            receiver.as_ref().copied(),
        );
        if crate::exception_pending(&py) {
            if let Some(bits) = result {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(&py, bits));
            }
            OwnedHandleResult::error()
        } else {
            result.map_or_else(OwnedHandleResult::missing, OwnedHandleResult::ok)
        }
    })
}

unsafe extern "C" fn hook_descriptor_set(
    descriptor: u64,
    receiver: u64,
    value: *const u64,
) -> molt_cpython_abi::hooks::DescriptorMutationStatus {
    use crate::builtins::attr::{DescriptorMutation, DescriptorMutationOutcome};
    use molt_cpython_abi::hooks::DescriptorMutationStatus;
    with_gil(|py| unsafe {
        let mutation = value.as_ref().map_or(DescriptorMutation::Delete, |&bits| {
            DescriptorMutation::Set(bits)
        });
        match crate::builtins::attr::descriptor_mutate(&py, descriptor, receiver, mutation) {
            DescriptorMutationOutcome::Applied => DescriptorMutationStatus::Applied,
            DescriptorMutationOutcome::NotDescriptor => DescriptorMutationStatus::Missing,
            DescriptorMutationOutcome::Error => DescriptorMutationStatus::Error,
        }
    })
}

unsafe extern "C" fn hook_object_get_attr(
    obj_bits: u64,
    name_bits: u64,
    access: AttributeAccess,
    dictionary: *const u64,
    suppress: bool,
) -> OwnedHandleResult {
    if access == AttributeAccess::Normal {
        return owned_result_from_pending(crate::builtins::attributes::molt_get_attr_name(
            obj_bits, name_bits,
        ));
    }
    if let Some(dictionary) = unsafe { dictionary.as_ref().copied() } {
        return with_gil(|py| unsafe {
            let result = crate::builtins::attr::object_attr_lookup_with_dict(
                &py, obj_bits, name_bits, dictionary, suppress,
            );
            if crate::exception_pending(&py) {
                if let Some(result) = result {
                    crate::dec_ref_bits(&py, result);
                }
                return if suppress && crate::builtins::attr::clear_attribute_error_if_pending(&py) {
                    OwnedHandleResult::missing()
                } else {
                    OwnedHandleResult::error()
                };
            }
            if let Some(result) = result {
                return OwnedHandleResult::ok(result);
            }
            if suppress {
                return OwnedHandleResult::missing();
            }
            let name = crate::string_obj_to_owned(MoltObject::from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_owned());
            crate::builtins::attr::attr_error_with_obj(
                &py,
                crate::type_name(&py, MoltObject::from_bits(obj_bits)),
                &name,
                obj_bits,
            );
            OwnedHandleResult::error()
        });
    }
    let bits = crate::object::ops_builtins::object_getattribute(obj_bits, name_bits, suppress);
    with_gil(|py| {
        if suppress && crate::builtins::attr::clear_attribute_error_if_pending(&py) {
            OwnedHandleResult::missing()
        } else if crate::exception_pending(&py) {
            OwnedHandleResult::error()
        } else {
            OwnedHandleResult::ok(bits)
        }
    })
}

unsafe extern "C" fn hook_object_set_attr(
    obj_bits: u64,
    name_bits: u64,
    value_bits: u64,
    delete: bool,
    access: AttributeMutation,
) -> i32 {
    // Mutation returns Python None, not a C status. Keep deletion separate
    // from the value payload: all-zero bits encode the valid float +0.0.
    let _ = match (access, delete) {
        (AttributeMutation::Normal, false) => {
            crate::builtins::attributes::molt_set_attr_name(obj_bits, name_bits, value_bits)
        }
        (AttributeMutation::Normal, true) => {
            crate::builtins::attributes::molt_del_attr_name(obj_bits, name_bits)
        }
        (AttributeMutation::Generic, false) => {
            crate::builtins::attributes::generic_set_attr_name(obj_bits, name_bits, value_bits)
        }
        (AttributeMutation::Generic, true) => {
            crate::builtins::attributes::generic_del_attr_name(obj_bits, name_bits)
        }
        (AttributeMutation::TypeDefault, _) => crate::builtins::attributes::type_mutate_attr_name(
            obj_bits,
            name_bits,
            if delete { None } else { Some(value_bits) },
        ),
    };
    if with_gil(|py| crate::exception_pending(&py)) {
        -1
    } else {
        0
    }
}

/// `PyObject_Call` authority for bridge-managed Molt callables.
///
/// Routes through the runtime's single call authority (`molt_call_bind`):
/// compiled functions, types, bound methods, kwargs binding, and CPython-shaped
/// exceptions all live there. `args_bits` is a Molt tuple handle of positional
/// arguments (0 = none); `kwargs_bits` is a Molt dict handle (0 = none).
/// Returns a typed owned result, with failures left in the runtime pending
/// exception state for the ABI wrapper to translate.
unsafe extern "C" fn hook_object_call(
    callable_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> OwnedHandleResult {
    if args_bits == 0 {
        return unsafe { hook_object_call_with_pos(callable_bits, &[], kwargs_bits) };
    }
    let obj = MoltObject::from_bits(args_bits);
    let Some(ptr) = obj.as_ptr() else {
        return object_call_type_error("PyObject_Call args must be a tuple");
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TUPLE {
        return object_call_type_error("PyObject_Call args must be a tuple");
    }
    // The immutable tuple keeps every borrowed positional handle alive while
    // the scoped reader transfers it into call-argument custody. The slice
    // cannot escape even though binding may invoke Python.
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |pos| {
            hook_object_call_with_pos(callable_bits, pos, kwargs_bits)
        })
    }
    .unwrap_or_else(|| object_call_type_error("PyObject_Call args must be a tuple"))
}

/// Borrowed flat vector ingress. The C caller keeps operands alive through
/// synchronous dispatch; span arithmetic is checked before constructing slices.
unsafe extern "C" fn hook_object_vectorcall(
    callable_bits: u64,
    values: *const u64,
    positional_count: usize,
    keyword_names: *const u64,
    keyword_count: usize,
) -> OwnedHandleResult {
    let Some(total) = positional_count.checked_add(keyword_count) else {
        return object_call_type_error("PyObject_Vectorcall argument span overflow");
    };
    let max_elements = isize::MAX as usize / std::mem::size_of::<u64>();
    if total > max_elements
        || keyword_count > max_elements
        || (total != 0 && values.is_null())
        || (keyword_count != 0 && keyword_names.is_null())
    {
        return object_call_type_error("PyObject_Vectorcall argument span is invalid");
    }
    let values = if total == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(values, total) }
    };
    let names = if keyword_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(keyword_names, keyword_count) }
    };
    #[cfg(test)]
    cfunction_tests::record_vector_hook_entry();
    with_gil(|py| unsafe {
        let result = crate::call::bind::call_bind_capi_vector(
            &py,
            callable_bits,
            &values[..positional_count],
            names,
            &values[positional_count..],
        );
        if crate::exception_pending(&py) {
            with_preserved_error(|| {
                crate::dec_ref_bits(&py, result);
            });
            OwnedHandleResult::error()
        } else {
            OwnedHandleResult::ok(result)
        }
    })
}

unsafe fn hook_object_call_with_pos(
    callable_bits: u64,
    pos: &[u64],
    kwargs_bits: u64,
) -> OwnedHandleResult {
    with_gil(|py| unsafe {
        if kwargs_bits != 0 {
            let Some(ptr) = MoltObject::from_bits(kwargs_bits).as_ptr() else {
                return object_call_type_error("PyObject_Call kwargs must be a dict");
            };
            if object_type_id(ptr) != TYPE_ID_DICT {
                return object_call_type_error("PyObject_Call kwargs must be a dict");
            }
        }
        let mapping = if kwargs_bits == 0 {
            MoltObject::none().bits()
        } else {
            kwargs_bits
        };
        let result = crate::call::bind::call_bind_capi(&py, callable_bits, None, pos, mapping);
        if crate::exception_pending(&py) {
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                crate::dec_ref_bits(&py, result);
            });
            return OwnedHandleResult::error();
        }
        OwnedHandleResult::ok(result)
    })
}

/// Allocate a `TYPE_ID_FOREIGN` wrapper around a genuine C-extension `PyObject*`
/// crossing INTO compiled Python. The bridge caller takes the strong reference
/// custody; this hook only materializes the Molt heap wrapper.
unsafe extern "C" fn hook_foreign_new(c_ptr: usize) -> u64 {
    with_gil(|_py| crate::object::foreign::foreign_new(&_py, c_ptr))
}

unsafe extern "C" fn hook_native_gc_allocate(address: usize) -> c_int {
    with_gil(|_py| {
        if crate::object::gc::native_gc_allocate(&_py, address) {
            0
        } else {
            -1
        }
    })
}

unsafe extern "C" fn hook_native_gc_track(address: usize) -> c_int {
    with_gil(|_| {
        if unsafe { crate::object::gc::native_gc_track(address) } {
            0
        } else {
            -1
        }
    })
}

unsafe extern "C" fn hook_native_gc_untrack(address: usize) {
    with_gil(|_| crate::object::gc::native_gc_untrack(address));
}

unsafe extern "C" fn hook_native_gc_deallocate(address: usize) {
    with_gil(|_py| crate::object::gc::native_gc_deallocate(&_py, address));
}

unsafe extern "C" fn hook_native_gc_is_tracked(address: usize) -> c_int {
    with_gil(|_| c_int::from(crate::object::gc::native_gc_is_tracked(address)))
}

unsafe extern "C" fn hook_native_gc_is_finalized(address: usize) -> c_int {
    with_gil(|_| c_int::from(crate::object::gc::native_gc_is_finalized(address)))
}

unsafe extern "C" fn hook_native_gc_claim_finalizer(address: usize) -> c_int {
    with_gil(|_| crate::object::gc::native_gc_claim_finalizer(address))
}

unsafe extern "C" fn hook_gc_collect() -> isize {
    with_gil(|_py| {
        let outcome = unsafe { crate::object::gc::collect_cycles(&_py) };
        match outcome.status {
            crate::object::gc::GcCollectStatus::Completed
            | crate::object::gc::GcCollectStatus::ReentrantNoop => {
                isize::try_from(outcome.collected).unwrap_or_else(|_| std::process::abort())
            }
            crate::object::gc::GcCollectStatus::ResourceError(message) => {
                let _ = crate::raise_exception::<i64>(&_py, "MemoryError", message);
                -1
            }
            crate::object::gc::GcCollectStatus::CallbackError(message) => {
                if !crate::exception_pending(&_py)
                    && unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
                {
                    let _ = crate::raise_exception::<i64>(&_py, "RuntimeError", message);
                }
                -1
            }
            crate::object::gc::GcCollectStatus::UnsupportedConcurrency => {
                let _ = crate::raise_exception::<i64>(
                    &_py,
                    "RuntimeError",
                    "cyclic GC requires a free-threaded stop-the-world guard",
                );
                -1
            }
        }
    })
}

unsafe extern "C" fn hook_gc_enable() -> c_int {
    with_gil(|_py| {
        let state = &crate::runtime_state(&_py).gc;
        let previous = state.enabled();
        state.set_enabled(true);
        c_int::from(previous)
    })
}

unsafe extern "C" fn hook_gc_disable() -> c_int {
    with_gil(|_py| {
        let state = &crate::runtime_state(&_py).gc;
        let previous = state.enabled();
        state.set_enabled(false);
        c_int::from(previous)
    })
}

unsafe extern "C" fn hook_gc_is_enabled() -> c_int {
    with_gil(|_py| c_int::from(crate::runtime_state(&_py).gc.enabled()))
}

/// Raise a `TypeError` for a malformed `hook_object_call` argument shape and
/// return the hook's error sentinel (0).
fn object_call_type_error(message: &str) -> OwnedHandleResult {
    with_gil(|_py| {
        let _ = crate::raise_exception::<u64>(&_py, "TypeError", message);
    });
    OwnedHandleResult::error()
}

unsafe extern "C" fn hook_object_format(obj_bits: u64, spec_bits: u64) -> OwnedHandleResult {
    let bits = crate::molt_format_builtin(obj_bits, spec_bits);
    owned_result_from_pending(bits)
}

unsafe extern "C" fn hook_object_str(obj_bits: u64) -> OwnedHandleResult {
    owned_result_from_pending(crate::molt_str_from_obj(obj_bits))
}

unsafe extern "C" fn hook_object_repr(obj_bits: u64) -> OwnedHandleResult {
    owned_result_from_pending(crate::molt_repr_from_obj(obj_bits))
}

unsafe extern "C" fn hook_object_contains(container: u64, needle: u64) -> c_int {
    with_gil(|py| {
        let result = crate::builtins::exceptions::ExceptionValue::adopt(
            &py,
            crate::molt_contains(container, needle),
        );
        if crate::exception_pending(&py) {
            return -1;
        }
        let truth = crate::is_truthy(&py, MoltObject::from_bits(result.bits()));
        if crate::exception_pending(&py) {
            -1
        } else {
            c_int::from(truth)
        }
    })
}

unsafe extern "C" fn hook_object_is_true(obj_bits: u64) -> c_int {
    with_gil(|py| {
        let truth = crate::is_truthy(&py, MoltObject::from_bits(obj_bits));
        if crate::exception_pending(&py) {
            -1
        } else {
            c_int::from(truth)
        }
    })
}

unsafe extern "C" fn hook_object_get_iter(obj_bits: u64) -> OwnedHandleResult {
    with_gil(|py| {
        let bits = crate::molt_iter(obj_bits);
        if crate::exception_pending(&py) {
            with_preserved_error(|| dec_ref_bits(&py, bits));
            return OwnedHandleResult::error();
        }
        if MoltObject::from_bits(bits).is_none() {
            crate::raise_not_iterable::<()>(&py, obj_bits);
            return OwnedHandleResult::error();
        }
        OwnedHandleResult::ok(bits)
    })
}

unsafe extern "C" fn hook_object_get_item(obj_bits: u64, key_bits: u64) -> OwnedHandleResult {
    with_gil(|py| {
        let bits = crate::object::ops::molt_index(obj_bits, key_bits);
        if crate::exception_pending(&py) {
            with_preserved_error(|| dec_ref_bits(&py, bits));
            OwnedHandleResult::error()
        } else {
            OwnedHandleResult::ok(bits)
        }
    })
}

unsafe extern "C" fn hook_object_supports_subscript(obj_bits: u64) -> c_int {
    with_gil(|py| {
        c_int::from(crate::object::ops::value_supports_mp_subscript(
            &py, obj_bits,
        ))
    })
}

unsafe extern "C" fn hook_object_set_item(
    obj_bits: u64,
    key_bits: u64,
    value: *const u64,
) -> c_int {
    with_gil(|py| {
        if value.is_null() {
            let _ = crate::object::ops::molt_del_index(obj_bits, key_bits);
        } else {
            let _ = crate::object::ops::molt_store_index(obj_bits, key_bits, unsafe { *value });
        }
        // Statement operations return a borrowed container, never an owned edge.
        if crate::exception_pending(&py) { -1 } else { 0 }
    })
}

unsafe extern "C" fn hook_iter_check(obj_bits: u64) -> c_int {
    with_gil(|py| c_int::from(unsafe { crate::builtins::attr::is_iterator_bits(&py, obj_bits) }))
}

unsafe extern "C" fn hook_iter_next(iter_bits: u64, exhausted: *mut c_int) -> OwnedHandleResult {
    with_gil(|py| {
        // Keep the completion payload: compiled-loop unboxed iteration discards
        // it, whereas PyIter_Send must return the exact StopIteration.value.
        let pair = crate::molt_iter_next(iter_bits);
        if crate::exception_pending(&py) {
            with_preserved_error(|| dec_ref_bits(&py, pair));
            return OwnedHandleResult::error();
        }
        let decoded = MoltObject::from_bits(pair).as_ptr().and_then(|ptr| unsafe {
            (object_type_id(ptr) == TYPE_ID_TUPLE)
                .then(|| crate::object::seq_access::tuple_pair(ptr))
                .flatten()
        });
        let decoded = decoded.and_then(|(value, done)| {
            MoltObject::from_bits(done)
                .as_bool()
                .map(|done| (value, done))
        });
        let Some((value, done)) = decoded else {
            dec_ref_bits(&py, pair);
            crate::raise_exception::<()>(&py, "SystemError", "invalid runtime iterator result");
            return OwnedHandleResult::error();
        };
        inc_ref_bits(&py, value);
        dec_ref_bits(&py, pair);
        unsafe { *exhausted = c_int::from(done) };
        OwnedHandleResult::ok(value)
    })
}

unsafe extern "C" fn hook_sequence_item(obj_bits: u64, index: isize) -> OwnedHandleResult {
    with_gil(|py| {
        owned_result_from_pending(crate::object::sequence_index::sequence_item_at_index(
            &py,
            obj_bits,
            index as i64,
        ))
    })
}

unsafe extern "C" fn hook_sequence_check(obj_bits: u64) -> c_int {
    with_gil(|py| crate::object::ops::sequence_check_bits(&py, obj_bits))
}

unsafe extern "C" fn hook_object_length_hint(obj_bits: u64, default: isize) -> isize {
    with_gil(|py| {
        let default_bits = int_bits_from_i64(&py, default as i64);
        if crate::exception_pending(&py) {
            dec_ref_bits(&py, default_bits);
            return -1;
        }
        let result = crate::builtins::operator::molt_operator_length_hint(obj_bits, default_bits);
        dec_ref_bits(&py, default_bits);
        let hint = if crate::exception_pending(&py) {
            None
        } else {
            let hint = crate::builtins::numbers::index_i64_integral_bits(result)
                .and_then(|value| isize::try_from(value).ok());
            if hint.is_none() {
                crate::raise_exception::<()>(
                    &py,
                    "OverflowError",
                    "cannot fit length hint into an index-sized integer",
                );
            }
            hint
        };
        dec_ref_bits(&py, result);
        hint.unwrap_or(-1)
    })
}

unsafe extern "C" fn hook_tuple_uses_length_hint() -> bool {
    with_gil(|py| {
        matches!(
            crate::object::iterable::tuple_length_hint_policy(&py),
            crate::object::iterable::LengthHint::Consult
        )
    })
}

unsafe extern "C" fn hook_object_length(obj_bits: u64) -> isize {
    with_gil(|py| {
        let result = crate::molt_len(obj_bits);
        let length = if crate::exception_pending(&py) {
            None
        } else {
            crate::object::ops_sys::coerce_length_result(&py, result)
        };
        dec_ref_bits(&py, result);
        length.unwrap_or(-1)
    })
}

unsafe extern "C" fn hook_object_richcompare(op: i32, left: u64, right: u64) -> OwnedHandleResult {
    use molt_obj_model::sequence_compare::RichCompareOp;
    let Some(op) = RichCompareOp::from_i32(op) else {
        with_gil(|py| {
            crate::raise_exception::<()>(&py, "SystemError", "invalid rich comparison operation");
        });
        return OwnedHandleResult::error();
    };
    let result = match op {
        RichCompareOp::Eq => crate::molt_eq(left, right),
        RichCompareOp::Ne => crate::molt_ne(left, right),
        RichCompareOp::Lt => crate::molt_lt(left, right),
        RichCompareOp::Le => crate::molt_le(left, right),
        RichCompareOp::Gt => crate::molt_gt(left, right),
        RichCompareOp::Ge => crate::molt_ge(left, right),
    };
    owned_result_from_pending(result)
}

unsafe extern "C" fn hook_object_richcompare_builtin(
    declaring_class: u64,
    op: i32,
    left: u64,
    right: u64,
) -> OwnedHandleResult {
    use molt_obj_model::sequence_compare::RichCompareOp;
    with_gil(|py| {
        let Some(op) = RichCompareOp::from_i32(op) else {
            crate::raise_exception::<()>(&py, "SystemError", "invalid rich comparison operation");
            return OwnedHandleResult::error();
        };
        let Some(family) =
            crate::object::ops_compare::builtin_families::family_for_owner(&py, declaring_class)
        else {
            crate::raise_exception::<()>(&py, "SystemError", "invalid declaring comparison class");
            return OwnedHandleResult::error();
        };
        owned_result_from_pending(family.invoke(&py, left, right, op))
    })
}

/// Resolve a borrowed dictionary entry from the interpreter-owned sys module.
/// The canonical module owns its readonly dictionary for this whole lifetime;
/// public imports may replace/delete their sys entry without releasing it.
fn sys_module_attr_borrowed(
    attr: &[u8],
    policy: molt_cpython_abi::hooks::SysLookupPolicy,
) -> BorrowedHandleResult {
    with_gil(|py| {
        let lookup = || unsafe {
            let Some(module) = crate::builtins::modules::ensure_interpreter_sys_module(&py) else {
                return if crate::exception_pending(&py) {
                    BorrowedHandleResult::error()
                } else {
                    BorrowedHandleResult::missing()
                };
            };
            let module_ptr = MoltObject::from_bits(module).as_ptr().unwrap();
            let Some(name) = crate::attr_name_bits_from_bytes(&py, attr) else {
                return BorrowedHandleResult::error();
            };
            let _name_owner =
                crate::PtrDropGuard::new(MoltObject::from_bits(name).as_ptr().unwrap());
            let value =
                crate::object::accessors::instance_attribute_lookup(&py, module_ptr, name, None);

            if crate::exception_pending(&py) {
                if let Some(bits) = value {
                    with_preserved_error(|| dec_ref_bits(&py, bits));
                }
                return BorrowedHandleResult::error();
            }
            value.map_or_else(BorrowedHandleResult::missing, |bits| {
                // This temporary owner came from the shared lookup. The
                // interpreter namespace still owns the selected dictionary edge.
                dec_ref_bits(&py, bits);
                BorrowedHandleResult::ok(bits)
            })
        };
        if policy == molt_cpython_abi::hooks::SysLookupPolicy::PySysGetObject
            && crate::object::ops_sys::runtime_target_at_least(&py, 3, 13)
        {
            let result = crate::builtins::exceptions::run_unraisable(
                &py,
                MoltObject::none().bits(),
                Some("Exception ignored in PySys_GetObject()"),
                lookup,
            );
            // Reporting has consumed the lookup error. PySys_GetObject's ABI
            // boundary restores the detached incoming indicator afterward.
            match result.decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Error => {
                    BorrowedHandleResult::missing()
                }
                _ => result,
            }
        } else {
            lookup()
        }
    })
}

unsafe extern "C" fn hook_sys_get_object_borrowed(
    name_data: *const u8,
    name_len: usize,
    policy: molt_cpython_abi::hooks::SysLookupPolicy,
) -> BorrowedHandleResult {
    if name_data.is_null() {
        return BorrowedHandleResult::missing();
    }
    let name = match std::str::from_utf8(unsafe { std::slice::from_raw_parts(name_data, name_len) })
    {
        Ok(name) => name,
        Err(_) => return BorrowedHandleResult::missing(),
    };
    sys_module_attr_borrowed(name.as_bytes(), policy)
}

unsafe extern "C" fn hook_eval_get_builtins_borrowed() -> BorrowedHandleResult {
    with_gil(|_py| {
        let as_builtins_dict = |bits: u64| -> Option<u64> {
            let ptr = crate::obj_from_bits(bits).as_ptr()?;
            match unsafe { object_type_id(ptr) } {
                TYPE_ID_DICT => Some(bits),
                TYPE_ID_MODULE => {
                    let dict_bits = unsafe { module_dict_bits(ptr) };
                    crate::obj_from_bits(dict_bits)
                        .as_ptr()
                        .is_some_and(|dict_ptr| unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT)
                        .then_some(dict_bits)
                }
                _ => None,
            }
        };
        if let Some(frame_builtins) = crate::builtins::frames::frame_stack_active_builtins() {
            // CPython exposes f_builtins itself, including custom mappings and
            // explicit nonmappings. Protocol validation belongs to item lookup.
            if frame_builtins != 0 {
                return BorrowedHandleResult::ok(frame_builtins);
            }
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "active frame builtins namespace is unavailable",
            );
            return BorrowedHandleResult::error();
        }
        let builtins_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(&_py);
            cache.lock().unwrap().get("builtins").copied()
        };
        if let Some(dict_bits) = builtins_bits.and_then(as_builtins_dict) {
            BorrowedHandleResult::ok(dict_bits)
        } else {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "interpreter builtins dictionary is unavailable",
            );
            BorrowedHandleResult::error()
        }
    })
}

unsafe extern "C" fn hook_method_new(function: u64, receiver: u64) -> OwnedHandleResult {
    with_gil(|py| {
        owned_result_from_pending(crate::builtins::functions::explicit_bound_method_new(
            &py, function, receiver,
        ))
    })
}

unsafe extern "C" fn hook_method_part(
    method: u64,
    part: molt_cpython_abi::hooks::MethodPart,
) -> BorrowedHandleResult {
    let Some(ptr) = MoltObject::from_bits(method).as_ptr() else {
        return BorrowedHandleResult::error();
    };
    if unsafe { object_type_id(ptr) } != crate::TYPE_ID_BOUND_METHOD {
        return BorrowedHandleResult::error();
    }
    BorrowedHandleResult::ok(unsafe {
        match part {
            molt_cpython_abi::hooks::MethodPart::Function => crate::bound_method_func_bits(ptr),
            molt_cpython_abi::hooks::MethodPart::Receiver => crate::bound_method_self_bits(ptr),
        }
    })
}

unsafe extern "C" fn hook_classify_heap(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    let ptr = match obj.as_ptr() {
        Some(p) => p,
        None => return MoltTypeTag::Other as u8,
    };
    match unsafe { object_type_id(ptr) } {
        TYPE_ID_STRING => MoltTypeTag::Str as u8,
        TYPE_ID_BYTES => MoltTypeTag::Bytes as u8,
        crate::TYPE_ID_MEMORYVIEW => MoltTypeTag::MemoryView as u8,
        crate::TYPE_ID_SLICE => MoltTypeTag::Slice as u8,
        TYPE_ID_BIGINT => MoltTypeTag::Int as u8,
        crate::TYPE_ID_FLOAT => MoltTypeTag::Float as u8,
        TYPE_ID_COMPLEX => MoltTypeTag::Complex as u8,
        TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => MoltTypeTag::List as u8,
        TYPE_ID_TUPLE => MoltTypeTag::Tuple as u8,
        TYPE_ID_DICT => MoltTypeTag::Dict as u8,
        TYPE_ID_SET => MoltTypeTag::Set as u8,
        TYPE_ID_FROZENSET => MoltTypeTag::FrozenSet as u8,
        crate::TYPE_ID_TYPE => MoltTypeTag::Type as u8,
        // Callable storage is independent of semantic builtin/Python identity.
        // Both functions and bound methods have vector entry; classes and
        // arbitrary __call__ instances retain their non-vector carriers.
        crate::TYPE_ID_FUNCTION => MoltTypeTag::RuntimeCallable as u8,
        crate::TYPE_ID_BOUND_METHOD => with_gil(|py| {
            let class = unsafe { crate::object_class_bits(ptr) };
            let builtins = crate::builtin_classes(&py);
            if class == builtins.builtin_function_or_method || class == builtins.builtin_method {
                MoltTypeTag::RuntimeCallable as u8
            } else {
                MoltTypeTag::BoundMethod as u8
            }
        }),

        TYPE_ID_MODULE => MoltTypeTag::Module as u8,
        crate::TYPE_ID_EXCEPTION => MoltTypeTag::Exception as u8,
        crate::TYPE_ID_OBJECT
            if with_gil(|_py| {
                (unsafe { crate::object_class_bits(ptr) }) == crate::builtin_classes(&_py).traceback
            }) =>
        {
            MoltTypeTag::Traceback as u8
        }
        _ => MoltTypeTag::Other as u8,
    }
}

unsafe extern "C" fn hook_object_hash(bits: u64) -> i64 {
    with_gil(|_py| {
        let hash = crate::object::ops::hash_bits_signed(&_py, bits);
        if crate::exception_pending(&_py) {
            -1
        } else if hash == -1 {
            -2
        } else {
            hash
        }
    })
}

unsafe extern "C" fn hook_complex_parts(bits: u64, real: *mut f64, imag: *mut f64) -> c_int {
    if real.is_null() || imag.is_null() {
        return -1;
    }
    let Some(ptr) = crate::builtins::numbers::complex_ptr_from_bits(bits) else {
        return -1;
    };
    let value = unsafe { *crate::builtins::numbers::complex_ref(ptr) };
    unsafe {
        *real = value.re;
        *imag = value.im;
    }
    0
}

unsafe extern "C" fn hook_complex_from_doubles(real: f64, imag: f64) -> OwnedHandleResult {
    let bits = with_gil(|_py| crate::builtins::numbers::complex_bits(&_py, real, imag));
    owned_result_from_pending(bits)
}

unsafe extern "C" fn hook_inc_ref(bits: u64) {
    with_gil(|_py| inc_ref_bits(&_py, bits));
}

unsafe extern "C" fn hook_dec_ref(bits: u64) {
    with_gil(|_py| dec_ref_bits(&_py, bits));
}

unsafe extern "C" fn hook_ref_count(bits: u64) -> usize {
    MoltObject::from_bits(bits).as_ptr().map_or(0, |ptr| {
        let header = unsafe { header_from_obj_ptr(ptr) };
        unsafe { (*header).ref_count_snapshot() as usize }
    })
}

unsafe extern "C" fn hook_try_mark_abi_view(bits: u64, present: c_int) -> c_int {
    crate::gil_assert();
    let Some(ptr) = MoltObject::from_bits(bits).as_ptr() else {
        return 1;
    };
    let type_id = unsafe { object_type_id(ptr) };
    if present != 0 && matches!(type_id, TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL) {
        // A published PyListObject requires one stable generic storage
        // authority. Compact int/bool representations cannot retain an ABI
        // view because later promotion would otherwise replace their backing
        // allocation behind C's ob_item pointer.
        with_gil(|_py| unsafe {
            crate::object::ops_list::promote_specialized_list_to_list(&_py, ptr)
        });
        if unsafe { object_type_id(ptr) } != TYPE_ID_LIST {
            return 0;
        }
    }
    let header = unsafe { header_from_obj_ptr(ptr) };
    unsafe {
        if present != 0 {
            if !(*header).try_set_flags_unless(
                crate::object::HEADER_FLAG_HAS_ABI_VIEW,
                crate::object::HEADER_FLAG_DEALLOCATING,
            ) {
                return 0;
            }
        } else {
            (*header).fetch_and_flags(!crate::object::HEADER_FLAG_HAS_ABI_VIEW);
        }
    }
    1
}

// ─── Module / C-extension support ────────────────────────────────────────

unsafe extern "C" fn hook_alloc_module(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        let name_ptr = alloc_string(&_py, bytes);
        if name_ptr.is_null() {
            return 0;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_ptr = alloc_module_obj(&_py, name_bits);
        // alloc_module_obj inc_ref's the name; drop the local reference.
        dec_ref_bits(&_py, name_bits);
        if module_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(module_ptr).bits()
    })
}

unsafe extern "C" fn hook_import_module(name_data: *const u8, name_len: usize) -> u64 {
    if name_data.is_null() || name_len == 0 {
        return 0;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    let name_bits = with_gil(|_py| {
        let name_ptr = alloc_string(&_py, bytes);
        if name_ptr.is_null() {
            return 0;
        }
        MoltObject::from_ptr(name_ptr).bits()
    });
    if MoltObject::from_bits(name_bits).as_ptr().is_none() {
        return 0;
    }
    // molt_module_import owns its own GIL entry and returns an owned module
    // reference; import failures stay in the runtime pending-exception state
    // so the ABI-side module-init diagnostics can drain the real error.
    let module_bits = crate::builtins::modules::molt_module_import(name_bits);
    with_gil(|_py| dec_ref_bits(&_py, name_bits));
    match MoltObject::from_bits(module_bits).as_ptr() {
        Some(_) => module_bits,
        None => 0,
    }
}

unsafe extern "C" fn hook_exception_pending() -> std::os::raw::c_int {
    with_gil(|_py| crate::exception_pending(&_py) as std::os::raw::c_int)
}

unsafe extern "C" fn hook_report_unraisable(
    context_bits: u64,
    type_bits: u64,
    value_bits: u64,
    traceback_bits: u64,
    message: *const u8,
    message_len: usize,
    err_msg: *const u8,
    err_msg_len: usize,
    has_err_msg: c_int,
) {
    let message = if message.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(message, message_len) }
    };
    let err_msg = if has_err_msg != 0 {
        if err_msg.is_null() {
            Some("")
        } else {
            std::str::from_utf8(unsafe { std::slice::from_raw_parts(err_msg, err_msg_len) }).ok()
        }
    } else {
        None
    };
    let fallback = || {
        let text = std::str::from_utf8(message).unwrap_or("<non-UTF-8 C API exception>");
        eprintln!(
            "Exception ignored in C API callback (type=0x{type_bits:x}, context=0x{context_bits:x}): {text}"
        );
    };
    with_gil(|_py| {
        let owned_message_bits = if crate::obj_from_bits(value_bits).is_none() {
            let msg_ptr = crate::alloc_string(&_py, message);
            if msg_ptr.is_null() {
                fallback();
                return;
            }
            Some(MoltObject::from_ptr(msg_ptr).bits())
        } else {
            None
        };
        let payload_bits = owned_message_bits.unwrap_or(value_bits);
        if crate::builtins::exceptions::exception_is_instance(&_py, value_bits) {
            if !crate::obj_from_bits(traceback_bits).is_none() {
                let attached = crate::builtins::exceptions::molt_exception_with_traceback(
                    value_bits,
                    traceback_bits,
                );
                if !crate::obj_from_bits(attached).is_none() {
                    crate::dec_ref_bits(&_py, attached);
                }
            }
            crate::builtins::exceptions::report_captured_unraisable(
                &_py,
                context_bits,
                value_bits,
                err_msg,
            );
            return;
        }
        let args_ptr = crate::alloc_tuple(&_py, &[payload_bits]);
        if args_ptr.is_null() {
            if let Some(bits) = owned_message_bits {
                crate::dec_ref_bits(&_py, bits);
            }
            fallback();
            return;
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = if crate::obj_from_bits(type_bits).as_ptr().is_some() {
            type_bits
        } else {
            crate::exception_type_bits_from_name(&_py, "RuntimeError")
        };
        let exc_ptr = crate::alloc_exception_from_class_bits(&_py, class_bits, args_bits);
        crate::dec_ref_bits(&_py, args_bits);
        if let Some(bits) = owned_message_bits {
            crate::dec_ref_bits(&_py, bits);
        }
        if exc_ptr.is_null() {
            fallback();
            return;
        }
        let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
        if !crate::obj_from_bits(traceback_bits).is_none() {
            let attached = crate::builtins::exceptions::molt_exception_with_traceback(
                exc_bits,
                traceback_bits,
            );
            if !crate::obj_from_bits(attached).is_none() {
                crate::dec_ref_bits(&_py, attached);
            }
        }
        crate::builtins::exceptions::report_captured_unraisable(
            &_py,
            context_bits,
            exc_bits,
            err_msg,
        );
        crate::dec_ref_bits(&_py, exc_bits);
    });
}

// ── Numeric protocol (PyNumber_*) ─────────────────────────────────────────
//
// The single numeric authority is the runtime's `PyNumber_*` compat functions
// (`crate::c_api::PyNumber_*`), which delegate to `molt_add`/`molt_pow`/etc.
// with arbitrary-precision int promotion, float coercion, operator-overload
// dispatch, and CPython-shaped exceptions. Each returns result handle bits or
// `0` with a pending runtime exception on error. These hooks are a thin routing
// layer; they perform NO arithmetic themselves.

fn runtime_exception_field(field: u32) -> Option<crate::builtins::exceptions::ExceptionFieldSlot> {
    use crate::builtins::exceptions::ExceptionFieldSlot;
    use molt_cpython_abi::ExceptionField;
    match field {
        value if value == ExceptionField::Cause as u32 => Some(ExceptionFieldSlot::Cause),
        value if value == ExceptionField::Context as u32 => Some(ExceptionFieldSlot::Context),
        value if value == ExceptionField::Traceback as u32 => Some(ExceptionFieldSlot::Traceback),
        value if value == ExceptionField::Args as u32 => Some(ExceptionFieldSlot::Args),
        _ => None,
    }
}

/// Canonical exception normalization for the C error indicator. The ABI has
/// already shaped `_PyErr_CreateException`'s args tuple; invoke the requested
/// managed class through the same call/bind authority as ordinary Python so
/// custom `__new__`/`__init__` and constructor validation remain authoritative.
unsafe extern "C" fn hook_exception_set_field(
    exception_bits: u64,
    field: u32,
    value_bits: u64,
    has_value: c_int,
) -> c_int {
    with_gil(|_py| {
        let Some(field) = runtime_exception_field(field) else {
            return -1;
        };
        let value_bits = if has_value == 0 {
            MoltObject::none().bits()
        } else {
            value_bits
        };
        crate::builtins::exceptions::exception_replace_field_bits(
            &_py,
            exception_bits,
            field,
            value_bits,
        )
        .map_or(-1, |()| 0)
    })
}

unsafe extern "C" fn hook_exception_get_field(
    exception_bits: u64,
    field: u32,
) -> OwnedHandleResult {
    with_gil(|_py| {
        let Some(field) = runtime_exception_field(field) else {
            return OwnedHandleResult::error();
        };
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return OwnedHandleResult::error();
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return OwnedHandleResult::error();
        }
        let value_bits = match field {
            crate::builtins::exceptions::ExceptionFieldSlot::Cause => unsafe {
                crate::exception_cause_bits(exception_ptr)
            },
            crate::builtins::exceptions::ExceptionFieldSlot::Context => unsafe {
                crate::exception_context_bits(exception_ptr)
            },
            crate::builtins::exceptions::ExceptionFieldSlot::Traceback => {
                crate::exception_materialize_traceback_bits(&_py, exception_ptr)
            }
            crate::builtins::exceptions::ExceptionFieldSlot::Args => {
                let Some(args) = crate::exception_materialized_args_bits(&_py, exception_ptr)
                else {
                    return OwnedHandleResult::error();
                };
                args
            }
            crate::builtins::exceptions::ExceptionFieldSlot::Dict
            | crate::builtins::exceptions::ExceptionFieldSlot::Notes => {
                unreachable!("runtime_exception_field admits only the public C exception fields")
            }
        };
        if crate::exception_pending(&_py) {
            return OwnedHandleResult::error();
        }
        if !matches!(field, crate::builtins::exceptions::ExceptionFieldSlot::Args)
            && crate::obj_from_bits(value_bits).is_none()
        {
            return OwnedHandleResult::missing();
        }
        if crate::obj_from_bits(value_bits).is_none() {
            return OwnedHandleResult::error();
        }
        crate::inc_ref_bits(&_py, value_bits);
        OwnedHandleResult::ok(value_bits)
    })
}

unsafe extern "C" fn hook_runtime_class_borrowed(value_bits: u64) -> BorrowedHandleResult {
    with_gil(|_py| {
        let class_bits = crate::builtins::type_ops::type_of_bits(&_py, value_bits);
        // Class lookup is also used while matching an already-pending error;
        // a pre-existing exception does not invalidate a borrowed class edge.
        if class_bits == 0 {
            BorrowedHandleResult::error()
        } else {
            BorrowedHandleResult::ok(class_bits)
        }
    })
}

/// Return the immutable physical layout discriminator without materializing
/// any Python field or allocating a sidecar. The ABI bridge calls this for
/// both exception instances and their runtime class identities.
unsafe extern "C" fn hook_exception_layout_kind(exception_or_class_bits: u64) -> u8 {
    with_gil(|_py| unsafe {
        crate::builtins::exceptions::exception_layout_kind_from_bits(&_py, exception_or_class_bits)
    })
}

/// Native `BaseExceptionGroup.__new__` admission through the runtime group
/// authority; the C allocator keeps the physical layout and field owners.
unsafe extern "C" fn hook_exception_group_admit(
    request: u32,
    type_name: *const std::os::raw::c_char,
    args_bits: u64,
    message_bits: *mut u64,
    exceptions_bits: *mut u64,
) -> c_int {
    if message_bits.is_null() || exceptions_bits.is_null() {
        return -1;
    }
    let Some(request) = molt_cpython_abi::hooks::ExceptionGroupRequest::from_abi(request) else {
        return -1;
    };
    let name = if type_name.is_null() {
        &[][..]
    } else {
        unsafe { CStr::from_ptr(type_name) }.to_bytes()
    };
    with_gil(|_py| {
        let Some((message, exceptions, narrow)) =
            crate::builtins::exceptions::exception_group_admit_native(
                &_py, request, name, args_bits,
            )
        else {
            return -1;
        };
        unsafe {
            message_bits.write(message);
            exceptions_bits.write(exceptions);
        }
        c_int::from(narrow)
    })
}

unsafe extern "C" fn hook_exception_snapshot(
    exception_bits: u64,
    out: *mut ExceptionSnapshot,
) -> c_int {
    if out.is_null() {
        return -1;
    }
    unsafe { out.write(ExceptionSnapshot::default()) };
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return -1;
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return -1;
        }
        let Some(args) = crate::exception_materialized_args_bits(&_py, exception_ptr) else {
            return -1;
        };
        let traceback = crate::exception_materialize_traceback_bits(&_py, exception_ptr);
        if crate::exception_pending(&_py) || crate::obj_from_bits(args).is_none() {
            return -1;
        }
        let dict = unsafe { crate::exception_dict_bits(exception_ptr) };
        let notes = unsafe { crate::exception_notes_bits(exception_ptr) };
        let context = unsafe { crate::exception_context_bits(exception_ptr) };
        let cause = unsafe { crate::exception_cause_bits(exception_ptr) };
        let suppress =
            crate::obj_from_bits(unsafe { crate::exception_suppress_bits(exception_ptr) })
                .as_bool()
                .unwrap_or(false);
        let typed = unsafe {
            crate::builtins::exceptions::exception_capture_typed_snapshot(&_py, exception_ptr)
        };
        let mut snapshot = ExceptionSnapshot {
            present_mask: EXCEPTION_SNAPSHOT_ARGS,
            typed_present_mask: typed.present_mask,
            layout_kind: typed.layout_kind as u8,
            suppress_context: u32::from(suppress),
            args,
            typed_handles: typed.handles,
            unicode_start: typed.unicode_start,
            unicode_end: typed.unicode_end,
            os_error_written: typed.os_error_written,
            ..ExceptionSnapshot::default()
        };
        if crate::exception_pending(&_py) {
            for bits in snapshot.typed_fields().into_iter().flatten() {
                crate::dec_ref_bits(&_py, bits);
            }
            return -1;
        }
        for (mask, bits, slot) in [
            (EXCEPTION_SNAPSHOT_DICT, dict, &raw mut snapshot.dict),
            (EXCEPTION_SNAPSHOT_NOTES, notes, &raw mut snapshot.notes),
            (
                EXCEPTION_SNAPSHOT_TRACEBACK,
                traceback,
                &raw mut snapshot.traceback,
            ),
            (
                EXCEPTION_SNAPSHOT_CONTEXT,
                context,
                &raw mut snapshot.context,
            ),
            (EXCEPTION_SNAPSHOT_CAUSE, cause, &raw mut snapshot.cause),
        ] {
            let present = if mask == EXCEPTION_SNAPSHOT_NOTES {
                !crate::builtins::exceptions::exception_field_is_missing(bits)
            } else {
                !crate::obj_from_bits(bits).is_none()
            };
            if present {
                snapshot.present_mask |= mask;
                unsafe { *slot = bits };
            }
        }
        for bits in snapshot.base_fields().into_iter().flatten() {
            inc_ref_bits(&_py, bits);
        }
        unsafe { out.write(snapshot) };
        0
    })
}

unsafe extern "C" fn hook_exception_commit_snapshot(
    exception_bits: u64,
    snapshot: *const ExceptionSnapshot,
) -> c_int {
    if snapshot.is_null() {
        return -1;
    }
    let snapshot = unsafe { *snapshot };
    let Some(layout_kind) = snapshot.validated_layout(None) else {
        return -1;
    };
    let [dict, args, notes, traceback, context, cause] = snapshot.base_fields();
    // Notes accept every Python object, including None. The remaining common
    // fields normalize runtime None to absence and enforce their field types.
    if [dict, args, traceback, context, cause]
        .into_iter()
        .flatten()
        .any(|bits| crate::obj_from_bits(bits).is_none())
    {
        return -1;
    }
    let [dict, args, traceback, context, cause] = [dict, args, traceback, context, cause]
        .map(|bits| bits.unwrap_or(MoltObject::none().bits()));
    let notes = notes.unwrap_or_else(crate::builtins::exceptions::exception_field_missing_bits);
    with_gil(|_py| {
        let Some(exception_ptr) = crate::obj_from_bits(exception_bits).as_ptr() else {
            return -1;
        };
        if unsafe { crate::object_type_id(exception_ptr) } != crate::TYPE_ID_EXCEPTION {
            return -1;
        }
        if unsafe { crate::builtins::exceptions::exception_layout_kind(exception_ptr) }
            != layout_kind
        {
            return -1;
        }
        let valid_args = crate::obj_from_bits(args)
            .as_ptr()
            .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) } == TYPE_ID_TUPLE);
        let valid_dict = crate::obj_from_bits(dict).is_none()
            || crate::obj_from_bits(dict)
                .as_ptr()
                .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) } == TYPE_ID_DICT);
        let valid_chain = |bits: u64| {
            crate::obj_from_bits(bits).is_none()
                || crate::builtins::exceptions::exception_is_instance(&_py, bits)
        };
        let valid_traceback = crate::obj_from_bits(traceback).is_none()
            || (crate::builtin_classes(&_py).traceback != 0
                && crate::isinstance_bits(&_py, traceback, crate::builtin_classes(&_py).traceback));
        if !valid_args
            || !valid_dict
            || !valid_chain(context)
            || !valid_chain(cause)
            || !valid_traceback
        {
            return -1;
        }
        let typed = crate::builtins::exceptions::ExceptionTypedSnapshotState {
            layout_kind,
            present_mask: snapshot.typed_present_mask,
            handles: snapshot.typed_handles,
            unicode_start: snapshot.unicode_start,
            unicode_end: snapshot.unicode_end,
            os_error_written: snapshot.os_error_written,
        };
        // Nothing below this point can fail. Pin all new edges, publish every
        // public field as one GIL-serialized transaction, then release the old
        // graph. A rejected snapshot leaves the exception byte-for-byte intact.
        unsafe {
            crate::builtins::exceptions::exception_commit_snapshot_unchecked(
                &_py,
                exception_ptr,
                [dict, args, notes, traceback, context, cause],
                snapshot.suppress_context != 0,
                &typed,
            )
        };
        0
    })
}

unsafe extern "C" fn hook_object_classinfo_match(
    operation: molt_cpython_abi::hooks::ClassInfoOperation,
    value: u64,
    classinfo: u64,
) -> c_int {
    with_gil(|py| {
        let matched = match operation {
            molt_cpython_abi::hooks::ClassInfoOperation::Instance => {
                crate::isinstance_runtime(&py, value, classinfo)
            }
            molt_cpython_abi::hooks::ClassInfoOperation::Subclass => {
                crate::issubclass_runtime(&py, value, classinfo)
            }
        };
        if crate::exception_pending(&py) {
            -1
        } else {
            c_int::from(matched)
        }
    })
}

unsafe extern "C" fn hook_type_is_subtype(subclass_bits: u64, class_bits: u64) -> c_int {
    with_gil(|_py| c_int::from(crate::issubclass_bits(subclass_bits, class_bits)))
}

unsafe extern "C" fn hook_take_pending_exception(
    actual_class_bits: *mut u64,
    traceback_bits: *mut u64,
) -> OwnedHandleResult {
    if actual_class_bits.is_null() || traceback_bits.is_null() {
        return OwnedHandleResult::error();
    }
    unsafe {
        *actual_class_bits = 0;
        *traceback_bits = 0;
    }
    let exception_bits = crate::builtins::exceptions::molt_exception_last_pending();
    let Some(_) = crate::obj_from_bits(exception_bits).as_ptr() else {
        return OwnedHandleResult::error();
    };
    // Detach the original pending edge while lazily materializing traceback.
    // Any failure then leaves its new exact exception pending instead of being
    // mistaken for the original indicator or cleared at the end.
    let _ = crate::builtins::exceptions::molt_exception_clear();
    let captured = with_gil(|_py| {
        let class = crate::builtins::exceptions::exception_class(&_py, exception_bits)?;
        let trace = crate::builtins::exceptions::exception_traceback(&_py, exception_bits)?;
        if crate::exception_pending(&_py) {
            return None;
        }
        Some((class.into_bits(), trace.into_bits()))
    });
    let Some((class_bits, trace_bits)) = captured else {
        with_gil(|_py| crate::dec_ref_bits(&_py, exception_bits));
        return OwnedHandleResult::error();
    };
    unsafe {
        *actual_class_bits = class_bits;
        *traceback_bits = if crate::obj_from_bits(trace_bits).is_none() {
            0
        } else {
            trace_bits
        };
    }
    OwnedHandleResult::ok(exception_bits)
}

unsafe extern "C" fn hook_pending_exception_class() -> molt_cpython_abi::hooks::PendingExceptionClass
{
    with_gil(|py| crate::builtins::exceptions::pending_exception_class(&py))
}

unsafe extern "C" fn hook_clear_pending_exception() {
    let exception_bits = crate::builtins::exceptions::molt_exception_clear();
    with_gil(|_py| {
        if !crate::obj_from_bits(exception_bits).is_none() {
            crate::dec_ref_bits(&_py, exception_bits);
        }
    });
}

unsafe extern "C" fn hook_with_preserved_pending_exception(
    callback: unsafe extern "C" fn(*mut std::ffi::c_void),
    context: *mut std::ffi::c_void,
) {
    with_gil(|py| {
        crate::builtins::exceptions::with_saved_raised_exception(&py, || {
            // The ABI trampoline catches Rust unwind and drains cleanup errors
            // before returning, while the exact raised owner remains detached.
            unsafe { callback(context) };
            true
        });
    });
}

unsafe extern "C" fn hook_handled_exception_get() -> OwnedHandleResult {
    with_gil(|_py| {
        let Some(bits) = crate::builtins::exceptions::exception_context_active_bits() else {
            return OwnedHandleResult::missing();
        };
        crate::inc_ref_bits(&_py, bits);
        OwnedHandleResult::ok(bits)
    })
}

unsafe extern "C" fn hook_handled_exception_set(owned_exception_bits: u64) -> c_int {
    with_gil(|_py| {
        if owned_exception_bits == 0 {
            crate::builtins::exceptions::exception_context_set_abi(&_py, MoltObject::none().bits());
            return 0;
        }
        // CPython deliberately accepts any PyObject here: this API directly
        // replaces the thread state's handled-value slot without validating
        // that the value is a BaseException instance.
        crate::builtins::exceptions::exception_context_set_abi(&_py, owned_exception_bits);
        crate::dec_ref_bits(&_py, owned_exception_bits);
        0
    })
}

/// Binary numeric op. `op` matches [`molt_cpython_abi::NumberBinaryOp`].
unsafe extern "C" fn hook_number_binary_op(
    op: u32,
    mode: u32,
    a_bits: u64,
    b_bits: u64,
) -> OwnedHandleResult {
    use molt_cpython_abi::{NumberBinaryOp, NumberOperationMode};
    let Some(mode) = NumberOperationMode::from_abi(mode) else {
        return with_gil(|py| {
            crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "PyNumber binary op: unknown mode discriminant",
            );
            OwnedHandleResult::error()
        });
    };
    let bits = match op {
        x if x == NumberBinaryOp::Add as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Add(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_add(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Subtract as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Subtract(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_sub(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Multiply as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Multiply(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_mul(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::TrueDivide as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_TrueDivide(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_div(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::FloorDivide as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_FloorDivide(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_floordiv(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Remainder as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Remainder(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_mod(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Lshift as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Lshift(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_lshift(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Rshift as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Rshift(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_rshift(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::And as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_And(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_bit_and(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Or as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Or(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_bit_or(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::Xor as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_Xor(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_bit_xor(a_bits, b_bits)
            }
        },
        x if x == NumberBinaryOp::MatrixMultiply as u32 => match mode {
            NumberOperationMode::Normal => crate::c_api::PyNumber_MatrixMultiply(a_bits, b_bits),
            NumberOperationMode::InPlace => {
                crate::object::ops_arith::molt_inplace_matmul(a_bits, b_bits)
            }
        },
        _ => with_gil(|py| {
            crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "PyNumber binary op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

/// Unary numeric op. `op` matches [`molt_cpython_abi::NumberUnaryOp`].
unsafe extern "C" fn hook_number_unary_op(op: u32, a_bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::NumberUnaryOp;
    let bits = match op {
        x if x == NumberUnaryOp::Negative as u32 => crate::c_api::PyNumber_Negative(a_bits),
        x if x == NumberUnaryOp::Positive as u32 => crate::c_api::PyNumber_Positive(a_bits),
        x if x == NumberUnaryOp::Absolute as u32 => crate::c_api::PyNumber_Absolute(a_bits),
        x if x == NumberUnaryOp::Invert as u32 => crate::c_api::PyNumber_Invert(a_bits),
        x if x == NumberUnaryOp::Float as u32 => crate::molt_float_from_obj(a_bits),
        x if x == NumberUnaryOp::Long as u32 => crate::molt_int_from_obj(
            a_bits,
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        ),
        x if x == NumberUnaryOp::FloatAsDouble as u32 => with_gil(|py| {
            crate::builtins::numbers::float_as_double(&py, a_bits).map_or_else(
                || MoltObject::none().bits(),
                |value| crate::object::ops::float_result_bits(&py, value),
            )
        }),
        x if x == NumberUnaryOp::Index as u32 => {
            with_gil(|py| crate::builtins::numbers::index_from_object(&py, a_bits))
        }
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyNumber unary op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

/// Ternary power `pow(base, exp, modulus)`. Only canonical None means two-arg
/// pow; zero bits are a present float modulus and retain normal type checking.
unsafe extern "C" fn hook_number_power(
    mode: u32,
    a_bits: u64,
    b_bits: u64,
    mod_bits: u64,
) -> OwnedHandleResult {
    let Some(mode) = molt_cpython_abi::NumberOperationMode::from_abi(mode) else {
        return with_gil(|py| {
            crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "PyNumber power: unknown mode discriminant",
            );
            OwnedHandleResult::error()
        });
    };
    let bits = crate::object::ops_arith::number_power(
        a_bits,
        b_bits,
        mod_bits,
        mode == molt_cpython_abi::NumberOperationMode::InPlace,
    );
    owned_result_from_pending(bits)
}

unsafe extern "C" fn hook_target_python_minor() -> i64 {
    with_gil(|py| crate::object::ops_sys::runtime_target_minor(&py))
}

/// Dict copy/keys/values. `op` matches [`molt_cpython_abi::DictOp`]. Routes to
/// the runtime dict authority; returns 0 with a pending exception on error.
unsafe extern "C" fn hook_dict_op(op: u32, dict_bits: u64) -> u64 {
    use molt_cpython_abi::DictOp;
    match op {
        x if x == DictOp::MappingKeys as u32 => crate::c_api::PyMapping_Keys(dict_bits),
        x if x == DictOp::MappingValues as u32 => crate::c_api::PyMapping_Values(dict_bits),
        x if x == DictOp::MappingItems as u32 => crate::c_api::PyMapping_Items(dict_bits),
        x if x == DictOp::Copy as u32 => crate::c_api::PyDict_Copy(dict_bits),
        x if x == DictOp::Keys as u32 => crate::c_api::PyDict_Keys(dict_bits),
        x if x == DictOp::Values as u32 => crate::c_api::PyDict_Values(dict_bits),
        x if x == DictOp::Items as u32 => crate::c_api::PyDict_Items(dict_bits),
        x if x == DictOp::Clear as u32 => {
            let result = crate::molt_dict_clear(dict_bits);
            if with_gil(|_py| crate::exception_pending(&_py)) {
                0
            } else {
                result
            }
        }
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PyDict op: unknown operation discriminant",
            )
        }),
    }
}

unsafe extern "C" fn hook_set_op(op: u32, set_bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::SetOp;
    let bits = match op {
        x if x == SetOp::Pop as u32 => crate::c_api::PySet_Pop(set_bits),
        x if x == SetOp::Clear as u32 => {
            let rc = crate::c_api::PySet_Clear(set_bits);
            if rc == 0 {
                MoltObject::none().bits()
            } else {
                0
            }
        }
        _ => with_gil(|_py| {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "PySet op: unknown operation discriminant",
            )
        }),
    };
    owned_result_from_pending(bits)
}

// ── Set protocol (PySet_*) ────────────────────────────────────────────────
//
// The single set authority is the runtime's `PySet_*` compat functions
// (`crate::c_api::PySet_*`), which delegate to the runtime set object's hash
// table (`molt_set_*` / `set_del_in_place`) with dedup, hashed membership,
// frozenset immutability, and CPython-shaped exceptions (TypeError for
// unhashable keys, SystemError for non-sets). These hooks are a thin routing
// layer; they perform NO set logic themselves.

/// One tagged constructor ingress for both mutable and frozen sets.
unsafe extern "C" fn hook_set_new(iterable: BorrowedHandleResult, frozen: bool) -> u64 {
    let iterable = match iterable.decode() {
        DecodedHandleResult::Ok(bits) => Some(bits),
        DecodedHandleResult::Missing => None,
        DecodedHandleResult::Error => return 0,
    };
    crate::c_api::new_set_from_iterable(iterable, frozen)
}

/// `PySet_Size(anyset)` — element count, or -1 with a pending exception.
unsafe extern "C" fn hook_set_size(set_bits: u64) -> c_int {
    // PySet_Size returns isize; the ABI hook narrows to c_int. A set never holds
    // more than isize::MAX elements, and the only out-of-band value is -1
    // (error), which round-trips through c_int unchanged.
    crate::c_api::PySet_Size(set_bits) as c_int
}

/// `PySet_Contains(anyset, key)` — 1 / 0 / -1.
unsafe extern "C" fn hook_set_contains(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Contains(set_bits, key_bits)
}

/// `PySet_Add(set, key)` — 0 on success, -1 on error.
unsafe extern "C" fn hook_set_add(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Add(set_bits, key_bits)
}

/// `PySet_Discard(set, key)` — 1 (removed) / 0 (absent) / -1 (error).
unsafe extern "C" fn hook_set_discard(set_bits: u64, key_bits: u64) -> c_int {
    crate::c_api::PySet_Discard(set_bits, key_bits)
}

/// dir() owns special lookup, result materialization and sorting for both
/// managed objects and existing foreign wrappers. Errors cross as owned status.
unsafe extern "C" fn hook_object_dir(obj_bits: u64) -> OwnedHandleResult {
    owned_result_from_pending(crate::molt_dir_builtin(obj_bits))
}

unsafe extern "C" fn hook_object_bytes(obj_bits: u64, special: bool) -> OwnedHandleResult {
    with_gil(|py| {
        owned_result_from_pending(crate::object::ops_bytes::bytes_from_object(
            &py, obj_bits, special,
        ))
    })
}

unsafe extern "C" fn hook_memoryview_new(bits: u64) -> OwnedHandleResult {
    owned_result_from_pending(crate::object::ops_memoryview::molt_memoryview_new(bits))
}
unsafe extern "C" fn hook_memoryview_release(bits: u64) -> OwnedHandleResult {
    owned_result_from_pending(crate::object::ops_memoryview::molt_memoryview_release(bits))
}
unsafe extern "C" fn hook_memoryview_from_buffer(
    view: *const AbiMoltBufferView,
    format: *const std::ffi::c_char,
    lease: *const std::ffi::c_void,
    restricted: bool,
) -> OwnedHandleResult {
    with_gil(|py| {
        if view.is_null() {
            crate::raise_exception::<u64>(&py, "SystemError", "null memoryview descriptor");
            return OwnedHandleResult::error();
        }
        let result = unsafe {
            crate::object::memoryview::from_native_descriptor(
                &py,
                &*view,
                format,
                lease
                    .cast::<molt_cpython_abi::api::memory::MemoryViewLease>()
                    .as_ref()
                    .cloned(),
            )
        };
        if !crate::exception_pending(&py)
            && let Some(pointer) = MoltObject::from_bits(result).as_ptr()
        {
            unsafe {
                (*crate::memoryview_ptr(pointer)).restricted = restricted as u8;
            }
        }
        owned_result_from_pending(result)
    })
}
unsafe extern "C" fn hook_memoryview_snapshot(
    bits: u64,
    out: *mut AbiMoltBufferView,
    native_base: *mut *mut molt_cpython_abi::abi_types::PyObject,
    format: *mut *const u8,
    format_len: *mut usize,
) -> i32 {
    with_gil(|py| unsafe {
        let clear_outputs = || {
            if !out.is_null() {
                *out = AbiMoltBufferView::default();
            }
            if !native_base.is_null() {
                *native_base = std::ptr::null_mut();
            }
            if !format.is_null() {
                *format = std::ptr::null();
            }
            if !format_len.is_null() {
                *format_len = 0;
            }
        };
        let fail = |message| {
            clear_outputs();
            if !crate::exception_pending(&py) {
                crate::raise_exception::<u64>(&py, "SystemError", message);
            }
            -1
        };
        if out.is_null() || native_base.is_null() || format.is_null() || format_len.is_null() {
            return fail("null memoryview snapshot output");
        }
        let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
            return fail("memoryview snapshot requires a memoryview");
        };
        if crate::object_type_id(ptr) != crate::TYPE_ID_MEMORYVIEW {
            return fail("memoryview snapshot requires a memoryview");
        }
        if crate::memoryview_released(ptr) {
            clear_outputs();
            return 1;
        }
        let Some(format_ptr) = crate::obj_from_bits(crate::memoryview_format_bits(ptr))
            .as_ptr()
            .filter(|ptr| crate::object_type_id(*ptr) == TYPE_ID_STRING)
        else {
            return fail("memoryview has no valid format");
        };
        if crate::object::ops_memoryview::molt_buffer_export(bits, out.cast()) != 0 {
            return fail("memoryview snapshot export failed");
        }
        *native_base = (*crate::memoryview_ptr(ptr))
            .native_lease
            .as_ref()
            .map_or(std::ptr::null_mut(), |lease| lease.owner());
        *format = crate::string_bytes(format_ptr);
        *format_len = crate::string_len(format_ptr);
        0
    })
}

/// Read the existing declared callable identity without public attribute lookup,
/// executable-name guessing, or a second builtin symbol table. Native metadata
/// is read-only to Python; ordinary functions cannot acquire native kind/owner
/// identity by assigning __name__ or __objclass__.
pub(crate) unsafe fn builtin_slot_owner(
    py: &crate::PyToken<'_>,
    descriptor: u64,
    name: &[u8],
    constructor: bool,
) -> Option<u64> {
    use crate::builtins::functions::native_callable::NativeCallableKind;
    use crate::object::function_metadata::FunctionMetadataField;
    let name_len = name.len();
    let name = name.as_ptr();
    unsafe {
        let function = crate::obj_from_bits(descriptor)
            .as_ptr()
            .filter(|p| object_type_id(*p) == crate::TYPE_ID_FUNCTION)?;
        let expected = if constructor {
            NativeCallableKind::Function
        } else {
            NativeCallableKind::WrapperDescriptor
        };
        if NativeCallableKind::from_class(py, crate::object_class_bits(function)) != Some(expected)
        {
            return None;
        }
        let name_bits = FunctionMetadataField::Name.load(function)?;
        let declared_name = crate::obj_from_bits(name_bits)
            .as_ptr()
            .filter(|p| object_type_id(*p) == crate::TYPE_ID_STRING)?;
        if name.is_null()
            || crate::string_len(declared_name) != name_len
            || std::slice::from_raw_parts(crate::string_bytes(declared_name), name_len)
                != std::slice::from_raw_parts(name, name_len)
        {
            return None;
        }
        let owner_field = if constructor {
            FunctionMetadataField::SelfValue
        } else {
            FunctionMetadataField::Owner
        };
        let owner = owner_field.load(function)?;
        // object construction also shares the existing executable identity
        // boundary used by the runtime constructor argument policy.
        if constructor
            && owner == crate::builtin_classes(py).object
            && !crate::call::type_policy::callable_matches_runtime_symbol(
                Some(descriptor),
                fn_key!(crate::molt_object_new_bound),
            )
        {
            return None;
        }
        if crate::obj_from_bits(owner)
            .as_ptr()
            .is_some_and(|p| object_type_id(p) == crate::TYPE_ID_TYPE)
        {
            Some(owner)
        } else {
            None
        }
    }
}

unsafe extern "C" fn hook_builtin_slot_owner(
    descriptor: u64,
    name: *const u8,
    name_len: usize,
    constructor: bool,
) -> BorrowedHandleResult {
    if name.is_null() {
        return BorrowedHandleResult::missing();
    }
    with_gil(|py| unsafe {
        match builtin_slot_owner(
            &py,
            descriptor,
            std::slice::from_raw_parts(name, name_len),
            constructor,
        ) {
            Some(owner) => BorrowedHandleResult::ok(owner),
            None => BorrowedHandleResult::missing(),
        }
    })
}

/// C type namespaces share the runtime declaration and raw MRO authorities.
unsafe extern "C" fn hook_type_metadata(
    type_bits: u64,
    field: molt_cpython_abi::hooks::TypeMetadataField,
) -> OwnedHandleResult {
    use molt_cpython_abi::hooks::TypeMetadataField;
    with_gil(|py| unsafe {
        let Some(class) = crate::obj_from_bits(type_bits)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == crate::TYPE_ID_TYPE)
        else {
            crate::raise_exception::<u64>(&py, "SystemError", "expected runtime type");
            return OwnedHandleResult::error();
        };
        let value = match field {
            TypeMetadataField::Name => crate::class_name_bits(class),
            TypeMetadataField::QualName => {
                let name = crate::class_qualname_bits(class);
                if name == 0 {
                    crate::class_name_bits(class)
                } else {
                    name
                }
            }
            TypeMetadataField::Bases | TypeMetadataField::Mro => {
                let tuple = if field == TypeMetadataField::Bases {
                    crate::class_bases_bits(class)
                } else {
                    crate::class_mro_bits(class)
                };
                if crate::obj_from_bits(tuple)
                    .as_ptr()
                    .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_TUPLE)
                {
                    crate::raise_exception::<u64>(
                        &py,
                        "SystemError",
                        "runtime type hierarchy is not sealed",
                    );
                    return OwnedHandleResult::error();
                }
                tuple
            }
            TypeMetadataField::NativeProtocolSlots => {
                let Some(protocols) = crate::object::class_storage::class_native_protocols(class)
                else {
                    return OwnedHandleResult::missing();
                };
                crate::MoltObject::from_int(protocols as i64).bits()
            }
            TypeMetadataField::SemanticFlags => {
                let flags = crate::object::class_storage::ClassSemanticPolicy::of(&py, class)
                    .cpython_flags();
                crate::MoltObject::from_int(flags as i64).bits()
            }
            TypeMetadataField::CreationDoc => {
                let doc = crate::object::class_storage::ClassReferenceSlot::CreationDoc.load(class);
                if doc == 0 {
                    crate::raise_exception::<u64>(
                        &py,
                        "SystemError",
                        "runtime type creation documentation is not sealed",
                    );
                    return OwnedHandleResult::error();
                }
                if crate::obj_from_bits(doc).is_none() {
                    return OwnedHandleResult::missing();
                }
                doc
            }
            TypeMetadataField::SolidOwner => {
                match crate::object::class_layout::class_solid_owner(&py, type_bits) {
                    Ok(owner) => owner,
                    Err(()) => return OwnedHandleResult::error(),
                }
            }
            TypeMetadataField::Base => {
                match crate::object::class_layout::class_best_base(&py, class) {
                    Ok(Some(base)) => base.direct,
                    Ok(None) => return OwnedHandleResult::missing(),
                    Err(()) => return OwnedHandleResult::error(),
                }
            }
        };
        inc_ref_bits(&py, value);
        OwnedHandleResult::ok(value)
    })
}

unsafe extern "C" fn hook_type_dict_borrowed(type_bits: u64) -> BorrowedHandleResult {
    with_gil(|py| unsafe {
        let Some(class) = crate::obj_from_bits(type_bits)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == crate::TYPE_ID_TYPE)
        else {
            crate::raise_exception::<u64>(&py, "SystemError", "expected runtime type");
            return BorrowedHandleResult::error();
        };
        if !crate::builtins::methods::publish_builtin_class_methods(&py, type_bits) {
            return BorrowedHandleResult::error();
        }
        let dict = crate::class_dict_bits(class);
        if crate::obj_from_bits(dict)
            .as_ptr()
            .is_none_or(|ptr| object_type_id(ptr) != TYPE_ID_DICT)
        {
            crate::raise_exception::<u64>(&py, "SystemError", "runtime type has no namespace");
            return BorrowedHandleResult::error();
        }
        BorrowedHandleResult::ok(dict)
    })
}

unsafe extern "C" fn hook_type_lookup_borrowed(
    type_bits: u64,
    name_bits: u64,
    search_mro: u8,
) -> BorrowedHandleResult {
    with_gil(|py| unsafe {
        let Some(class) = crate::obj_from_bits(type_bits)
            .as_ptr()
            .filter(|ptr| object_type_id(*ptr) == crate::TYPE_ID_TYPE)
        else {
            crate::raise_exception::<u64>(&py, "SystemError", "expected runtime type");
            return BorrowedHandleResult::error();
        };
        let value = if search_mro != 0 {
            crate::builtins::attr::class_namespace_lookup_mro(&py, class, name_bits)
        } else {
            crate::builtins::attr::class_namespace_lookup_raw(&py, class, name_bits)
        };
        if crate::exception_pending(&py) {
            BorrowedHandleResult::error()
        } else {
            value.map_or_else(BorrowedHandleResult::missing, BorrowedHandleResult::ok)
        }
    })
}

unsafe extern "C" fn hook_module_get_dict_borrowed(module_bits: u64) -> BorrowedHandleResult {
    with_gil(|_py| {
        let module_obj = MoltObject::from_bits(module_bits);
        let Some(module_ptr) = module_obj.as_ptr() else {
            let _ = crate::raise_exception::<u64>(&_py, "SystemError", "expected module object");
            return BorrowedHandleResult::error();
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            let _ = crate::raise_exception::<u64>(&_py, "SystemError", "expected module object");
            return BorrowedHandleResult::error();
        }
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let valid_dict = crate::obj_from_bits(dict_bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_DICT);
        if valid_dict {
            BorrowedHandleResult::ok(dict_bits)
        } else {
            let _ = crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "module object has no dictionary",
            );
            BorrowedHandleResult::error()
        }
    })
}

/// CPython import_add_module: an owned current public module, or a raw new
/// module published before its caller populates it. This operation neither
/// adopts a compiler LoaderNamespace nor copies public values into the private
/// cache. Both PyImport_AddModule and legacy extension replay use this primitive.
fn import_add_module_owned(
    py: &crate::PyToken<'_>,
    name_bits: u64,
    on_admitted: impl FnOnce(u64),
) -> Result<u64, u64> {
    let sys = crate::builtins::modules::interpreter_sys_module(py);

    let Some(sys) = sys else {
        return Err(crate::raise_exception::<u64>(
            py,
            "SystemError",
            "sys.modules is unavailable",
        ));
    };
    let modules = crate::builtins::modules::sys_modules_dict_bits(py, sys);
    if crate::exception_pending(py) {
        if let Some(bits) = modules {
            with_preserved_error(|| crate::dec_ref_bits(py, bits));
        }
        return Err(MoltObject::none().bits());
    }
    let Some(modules) = modules else {
        return Err(crate::raise_exception::<u64>(
            py,
            "SystemError",
            "sys.modules is unavailable",
        ));
    };
    let result = (|| {
        let Some(modules_ptr) = crate::obj_from_bits(modules).as_ptr() else {
            return Err(crate::raise_exception::<u64>(
                py,
                "SystemError",
                "sys.modules is not a dictionary",
            ));
        };
        let existing = unsafe { dict_get_in_place(py, modules_ptr, name_bits) };
        if crate::exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        if let Some(bits) = existing
            && crate::obj_from_bits(bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_MODULE)
        {
            crate::inc_ref_bits(py, bits);
            on_admitted(bits);
            return Ok(bits);
        }
        // Allocate a fresh physical module, independent of a surrounding
        // compiler execution context. Non-module public values are replaced.
        let ptr = alloc_module_obj(py, name_bits);
        if ptr.is_null() {
            return Err(MoltObject::none().bits());
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        match unsafe { crate::object::ops::dict_set_deferred(py, modules_ptr, name_bits, bits) } {
            Ok(retired) => {
                on_admitted(bits);
                // The constructor owner pins the result while displaced
                // values are released outside the dictionary mutation.
                with_preserved_error(|| drop(retired));
                Ok(bits)
            }
            Err(()) => {
                with_preserved_error(|| crate::dec_ref_bits(py, bits));
                Err(MoltObject::none().bits())
            }
        }
    })();
    with_preserved_error(|| crate::dec_ref_bits(py, modules));
    result
}

unsafe extern "C" fn hook_import_add_module_borrowed(
    name_data: *const u8,
    name_len: usize,
) -> BorrowedHandleResult {
    if name_data.is_null() {
        return with_gil(|py| {
            let py = &py;
            let _ = crate::raise_exception::<u64>(
                py,
                "SystemError",
                "PyImport_AddModule requires a module name",
            );
            BorrowedHandleResult::error()
        });
    }
    let name = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|py| {
        let py = &py;
        let name_ptr = alloc_string(py, name);
        if name_ptr.is_null() {
            return BorrowedHandleResult::error();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let result = import_add_module_owned(py, name_bits, |_| {});
        with_preserved_error(|| crate::dec_ref_bits(py, name_bits));
        match result {
            Ok(bits) => {
                // The C API borrows the dictionary's reference.
                with_preserved_error(|| crate::dec_ref_bits(py, bits));
                BorrowedHandleResult::ok(bits)
            }
            Err(_) => BorrowedHandleResult::error(),
        }
    })
}

unsafe extern "C" fn hook_module_set_attr(
    module_bits: u64,
    name_data: *const u8,
    name_len: usize,
    value_bits: u64,
) -> std::os::raw::c_int {
    if name_data.is_null() {
        return -1;
    }
    let module_obj = MoltObject::from_bits(module_bits);
    let Some(module_ptr) = module_obj.as_ptr() else {
        return -1;
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return -1;
    }
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        if crate::exception_pending(&_py) {
            return -1;
        }
        let dict_bits = unsafe { module_dict_bits(module_ptr) };
        let dict_obj = MoltObject::from_bits(dict_bits);
        let Some(dict_ptr) = dict_obj.as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
            return -1;
        }
        let name_str_ptr = alloc_string(&_py, name_bytes);
        if name_str_ptr.is_null() {
            return -1;
        }
        let name_str_bits = MoltObject::from_ptr(name_str_ptr).bits();
        unsafe { dict_set_in_place(&_py, dict_ptr, name_str_bits, value_bits) };
        // dict_set_in_place takes its own references on key+value.  Drop our
        // local key reference; the caller still owns the value.
        dec_ref_bits(&_py, name_str_bits);
        0
    })
}

// ── PyCFunction → Molt callable bridge ───────────────────────────────────
//
// CPython C extensions register functions as PyCFunction pointers with a
// METH_* flag bitmask describing the calling convention.  Molt's call
// dispatch uses fixed-arity native functions (TYPE_ID_FUNCTION) with a
// trampoline slot for variadic dispatch.
//
// The registry owns executable facts only. A traced closure retains its key,
// receiver, defining class and name. Both positional and keyword runtime calls
// use the same CFunctionConvention dispatcher as raw ABI objects.
//
unsafe extern "C" fn hook_module_capi_register(
    module_bits: u64,
    module_def_ptr: usize,
    module_state_size: u64,
    defer_state: bool,
    callbacks: molt_cpython_abi::hooks::ModuleGcCallbacks,
) -> i32 {
    crate::c_api::register_module_capi_with_callbacks(
        module_bits,
        module_def_ptr,
        module_state_size,
        defer_state,
        callbacks,
    )
}

unsafe extern "C" fn hook_module_capi_get_state(module_bits: u64) -> *mut u8 {
    crate::c_api::molt_module_capi_get_state(module_bits)
}
unsafe extern "C" fn hook_module_capi_get_def(module_bits: u64) -> usize {
    crate::c_api::molt_module_capi_get_def(module_bits)
}

unsafe extern "C" fn hook_module_state_add(module_bits: u64, module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_add(module_bits, module_def_ptr)
}

unsafe extern "C" fn hook_module_state_find(module_def_ptr: usize) -> BorrowedHandleResult {
    match crate::c_api::molt_module_state_find(module_def_ptr) {
        0 => BorrowedHandleResult::missing(),
        bits => BorrowedHandleResult::ok(bits),
    }
}

unsafe extern "C" fn hook_module_state_remove(module_def_ptr: usize) -> i32 {
    crate::c_api::molt_module_state_remove(module_def_ptr)
}

unsafe extern "C" fn hook_module_exec_begin(module_bits: u64, def: usize) -> i32 {
    crate::c_api::module_exec_begin(module_bits, def)
}

#[derive(Clone, Copy)]
struct CExtCallable {
    meth_target: *const (),
    flags: i32,
    self_is_null: bool,
    dispatch_kind: CFunctionConvention,
}

/// One immutable, traced runtime closure schema for every C calling convention.
/// The registry owns executable facts only; receiver, defining class and the
/// diagnostic name remain ordinary GC-visible Python edges.
struct CExtCallableContext {
    registry_id: u64,
    receiver: u64,
    defining_class: u64,
    name: u64,
}

impl CExtCallableContext {
    fn fields(&self) -> [u64; 4] {
        [
            self.registry_id,
            self.receiver,
            self.defining_class,
            self.name,
        ]
    }

    unsafe fn from_bits(bits: u64) -> Option<Self> {
        let tuple = MoltObject::from_bits(bits).as_ptr()?;
        unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(tuple, |fields| {
                let [registry_id, receiver, defining_class, name] = fields else {
                    return None;
                };
                Some(Self {
                    registry_id: *registry_id,
                    receiver: *receiver,
                    defining_class: *defining_class,
                    name: *name,
                })
            })
        }
        .flatten()
    }
}

// SAFETY: meth_target is transmuted back to the original
// PyCFunction signature inside the trampoline.  The pointer is guaranteed
// valid for the process lifetime by `loader::LOADED_EXTENSION_LIBRARIES`.
unsafe impl Send for CExtCallable {}
unsafe impl Sync for CExtCallable {}

fn with_cext_callable_registry<R>(read: impl FnOnce(&[CExtCallable]) -> R) -> R {
    // Never return a 'static borrow into a reclaimable runtime. Executable
    // records contain no object edges; the function's traced closure owns self.
    with_gil(|py| {
        let records = crate::runtime_state(&py)
            .cpython
            .callables
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&records)
    })
}

unsafe fn cext_bytes_from_raw<'a>(data: *const u8, len: u64) -> Result<&'a [u8], &'static str> {
    let len = usize::try_from(len).map_err(|_| "byte length does not fit in usize")?;
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() {
        return Err("byte pointer must not be NULL when length is non-zero");
    }
    Ok(unsafe { std::slice::from_raw_parts(data, len) })
}

unsafe fn cext_optional_bytes_from_raw<'a>(
    data: *const u8,
    len: u64,
) -> Result<Option<&'a [u8]>, &'static str> {
    if data.is_null() && len == 0 {
        return Ok(None);
    }
    Ok(Some(unsafe { cext_bytes_from_raw(data, len)? }))
}

unsafe fn cext_set_str_attr(
    obj_bits: u64,
    attr_name: &[u8],
    value_bytes: &[u8],
) -> Result<(), &'static str> {
    let value_bits = unsafe { hook_alloc_str(value_bytes.as_ptr(), value_bytes.len()) };
    if value_bits == 0 {
        return Err("failed to allocate C extension function metadata string");
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            obj_bits,
            attr_name.as_ptr(),
            attr_name.len() as u64,
            value_bits,
        )
    };
    unsafe { hook_dec_ref(value_bits) };
    if rc != 0 {
        return Err("failed to attach C extension function metadata");
    }
    Ok(())
}

unsafe fn cext_create_py_cfunction_bits(
    self_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<u64, &'static str> {
    if name_bytes.is_empty() {
        return Err("PyMethodDef name must not be empty");
    }
    if method_addr == 0 {
        return Err("PyMethodDef method pointer must not be NULL");
    }
    let flags = i32::try_from(method_flags).map_err(|_| "PyMethodDef flags do not fit in c_int")?;
    if CFunctionConvention::from_flags(flags).is_none() {
        return Err("unsupported PyMethodDef flags for CPython ABI bridge");
    }
    let func_bits = unsafe {
        hook_register_c_function(
            method_addr as u64,
            flags,
            self_bits,
            false,
            MoltObject::none().bits(),
            name_bytes.as_ptr(),
            name_bytes.len(),
        )
    };
    if func_bits == 0 {
        return Err("failed to register PyMethodDef callback with CPython ABI bridge");
    }
    if let Some(doc_bytes) = doc_bytes
        && unsafe { cext_set_str_attr(func_bits, b"__doc__", doc_bytes) }.is_err()
    {
        unsafe { hook_dec_ref(func_bits) };
        return Err("failed to attach PyMethodDef __doc__");
    }
    Ok(func_bits)
}

unsafe fn cext_attach_module_name(func_bits: u64, module_bits: u64) -> Result<(), &'static str> {
    let module_name_attr = b"__name__";
    let module_name_bits = unsafe {
        crate::c_api::molt_object_getattr_bytes(
            module_bits,
            module_name_attr.as_ptr(),
            module_name_attr.len() as u64,
        )
    };
    if MoltObject::from_bits(module_name_bits).is_none() {
        let _ = crate::molt_exception_clear();
        return Ok(());
    }
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            func_bits,
            b"__module__".as_ptr(),
            b"__module__".len() as u64,
            module_name_bits,
        )
    };
    unsafe { hook_dec_ref(module_name_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef __module__");
    }
    Ok(())
}

unsafe fn cext_add_py_cfunction_to_module(
    module_bits: u64,
    name_bytes: &[u8],
    method_addr: usize,
    method_flags: u32,
    doc_bytes: Option<&[u8]>,
) -> Result<(), &'static str> {
    let func_bits = unsafe {
        cext_create_py_cfunction_bits(
            module_bits,
            name_bytes,
            method_addr,
            method_flags,
            doc_bytes,
        )?
    };
    if let Err(message) = unsafe { cext_attach_module_name(func_bits, module_bits) } {
        unsafe { hook_dec_ref(func_bits) };
        return Err(message);
    }
    let rc = unsafe {
        hook_module_set_attr(
            module_bits,
            name_bytes.as_ptr(),
            name_bytes.len(),
            func_bits,
        )
    };
    unsafe { hook_dec_ref(func_bits) };
    if rc != 0 {
        return Err("failed to attach PyMethodDef callback to module");
    }
    Ok(())
}

/// # Safety
/// `name_ptr` must point to `name_len` readable bytes; `doc_ptr` must be null or
/// point to `doc_len` readable bytes. `method_addr` must be the address of a
/// valid C callable whose calling convention matches `method_flags`, and
/// `self_bits` must be a valid Molt object handle (or 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_py_cfunction_create_bytes(
    self_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> u64 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if !register_cpython_hooks() {
        return MoltObject::none().bits();
    }
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<u64>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_create_py_cfunction_bits(
                self_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(bits) => bits,
            Err(message) => crate::raise_exception::<u64>(&_py, "TypeError", message),
        }
    })
}

/// # Safety
/// `name_ptr` must point to `name_len` readable bytes; `doc_ptr` must be null or
/// point to `doc_len` readable bytes. `method_addr` must be the address of a
/// valid C callable whose calling convention matches `method_flags`, and
/// `module_bits` must be a valid Molt module handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_module_add_py_cfunction_bytes(
    module_bits: u64,
    name_ptr: *const u8,
    name_len: u64,
    method_addr: usize,
    method_flags: u32,
    doc_ptr: *const u8,
    doc_len: u64,
) -> i32 {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if !register_cpython_hooks() {
        return -1;
    }
    with_gil(|_py| {
        let name_bytes = match unsafe { cext_bytes_from_raw(name_ptr, name_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        let doc_bytes = match unsafe { cext_optional_bytes_from_raw(doc_ptr, doc_len) } {
            Ok(bytes) => bytes,
            Err(message) => return crate::raise_exception::<i32>(&_py, "TypeError", message),
        };
        match unsafe {
            cext_add_py_cfunction_to_module(
                module_bits,
                name_bytes,
                method_addr,
                method_flags,
                doc_bytes,
            )
        } {
            Ok(()) => 0,
            Err(message) => crate::raise_exception::<i32>(&_py, "TypeError", message),
        }
    })
}

/// Publish the native failure before returning to Python. All native call,
/// attribute and descriptor paths share this error boundary. A malformed native
/// failure must produce SystemError rather than appear to be a missing value.
pub(crate) fn propagate_native_failure(py: &crate::PyToken<'_>, operation: &str) {
    if !transfer_pending_cpython_exception() && !crate::exception_pending(py) {
        crate::raise_exception::<()>(
            py,
            "SystemError",
            &format!("{operation} failed without an exception"),
        );
    }
}

pub(crate) fn transfer_pending_cpython_exception() -> bool {
    let Some(error) = molt_cpython_abi::api::errors::take_current_error() else {
        return false;
    };
    let transferred = with_gil(|py| {
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, ExceptionValue, exception_is_instance, exception_replace_field_bits,
        };
        let value = if error.value.is_null() {
            None
        } else {
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(error.value) }
                .map(|bits| ExceptionValue::adopt(&py, bits))
        };
        let Some(value) = value else {
            if !crate::exception_pending(&py) {
                crate::raise_exception::<()>(
                    &py,
                    "SystemError",
                    "C error indicator has no normalized exception instance",
                );
            }
            return false;
        };
        if !exception_is_instance(&py, value.bits()) {
            crate::raise_exception::<()>(
                &py,
                "SystemError",
                "C error indicator has no normalized exception instance",
            );
            return false;
        }
        let trace = if error.traceback.is_null() {
            Some(ExceptionValue::pin(&py, MoltObject::none().bits()))
        } else {
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(error.traceback) }
                .map(|bits| ExceptionValue::adopt(&py, bits))
        };
        let Some(trace) = trace else {
            if !crate::exception_pending(&py) {
                crate::raise_exception::<()>(
                    &py,
                    "SystemError",
                    "C exception traceback projection failed",
                );
            }
            return false;
        };
        if let Err(message) = exception_replace_field_bits(
            &py,
            value.bits(),
            ExceptionFieldSlot::Traceback,
            trace.bits(),
        ) {
            if !crate::exception_pending(&py) {
                crate::raise_exception::<()>(&py, "SystemError", message);
            }
            return false;
        }
        // This is transport of an already-normalized C indicator, not a new
        // Python raise site. Preserve its context and traceback exactly.
        let bits = value.into_bits();
        let mut raised = Some(match crate::current_task_key() {
            Some(task) => crate::builtins::exceptions::RaisedSnapshot::Task(task, bits),
            None => crate::builtins::exceptions::RaisedSnapshot::Thread(bits),
        });
        crate::builtins::exceptions::resolve_raised(&py, &mut raised);
        true
    });
    molt_cpython_abi::api::errors::with_preserved_error(|| drop(error));
    transferred || with_gil(|py| crate::exception_pending(&py))
}

fn cpython_error_is_pending() -> bool {
    !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
        || with_gil(|_py| crate::exception_pending(&_py))
}

struct NativePendingSnapshot {
    c_error: Option<molt_cpython_abi::api::errors::OwnedCError>,
    runtime_error: Option<crate::builtins::exceptions::RaisedSnapshot>,
}

impl NativePendingSnapshot {
    fn has_error(&self) -> bool {
        self.c_error.is_some()
            || !matches!(
                self.runtime_error,
                None | Some(crate::builtins::exceptions::RaisedSnapshot::None)
            )
    }
}

fn take_native_pending_snapshot() -> NativePendingSnapshot {
    let c_error = molt_cpython_abi::api::errors::take_current_error();
    let runtime_error = with_gil(|py| Some(crate::builtins::exceptions::take_raised(&py)));
    NativePendingSnapshot {
        c_error,
        runtime_error,
    }
}

fn restore_native_pending_snapshot(mut snapshot: NativePendingSnapshot) {
    // Discard any destructor/conversion failure produced while the original
    // channels were detached, then restore the exact originals in their
    // respective authorities.
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    if let Some(error) = snapshot.c_error {
        molt_cpython_abi::api::errors::restore_current_error_exact(error);
    }
    with_gil(|py| {
        crate::builtins::exceptions::resolve_raised(&py, &mut snapshot.runtime_error);
    });
}

fn raise_native_result_with_error(message: &str) -> i64 {
    let _ = transfer_pending_cpython_exception();
    with_gil(|_py| crate::raise_exception::<i64>(&_py, "SystemError", message))
}

unsafe fn cext_owned_pyobject_from_bits(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

unsafe fn cext_new_pyobject_from_borrowed_bits(bits: u64) -> *mut PyObject {
    let ptr = unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
    if !ptr.is_null() {
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(ptr) };
    }
    ptr
}

const CEXT_INLINE_INGRESS_CAPACITY: usize = 8;

/// Contiguous borrowed-view custody for one extension call.
///
/// The common NOARGS/O/short-FASTCALL paths stay entirely inline.  Larger
/// calls promote once to a contiguous heap vector because CPython FASTCALL
/// requires one pointer span; ownership and call-argument storage therefore
/// share the same allocation instead of maintaining parallel vectors.
struct CExtIngress {
    inline: [*mut PyObject; CEXT_INLINE_INGRESS_CAPACITY],
    inline_len: usize,
    promoted: Vec<*mut PyObject>,
}

impl CExtIngress {
    fn new() -> Self {
        Self {
            inline: [ptr::null_mut(); CEXT_INLINE_INGRESS_CAPACITY],
            inline_len: 0,
            promoted: Vec::new(),
        }
    }

    fn push_owned_view_checked(&mut self, view: *mut PyObject) -> Option<()> {
        debug_assert!(!view.is_null());
        if self.promoted.is_empty() && self.inline_len < self.inline.len() {
            self.inline[self.inline_len] = view;
            self.inline_len += 1;
            return Some(());
        }
        if self.promoted.is_empty() {
            self.promoted
                .try_reserve_exact(self.inline_len.saturating_add(1))
                .ok()?;
            self.promoted
                .extend_from_slice(&self.inline[..self.inline_len]);
        } else {
            self.promoted.try_reserve(1).ok()?;
        }
        self.promoted.push(view);
        Some(())
    }

    unsafe fn push_borrowed_bits(&mut self, bits: u64) -> Option<*mut PyObject> {
        let view = unsafe { cext_new_pyobject_from_borrowed_bits(bits) };
        if view.is_null() {
            return None;
        }
        if self.push_owned_view_checked(view).is_none() {
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
            return None;
        }
        Some(view)
    }

    fn arguments(&self, prefix_len: usize) -> &[*mut PyObject] {
        let owned = if self.promoted.is_empty() {
            &self.inline[..self.inline_len]
        } else {
            self.promoted.as_slice()
        };
        &owned[prefix_len..]
    }
}

impl Drop for CExtIngress {
    fn drop(&mut self) {
        with_preserved_error(|| {
            let owned = if self.promoted.is_empty() {
                &self.inline[..self.inline_len]
            } else {
                self.promoted.as_slice()
            };
            for &view in owned {
                unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
            }
        });
    }
}

fn cext_ingress_failure(message: &str) -> i64 {
    if cpython_error_is_pending() {
        let _ = transfer_pending_cpython_exception();
        0
    } else {
        with_gil(|_py| crate::raise_exception::<i64>(&_py, "SystemError", message))
    }
}

/// Checked external-ingress trampoline for callers that do not already hold
/// Molt runtime execution custody. Signature matches Molt's
/// `extern "C" fn(closure_bits, args_ptr, args_len) -> i64`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_cext_call_trampoline(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    if crate::concurrency::execution::current_thread_has_c_extension_execution_context() {
        return molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len);
    }
    let _gil_call = RuntimeExecutionGuard::enter();
    molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len)
}

/// Internal trampoline used by generated Molt call dispatch after the outer
/// execution boundary has established lifecycle, thread, and GIL custody.
///
/// Keeping this as a distinct symbol makes external ingress fail-safe without
/// charging every extension call for a redundant execution-boundary guard.
pub(crate) extern "C" fn molt_cpython_abi_cext_call_trampoline_admitted(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    assert!(
        crate::concurrency::execution::current_thread_has_c_extension_execution_context(),
        "admitted C extension trampoline requires active execution and thread-state custody"
    );
    molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len)
}

#[inline(always)]
fn molt_cpython_abi_cext_call_trampoline_inner(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    let Some(args_ptr) = crate::provenance::abi::const_ptr::<u64>(args_ptr) else {
        return with_gil(|py| {
            crate::raise_exception::<i64>(
                &py,
                "SystemError",
                "C extension argument address exceeds the active address space",
            )
        });
    };
    let Some(args) = (unsafe { crate::provenance::abi::slice(args_ptr, args_len) }) else {
        return with_gil(|py| {
            crate::raise_exception::<i64>(
                &py,
                "SystemError",
                "C extension argument range is invalid for the active target",
            )
        });
    };
    call_cext_context(
        closure_bits,
        CExtCallArguments::Vector {
            positional: args,
            names: &[],
            values: &[],
        },
    )
}

const CEXT_TRAMPOLINE_SYMBOL: &str = "crate::molt_cpython_abi_cext_call_trampoline_admitted";

fn cext_trampoline_key() -> u64 {
    // Recognition is a read, including for non-C-extension functions. Only
    // construction registers executable targets; the binder must not lock and
    // mutate that registry just to compare callable identity.
    crate::builtins::functions::runtime_fn_key(
        CEXT_TRAMPOLINE_SYMBOL,
        molt_cpython_abi_cext_call_trampoline_admitted as *const (),
    )
}

/// Recognize a published C wrapper by its canonical executable identity.
/// Callers must provide a live function object; public names are irrelevant.
pub(crate) unsafe fn is_cext_callable(function: *mut u8) -> bool {
    crate::builtins::functions::runtime_callable_represents_symbol(
        unsafe { crate::function_fn_ptr(function) },
        unsafe { crate::function_trampoline_ptr(function) },
        cext_trampoline_key(),
    )
}

/// The caller chooses its real carrier. The owner variant can retain dictionary
/// identity until the resolved C convention decides whether it needs a vector.
pub(crate) enum CExtCallArguments<'a, 'b, 'py> {
    Vector {
        positional: &'a [u64],
        names: &'a [u64],
        values: &'a [u64],
    },
    Owned(&'a mut crate::call::bind::CallArguments<'b, 'py>),
}

/// Bind runtime argument owners to the same C convention authority used by
/// PyObject_Call/vectorcall, without inventing a Python parameter signature.
/// Own exactly one invocation and acquire execution custody only if absent.
pub(crate) unsafe fn try_call_cext(
    py: &crate::PyToken<'_>,
    function: *mut u8,
    arguments: CExtCallArguments<'_, '_, '_>,
) -> Option<u64> {
    if !unsafe { is_cext_callable(function) } {
        return None;
    }
    let Some(_recursion) = crate::state::recursion::RecursionGuard::enter(py) else {
        return Some(MoltObject::none().bits());
    };
    let Some(_frame) = crate::builtins::frames::FrameInvocationGuard::for_function(py, function)
    else {
        return Some(MoltObject::none().bits());
    };
    let _execution =
        (!crate::concurrency::execution::current_thread_has_c_extension_execution_context())
            .then(RuntimeExecutionGuard::enter);
    Some(call_cext_context(
        unsafe { crate::function_execution_closure_bits(function) },
        arguments,
    ) as u64)
}

fn call_cext_context(closure_bits: u64, arguments: CExtCallArguments<'_, '_, '_>) -> i64 {
    let Some(context) = with_gil(|_py| unsafe { CExtCallableContext::from_bits(closure_bits) })
    else {
        return with_gil(|py| {
            crate::raise_exception::<i64>(
                &py,
                "SystemError",
                "C extension trampoline requires a complete callable context",
            )
        });
    };
    let id_obj = MoltObject::from_bits(context.registry_id);
    let id = match id_obj.as_int() {
        Some(value) if value >= 0 => match usize::try_from(value) {
            Ok(value) => value,
            Err(_) => {
                return with_gil(|_py| {
                    crate::raise_exception::<i64>(
                        &_py,
                        "SystemError",
                        "C extension trampoline closure id exceeds the active address space",
                    )
                });
            }
        },
        _ => {
            return with_gil(|_py| {
                crate::raise_exception::<i64>(
                    &_py,
                    "SystemError",
                    "C extension trampoline received non-int closure id",
                )
            });
        }
    };
    let entry = with_cext_callable_registry(|records| records.get(id).copied());
    let Some(entry) = entry else {
        return with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension callable registry id is out of range",
            )
        });
    };

    let (args, keyword_names, keyword_values, mapping) = match arguments {
        CExtCallArguments::Vector {
            positional,
            names,
            values,
        } => (positional, names, values, None),
        CExtCallArguments::Owned(owner) => {
            if entry.dispatch_kind.uses_tuple_arguments()
                && let Some(mapping) = owner.capi_mapping()
            {
                (owner.positional(), &[][..], &[][..], Some(mapping))
            } else {
                let view = match unsafe { owner.unpacked_view() } {
                    Ok(view) => view,
                    Err(error) => return error as i64,
                };
                (view.pos, view.kw_names, view.kw_values, None)
            }
        }
    };
    if keyword_names.len() != keyword_values.len() {
        return with_gil(|py| {
            crate::raise_exception::<i64>(&py, "SystemError", "C extension keyword span mismatch")
        });
    }

    let mut ingress = CExtIngress::new();
    let (self_obj, mut prefix_len) =
        if entry.self_is_null || entry.flags & molt_cpython_abi::abi_types::METH_STATIC != 0 {
            (ptr::null_mut(), 0)
        } else {
            let Some(view) = (unsafe { ingress.push_borrowed_bits(context.receiver) }) else {
                return cext_ingress_failure("failed to materialize C extension self view");
            };
            (view, 1)
        };

    let defining_class = if entry.dispatch_kind == CFunctionConvention::Method {
        let Some(class) = (unsafe { ingress.push_borrowed_bits(context.defining_class) }) else {
            return cext_ingress_failure("failed to materialize C extension defining class");
        };
        prefix_len += 1;
        class.cast::<PyTypeObject>()
    } else {
        ptr::null_mut()
    };
    let arguments = if let Some(mapping) = mapping {
        // Tuple-based dictionary calls preserve the mapping object, including an
        // explicitly empty mapping. Its canonical C view shares that identity.
        let tuple_bits = with_gil(|py| {
            let tuple = crate::object::builders::alloc_tuple(&py, args);
            (!tuple.is_null()).then(|| MoltObject::from_ptr(tuple).bits())
        });
        let Some(tuple_bits) = tuple_bits else {
            return cext_ingress_failure("failed to allocate C extension positional tuple");
        };
        let tuple = unsafe { cext_owned_pyobject_from_bits(tuple_bits) };
        if tuple.is_null() {
            return cext_ingress_failure("failed to materialize C extension positional tuple");
        }
        if ingress.push_owned_view_checked(tuple).is_none() {
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tuple) };
            return cext_ingress_failure("failed to retain C extension positional tuple");
        }
        let kwargs = if MoltObject::from_bits(mapping).is_none() {
            ptr::null_mut()
        } else {
            let Some(dict) = (unsafe { ingress.push_borrowed_bits(mapping) }) else {
                return cext_ingress_failure("failed to materialize C extension keyword mapping");
            };
            dict
        };
        molt_cpython_abi::api::cfunction::CFunctionArguments::Mapping {
            positional: tuple,
            keywords: kwargs,
        }
    } else {
        let kwnames = if keyword_names.is_empty() {
            ptr::null_mut()
        } else {
            let names_bits = with_gil(|py| {
                let tuple = crate::object::builders::alloc_tuple(&py, keyword_names);
                (!tuple.is_null()).then(|| MoltObject::from_ptr(tuple).bits())
            });
            let Some(names_bits) = names_bits else {
                return cext_ingress_failure("failed to allocate C extension keyword names");
            };
            let names = unsafe { cext_owned_pyobject_from_bits(names_bits) };
            if names.is_null() {
                return cext_ingress_failure("failed to materialize C extension keyword names");
            }
            if ingress.push_owned_view_checked(names).is_none() {
                unsafe { molt_cpython_abi::api::refcount::Py_DECREF(names) };
                return cext_ingress_failure("failed to retain C extension keyword names");
            }
            prefix_len += 1;
            names
        };
        for &arg_bits in args.iter().chain(keyword_values) {
            if unsafe { ingress.push_borrowed_bits(arg_bits) }.is_none() {
                return cext_ingress_failure("failed to materialize C extension argument view");
            }
        }
        molt_cpython_abi::api::cfunction::CFunctionArguments::Vector(
            molt_cpython_abi::api::cfunction::VectorcallArguments {
                values: ingress.arguments(prefix_len),
                positional_count: args.len(),
                kwnames,
            },
        )
    };
    // Capture direct callback operands before native code can remove a value
    // from kwargs. CExtIngress owns ABI views; CallbackOperands owns publication
    // through the same authority as physical CFunction calls, on both outcomes.
    let operands = match &arguments {
        molt_cpython_abi::api::cfunction::CFunctionArguments::Mapping {
            positional,
            keywords,
        } => unsafe {
            molt_cpython_abi::api::callback::CallbackOperands::from_tuple_dict(
                [self_obj, defining_class.cast(), *positional, *keywords],
                *positional,
                *keywords,
            )
        },
        molt_cpython_abi::api::cfunction::CFunctionArguments::Vector(vector) => unsafe {
            molt_cpython_abi::api::callback::CallbackOperands::from_vector(
                [
                    self_obj,
                    defining_class.cast(),
                    vector.kwnames,
                    ptr::null_mut(),
                ],
                vector.values,
            )
        },
    };
    let Some(operands) = operands else {
        return cext_ingress_failure("failed to retain native callback operands");
    };
    let result_pyobj = unsafe {
        entry.dispatch_kind.invoke(
            entry.meth_target,
            self_obj,
            defining_class,
            arguments,
            || {
                crate::string_obj_to_owned(MoltObject::from_bits(context.name))
                    .unwrap_or_else(|| "<C extension>".to_owned())
            },
        )
    };

    let result_pyobj = unsafe { operands.complete_result(result_pyobj, "C extension function") };
    drop(operands);

    let returned_null = result_pyobj.is_null();
    let pending = take_native_pending_snapshot();
    let call_left_error = pending.has_error();
    if !returned_null && call_left_error {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result_pyobj) };
    }
    drop(ingress);
    restore_native_pending_snapshot(pending);
    match (returned_null, call_left_error) {
        (true, true) => {
            let _ = transfer_pending_cpython_exception();
            return 0;
        }
        (true, false) => {
            return with_gil(|_py| {
                let msg = format!(
                    "C extension function returned NULL without setting an exception (convention flags 0x{:x})",
                    entry.flags
                );
                crate::raise_exception::<i64>(&_py, "SystemError", &msg)
            });
        }
        (false, true) => {
            let msg = format!(
                "C extension function returned a result with an exception set (convention flags 0x{:x})",
                entry.flags
            );
            return raise_native_result_with_error(&msg);
        }
        (false, false) => {}
    }

    let result_bits =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(result_pyobj) };
    let conversion_pending = take_native_pending_snapshot();
    let conversion_left_error = conversion_pending.has_error();
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result_pyobj) };
    if let Some(bits) = result_bits
        && conversion_left_error
    {
        unsafe { hook_dec_ref(bits) };
    }
    restore_native_pending_snapshot(conversion_pending);
    match (result_bits, conversion_left_error) {
        (Some(bits), false) => bits as i64,
        (Some(_bits), true) => raise_native_result_with_error(
            "C extension result bridge returned a value with an exception set",
        ),
        (None, true) => {
            let _ = transfer_pending_cpython_exception();
            0
        }
        (None, false) => with_gil(|_py| {
            crate::raise_exception::<i64>(
                &_py,
                "SystemError",
                "C extension returned an object that could not enter the bridge",
            )
        }),
    }
}

#[cfg(test)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_cext_call_trampoline_baseline(
    closure_bits: u64,
    args_ptr: u64,
    args_len: u64,
) -> i64 {
    let _gil_call = GilGuard::new();
    molt_cpython_abi_cext_call_trampoline_inner(closure_bits, args_ptr, args_len)
}

unsafe extern "C" fn hook_register_c_function(
    meth_addr: u64,
    flags: std::os::raw::c_int,
    self_bits: u64,
    self_is_null: bool,
    defining_class_bits: u64,
    name_data: *const u8,
    name_len: usize,
) -> u64 {
    if meth_addr == 0 || name_data.is_null() {
        return 0;
    }
    let Some(dispatch_kind) = CFunctionConvention::from_flags(flags) else {
        return 0;
    };
    let Some(meth_target) = crate::provenance::abi::function_ptr(meth_addr) else {
        return 0;
    };
    if meth_target.is_null() {
        return 0;
    }
    let name_bytes = unsafe { std::slice::from_raw_parts(name_data, name_len) };
    with_gil(|_py| {
        if crate::exception_pending(&_py) || !admit_process_cpython_state(&_py) {
            return 0;
        }
        if (dispatch_kind == CFunctionConvention::Method)
            == MoltObject::from_bits(defining_class_bits).is_none()
        {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "C extension defining class does not match its calling convention",
            );
            return 0;
        }
        if dispatch_kind == CFunctionConvention::Method
            && !MoltObject::from_bits(defining_class_bits)
                .as_ptr()
                .is_some_and(|class| unsafe {
                    if object_type_id(class) == crate::TYPE_ID_TYPE {
                        return true;
                    }
                    if object_type_id(class) != crate::TYPE_ID_FOREIGN {
                        return false;
                    }
                    let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .handle_to_borrowed_pyobj(defining_class_bits);
                    molt_cpython_abi::api::typeobj::PyType_Check(view) != 0
                })
        {
            crate::raise_exception::<u64>(
                &_py,
                "SystemError",
                "C extension defining class must be a type",
            );
            return 0;
        }
        let raw_trampoline = molt_cpython_abi_cext_call_trampoline_admitted as *const ();
        let fn_ptr_value =
            crate::builtins::functions::runtime_fn_addr(CEXT_TRAMPOLINE_SYMBOL, raw_trampoline);
        let func_ptr = alloc_function_obj(&_py, fn_ptr_value, dispatch_kind.arity());
        if func_ptr.is_null() {
            if !crate::exception_pending(&_py) {
                crate::raise_exception::<u64>(
                    &_py,
                    "MemoryError",
                    "extension callable allocation failed",
                );
            }
            return 0;
        }
        unsafe {
            #[cfg(not(target_arch = "wasm32"))]
            function_set_call_target_ptr(func_ptr, raw_trampoline);
            function_set_trampoline_ptr(func_ptr, fn_ptr_value);
            if dispatch_kind.is_variadic() {
                (*header_from_obj_ptr(func_ptr))
                    .fetch_or_flags(HEADER_FLAG_FUNC_VARIADIC_TRAMPOLINE);
            }

            let public_name = String::from_utf8_lossy(name_bytes);
            let receiver = (!self_is_null).then_some(self_bits);
            let resolve_owner = || -> Result<Option<u64>, ()> {
                use crate::object::class_layout::{
                    real_class_view, real_type_bits, try_is_real_instance,
                };
                let Some(bits) = receiver else {
                    return Ok(None);
                };
                if try_is_real_instance(&_py, bits, crate::builtin_classes(&_py).module)? {
                    return Ok(None);
                }
                let managed_class = MoltObject::from_bits(bits)
                    .as_ptr()
                    .is_some_and(|pointer| object_type_id(pointer) == crate::TYPE_ID_TYPE);
                if managed_class || real_class_view(bits)?.is_some() {
                    inc_ref_bits(&_py, bits);
                    Ok(Some(bits))
                } else {
                    real_type_bits(&_py, bits).map(Some)
                }
            };
            let owner = match resolve_owner() {
                Ok(owner) => owner,
                Err(()) => {
                    with_preserved_error(|| {
                        dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits())
                    });
                    propagate_native_failure(&_py, "native callable receiver type");
                    return 0;
                }
            };
            let owner_guard = owner.map(|bits| {
                crate::PtrDropGuard::new(
                    MoltObject::from_bits(bits)
                        .as_ptr()
                        .expect("owned callable class"),
                )
            });
            let spec = crate::builtins::functions::native_callable::NativeCallableSpec {
                kind: if dispatch_kind == CFunctionConvention::Method {
                    crate::builtins::functions::native_callable::NativeCallableKind::CMethod
                } else {
                    crate::builtins::functions::native_callable::NativeCallableKind::Function
                },
                owner,
                name: Some(&public_name),
                self_bits: None,
                cache: None,
                text_signature: None,
            };
            let configured = crate::builtins::functions::native_callable::configure_native_callable(
                &_py, func_ptr, spec,
            );
            // Configuration has retained its descriptor edge or copied the
            // qualified name. The C closure below separately owns self_bits.
            with_preserved_error(|| drop(owner_guard));
            if !configured {
                dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits());
                return 0;
            }
            let name_str = alloc_string(&_py, name_bytes);
            if name_str.is_null() {
                dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits());
                return 0;
            }
            let name_bits = MoltObject::from_ptr(name_str).bits();
            // Preallocate every traced edge before claiming an executable
            // record. Filling the reserved integer slot cannot allocate or
            // call Python while holding the registry lock.
            let context = CExtCallableContext {
                registry_id: MoltObject::none().bits(),
                receiver: if self_is_null {
                    MoltObject::none().bits()
                } else {
                    self_bits
                },
                defining_class: defining_class_bits,
                name: name_bits,
            };
            let closure_ptr = crate::object::builders::alloc_tuple(&_py, &context.fields());
            dec_ref_bits(&_py, name_bits);
            if closure_ptr.is_null() {
                dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits());
                if !crate::exception_pending(&_py) {
                    crate::raise_exception::<u64>(
                        &_py,
                        "MemoryError",
                        "extension closure allocation failed",
                    );
                }
                return 0;
            }
            let closure_bits = MoltObject::from_ptr(closure_ptr).bits();
            // Only a fully initialized callable may claim a registry entry.
            // No Python allocation or callback occurs while the registry lock
            // is held; failed construction leaves no orphan callable entry.
            let mut registry = crate::runtime_state(&_py)
                .cpython
                .callables
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if registry.try_reserve(1).is_err() {
                drop(registry);
                dec_ref_bits(&_py, closure_bits);
                dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits());
                crate::raise_exception::<u64>(
                    &_py,
                    "MemoryError",
                    "extension callable registry allocation failed",
                );
                return 0;
            }
            let Some(closure) = u64::try_from(registry.len())
                .ok()
                .and_then(MoltObject::try_from_uint)
            else {
                drop(registry);
                dec_ref_bits(&_py, closure_bits);
                dec_ref_bits(&_py, MoltObject::from_ptr(func_ptr).bits());
                crate::raise_exception::<u64>(
                    &_py,
                    "OverflowError",
                    "extension callable registry exhausted",
                );
                return 0;
            };
            assert_eq!(
                crate::object::seq_access::replace_unique_item_owned(
                    closure_ptr,
                    0,
                    closure.bits()
                ),
                Some(MoltObject::none().bits()),
                "unpublished C extension closure lost exclusive ownership",
            );
            registry.push(CExtCallable {
                meth_target,
                flags,
                self_is_null,
                dispatch_kind,
            });
            drop(registry);
            crate::object::layout::function_set_closure_bits(
                &_py,
                func_ptr,
                closure_bits,
                crate::FunctionCallAbi::OpaqueContextFirst,
            );
            dec_ref_bits(&_py, closure_bits);
        }
        let func_bits = MoltObject::from_ptr(func_ptr).bits();
        if crate::exception_pending(&_py) {
            dec_ref_bits(&_py, func_bits);
            return 0;
        }
        func_bits
    })
}

// ─── Registration ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookRegistrationState {
    Uninitialized,
    Initializing { owner: std::thread::ThreadId },
    Ready,
    Retiring,
    Retired,
    Failed,
}

#[derive(Clone, Copy)]
struct StaticRuntimeBinding {
    pointer: *mut PyObject,
    bits: u64,
}

// SAFETY: the C shell has process lifetime. Runtime handle access/retirement
// requires the GIL and this runtime's lifecycle ownership.
unsafe impl Send for StaticRuntimeBinding {}

/// Runtime-instance CPython state. Only the immutable function-pointer vtable
/// and static C shell addresses are process-owned.
pub(crate) struct CpythonRuntimeState {
    registration: Mutex<HookRegistrationState>,
    ready: Condvar,
    static_bindings: Mutex<Vec<StaticRuntimeBinding>>,
    callables: Mutex<Vec<CExtCallable>>,
}

impl CpythonRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            registration: Mutex::new(HookRegistrationState::Uninitialized),
            ready: Condvar::new(),
            static_bindings: Mutex::new(Vec::new()),
            callables: Mutex::new(Vec::new()),
        }
    }

    unsafe fn bind_static(
        &self,
        py: &crate::PyToken<'_>,
        pointer: *mut PyObject,
        bits: u64,
        canonical_view: bool,
    ) {
        let semantic_flags = unsafe {
            crate::object::class_storage::ClassSemanticPolicy::of(
                py,
                crate::obj_from_bits(bits).as_ptr().expect("bound class"),
            )
            .cpython_flags()
        };
        unsafe {
            use molt_cpython_abi::abi_types::Py_TPFLAGS_HEAPTYPE;
            let ty = pointer.cast::<PyTypeObject>();
            assert_eq!(
                (*ty).tp_flags & Py_TPFLAGS_HEAPTYPE,
                semantic_flags & Py_TPFLAGS_HEAPTYPE,
                "bound C type allocation extent disagrees with runtime origin"
            );
            (*ty).tp_flags = ((*ty).tp_flags & !molt_cpython_abi::hooks::TYPE_SEMANTIC_FLAGS_MASK)
                | semantic_flags;
        }
        let mut bindings = self.static_bindings.lock().unwrap();
        assert!(
            bindings.iter().all(|binding| binding.pointer != pointer),
            "static C shell has duplicate runtime binding ownership"
        );
        bindings
            .try_reserve(1)
            .expect("runtime static binding ownership allocation failed");
        inc_ref_bits(py, bits);
        let bound = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(
                pointer,
                bits,
                canonical_view,
            )
        };
        if let Err(conflict) = bound {
            drop(bindings);
            let name = unsafe { (*pointer.cast::<PyTypeObject>()).tp_name };
            let name = if name.is_null() {
                std::borrow::Cow::Borrowed("<unnamed>")
            } else {
                unsafe { CStr::from_ptr(name) }.to_string_lossy()
            };
            dec_ref_bits(py, bits);
            panic!(
                "static C shell {name} ({pointer:p}) could not bind runtime class {bits:#x}: {conflict:?}"
            );
        }
        bindings.push(StaticRuntimeBinding { pointer, bits });
    }

    /// Detach the runtime-backed fields of this runtime's process-static shells.
    /// Keep the exact bridge bindings and callable dispatch available while
    /// dropping those fields: C deallocators can reenter the old runtime.
    pub(crate) fn retire_static_roots(&self) {
        crate::gil_assert();
        let mut registration = self.registration.lock().unwrap();
        match *registration {
            HookRegistrationState::Uninitialized | HookRegistrationState::Retired => return,
            HookRegistrationState::Retiring => return,
            HookRegistrationState::Ready | HookRegistrationState::Initializing { .. } => {
                *registration = HookRegistrationState::Retiring;
            }
            HookRegistrationState::Failed => panic!("retiring failed CPython bootstrap twice"),
        }
        drop(registration);
        unsafe { molt_cpython_abi::abi_types::retire_builtin_static_type_runtime_state() };
    }

    pub(crate) fn static_class_roots(&self) -> Vec<u64> {
        crate::gil_assert();
        self.static_bindings
            .lock()
            .unwrap()
            .iter()
            .filter_map(|binding| {
                crate::obj_from_bits(binding.bits)
                    .as_ptr()
                    .filter(|ptr| unsafe { object_type_id(*ptr) } == crate::TYPE_ID_TYPE)
                    .map(|_| binding.bits)
            })
            .collect()
    }

    /// Release exact static-class anchors only after the last permitted C
    /// callback drain. An isolate that never published these shells owns none.
    pub(crate) fn retire_static_bindings(&self, py: &crate::PyToken<'_>) {
        crate::gil_assert();
        let bindings = {
            let mut owned = self.static_bindings.lock().unwrap();
            std::mem::take(&mut *owned)
        };
        // Remove every ingress/reverse identity before releasing any anchor.
        // Reference destruction runs outside both the ownership and bridge locks.
        for binding in &bindings {
            assert!(
                unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .unbind_static_pyobj_from_runtime_handle(binding.pointer, binding.bits)
                },
                "static C shell lost its owning runtime binding before retirement"
            );
        }
        for binding in bindings {
            dec_ref_bits(py, binding.bits);
        }
        self.callables.lock().unwrap().clear();
        *self.registration.lock().unwrap() = HookRegistrationState::Retired;
        self.ready.notify_all();
    }
}

struct HookRegistrationGuard<'a> {
    runtime: &'a CpythonRuntimeState,
    owner: std::thread::ThreadId,
    committed: bool,
}

impl<'a> HookRegistrationGuard<'a> {
    fn begin(runtime: &'a CpythonRuntimeState) -> Option<Self> {
        let owner = std::thread::current().id();
        let mut state = runtime
            .registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match *state {
                HookRegistrationState::Ready | HookRegistrationState::Retiring => return None,
                HookRegistrationState::Retired => {
                    panic!("CPython bootstrap entered a retired runtime")
                }
                HookRegistrationState::Failed => panic!("CPython bootstrap previously failed"),
                HookRegistrationState::Initializing {
                    owner: active_owner,
                } if active_owner == owner => {
                    return None;
                }
                HookRegistrationState::Initializing { .. } => {
                    state = runtime
                        .ready
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                HookRegistrationState::Uninitialized => {
                    *state = HookRegistrationState::Initializing { owner };
                    return Some(Self {
                        runtime,
                        owner,
                        committed: false,
                    });
                }
            }
        }
    }

    fn commit(mut self) {
        let mut state = self
            .runtime
            .registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            *state,
            HookRegistrationState::Initializing { owner: self.owner },
            "CPython hook registration lost publication ownership"
        );
        *state = HookRegistrationState::Ready;
        self.committed = true;
        self.runtime.ready.notify_all();
    }
}

impl Drop for HookRegistrationGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // A partial bootstrap must not leave any old shell fields or bridge
        // identities available to a future runtime. Roll back under the same
        // runtime's GIL custody before marking the transaction failed.
        self.runtime.retire_static_roots();
        with_gil(|py| self.runtime.retire_static_bindings(&py));
        let mut state = self
            .runtime
            .registration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = HookRegistrationState::Failed;
        self.runtime.ready.notify_all();
    }
}

fn admit_process_cpython_state(py: &crate::PyToken<'_>) -> bool {
    if crate::state::runtime_state::owns_process_cpython_state(crate::runtime_state(py)) {
        return true;
    }
    crate::raise_exception::<u64>(
        py,
        "RuntimeError",
        "process-static C extensions are not supported in an isolated runtime",
    );
    false
}

/// Publish each lazy Context type into its process-lived C shell once. The
/// runtime class cache remains its semantic authority; static_bindings owns the
/// ordinary retained C projection and participates in class retirement.
pub(crate) fn bind_context_class(
    py: &crate::PyToken<'_>,
    bits: u64,
    shape: crate::object::ObjectShapeId,
) -> bool {
    if bits == 0 || crate::exception_pending(py) {
        return false;
    }
    // Context semantics belong to every runtime. Only the process C-shell owner
    // publishes these optional ABI projections; isolated runtime classes must
    // not attempt to rebind process-static C identities.
    if !crate::state::runtime_state::owns_process_cpython_state(crate::runtime_state(py)) {
        return true;
    }
    if !register_cpython_hooks() {
        return false;
    }
    let pointer = match shape {
        crate::object::ObjectShapeId::Context => {
            (&raw mut molt_cpython_abi::abi_types::PyContext_Type).cast::<PyObject>()
        }
        crate::object::ObjectShapeId::ContextVar => {
            (&raw mut molt_cpython_abi::abi_types::PyContextVar_Type).cast::<PyObject>()
        }
        crate::object::ObjectShapeId::ContextToken => {
            (&raw mut molt_cpython_abi::abi_types::PyContextToken_Type).cast::<PyObject>()
        }
        _ => return true,
    };
    let runtime = &crate::runtime_state(py).cpython;
    if let Some(binding) = runtime
        .static_bindings
        .lock()
        .unwrap()
        .iter()
        .find(|binding| binding.pointer == pointer)
    {
        assert_eq!(
            binding.bits, bits,
            "Context C shell bound to a different owner"
        );
        return true;
    }
    unsafe {
        runtime.bind_static(py, pointer, bits, true);
    }
    true
}

/// Register the runtime hooks into `molt-lang-cpython-abi`.
/// Install process-lived hooks once and publish class bindings once per runtime.
pub fn register_cpython_hooks() -> bool {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    with_gil(|_py| {
        if !admit_process_cpython_state(&_py) {
            return false;
        }
        let state = crate::runtime_state(&_py);
        let runtime = &state.cpython;
        let Some(registration) = HookRegistrationGuard::begin(runtime) else {
            return true;
        };
        let builtins = crate::builtin_classes(&_py);
        for (class_bits, type_object) in [
            (
                builtins.object,
                (&raw mut molt_cpython_abi::abi_types::PyBaseObject_Type).cast::<PyObject>(),
            ),
            (
                builtins.type_obj,
                (&raw mut molt_cpython_abi::abi_types::PyType_Type).cast::<PyObject>(),
            ),
            (
                builtins.none_type,
                (&raw mut molt_cpython_abi::abi_types::PyNone_Type).cast::<PyObject>(),
            ),
            (
                builtins.int,
                (&raw mut molt_cpython_abi::abi_types::PyLong_Type).cast::<PyObject>(),
            ),
            (
                builtins.float,
                (&raw mut molt_cpython_abi::abi_types::PyFloat_Type).cast::<PyObject>(),
            ),
            (
                builtins.complex,
                (&raw mut molt_cpython_abi::abi_types::PyComplex_Type).cast::<PyObject>(),
            ),
            (
                builtins.bool,
                (&raw mut molt_cpython_abi::abi_types::PyBool_Type).cast::<PyObject>(),
            ),
            (
                builtins.str,
                (&raw mut molt_cpython_abi::abi_types::PyUnicode_Type).cast::<PyObject>(),
            ),
            (
                builtins.bytes,
                (&raw mut molt_cpython_abi::abi_types::PyBytes_Type).cast::<PyObject>(),
            ),
            (
                builtins.bytearray,
                (&raw mut molt_cpython_abi::abi_types::PyByteArray_Type).cast::<PyObject>(),
            ),
            (
                builtins.range,
                (&raw mut molt_cpython_abi::abi_types::PyRange_Type).cast::<PyObject>(),
            ),
            (
                crate::builtins::types::mappingproxy_class_bits(&_py),
                (&raw mut molt_cpython_abi::abi_types::PyDictProxy_Type).cast::<PyObject>(),
            ),
            (
                builtins.list,
                (&raw mut molt_cpython_abi::abi_types::PyList_Type).cast::<PyObject>(),
            ),
            (
                builtins.tuple,
                (&raw mut molt_cpython_abi::abi_types::PyTuple_Type).cast::<PyObject>(),
            ),
            (
                builtins.dict,
                (&raw mut molt_cpython_abi::abi_types::PyDict_Type).cast::<PyObject>(),
            ),
            (
                builtins.set,
                (&raw mut molt_cpython_abi::abi_types::PySet_Type).cast::<PyObject>(),
            ),
            (
                builtins.frozenset,
                (&raw mut molt_cpython_abi::abi_types::PyFrozenSet_Type).cast::<PyObject>(),
            ),
            (
                builtins.slice,
                (&raw mut molt_cpython_abi::abi_types::PySlice_Type).cast::<PyObject>(),
            ),
            (
                builtins.memoryview,
                (&raw mut molt_cpython_abi::abi_types::PyMemoryView_Type).cast::<PyObject>(),
            ),
            (
                builtins.traceback,
                (&raw mut molt_cpython_abi::abi_types::PyTraceBack_Type).cast::<PyObject>(),
            ),
            (
                builtins.module,
                (&raw mut molt_cpython_abi::abi_types::PyModule_Type).cast::<PyObject>(),
            ),
            (
                builtins.generic_alias,
                (&raw mut molt_cpython_abi::abi_types::Py_GenericAliasType).cast::<PyObject>(),
            ),
            (
                crate::builtins::types::method_class(&_py),
                (&raw mut molt_cpython_abi::abi_types::PyMethod_Type).cast::<PyObject>(),
            ),
            (
                builtins.builtin_function_or_method,
                (&raw mut molt_cpython_abi::abi_types::PyCFunction_Type).cast::<PyObject>(),
            ),
            (
                builtins.classmethod,
                (&raw mut molt_cpython_abi::abi_types::PyClassMethod_Type).cast::<PyObject>(),
            ),
            (
                builtins.staticmethod,
                (&raw mut molt_cpython_abi::abi_types::PyStaticMethod_Type).cast::<PyObject>(),
            ),
            (
                builtins.builtin_method,
                (&raw mut molt_cpython_abi::abi_types::PyCMethod_Type).cast::<PyObject>(),
            ),
        ] {
            unsafe { runtime.bind_static(&_py, type_object, class_bits, true) };
        }
        for exception in molt_cpython_abi::abi_types::exc_singleton_ptrs() {
            let Some(c_name) = molt_cpython_abi::abi_types::exc_singleton_name(exception) else {
                continue;
            };
            let requested_name = c_name.strip_prefix("PyExc_").unwrap_or(c_name);
            let Some(spec) = molt_obj_model::builtin_exception_spec(requested_name) else {
                continue;
            };
            let class_bits = crate::exception_type_bits_from_name(&_py, spec.canonical_name());
            if class_bits == 0 {
                continue;
            }
            let canonical_view = spec.canonical_name() == requested_name;
            unsafe { runtime.bind_static(&_py, exception, class_bits, canonical_view) };
        }
        static INSTALL_PROCESS_HOOKS: Once = Once::new();
        INSTALL_PROCESS_HOOKS.call_once(|| {
            let hooks = RuntimeHooks {
                abi_magic: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_MAGIC,
                abi_version: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_VERSION,
                struct_size: std::mem::size_of::<RuntimeHooks>() as u32,
                gil_ensure: hook_gil_ensure,
                gil_leave: hook_gil_leave,
                gil_release: hook_gil_release,
                gil_restore: hook_gil_restore,
                gil_check: hook_gil_check,
                runtime_is_initialized: hook_runtime_is_initialized,
                thread_state_drop_enter: hook_thread_state_drop_enter,
                thread_state_drop_leave: hook_thread_state_drop_leave,
                attached_runtime_context: hook_attached_runtime_context,
                pending_call_error: hook_pending_call_error,
                alloc_str: Some(hook_alloc_str),
                alloc_bytes: hook_alloc_bytes,
                alloc_bytearray: hook_alloc_bytearray,
                numeric_identity_new: Some(hook_numeric_identity_new),
                float_payload: hook_float_payload,
                int_from_i64: hook_int_from_i64,
                int_from_u64: hook_int_from_u64,

                int_from_digits: hook_int_from_digits,
                int_from_f64_trunc: hook_int_from_f64_trunc,
                int_sign: hook_int_sign,

                int_from_bytes: hook_int_from_bytes,
                int_to_bytes: hook_int_to_bytes,
                int_num_bits: hook_int_num_bits,
                int_max_str_digits: hook_int_max_str_digits,
                complex_parts: hook_complex_parts,
                complex_from_doubles: hook_complex_from_doubles,
                alloc_list: hook_alloc_list,
                alloc_list_presized: hook_alloc_list_presized,
                list_append: hook_list_append,
                list_len: hook_list_len,
                list_item: hook_list_item,
                list_set: hook_list_set,
                list_insert: hook_list_insert,
                list_sort: hook_list_sort,
                list_reverse: hook_list_reverse,
                list_set_slice: hook_list_set_slice,
                alloc_tuple: Some(hook_alloc_tuple),
                tuple_set: Some(hook_tuple_set),
                tuple_len: Some(hook_tuple_len),
                tuple_item: Some(hook_tuple_item),
                alloc_dict: hook_alloc_dict,
                mappingproxy_new: hook_mappingproxy_new,
                dict_resolve: hook_dict_resolve,
                dict_mutate: hook_dict_mutate,
                dict_get: hook_dict_get,
                dict_pop: hook_dict_pop,
                dict_len: hook_dict_len,
                dict_next: hook_dict_next,
                str_data: hook_str_data,
                unicode_new: unicode::hook_unicode_new,
                unicode_commit: unicode::hook_unicode_commit,
                unicode_encode: unicode::hook_unicode_encode,
                bytes_data: hook_bytes_data,
                bytearray_data: hook_bytearray_data,
                bytearray_resize: hook_bytearray_resize,
                buffer_supports: hook_buffer_supports,
                buffer_acquire: hook_buffer_acquire,
                buffer_release: hook_buffer_release,
                object_get_attr: hook_object_get_attr,
                type_dict_borrowed: hook_type_dict_borrowed,
                type_metadata: hook_type_metadata,
                builtin_slot_owner: hook_builtin_slot_owner,
                type_lookup_borrowed: hook_type_lookup_borrowed,
                object_set_attr: hook_object_set_attr,
                descriptor_protocol: hook_descriptor_protocol,
                descriptor_get: hook_descriptor_get,
                descriptor_set: hook_descriptor_set,
                object_format: hook_object_format,
                object_str: hook_object_str,
                object_repr: hook_object_repr,
                object_is_true: hook_object_is_true,
                object_length: hook_object_length,
                object_get_item: hook_object_get_item,
                object_supports_subscript: hook_object_supports_subscript,
                object_set_item: hook_object_set_item,
                object_get_iter: hook_object_get_iter,
                iter_check: hook_iter_check,
                iter_next: hook_iter_next,
                object_richcompare: hook_object_richcompare,
                object_richcompare_builtin: hook_object_richcompare_builtin,
                sys_get_object_borrowed: hook_sys_get_object_borrowed,
                eval_get_builtins_borrowed: hook_eval_get_builtins_borrowed,
                classify_heap: Some(hook_classify_heap),
                object_hash: hook_object_hash,
                inc_ref: hook_inc_ref,
                dec_ref: hook_dec_ref,
                ref_count: Some(hook_ref_count),
                try_mark_abi_view: hook_try_mark_abi_view,
                alloc_module: hook_alloc_module,
                alloc_extension_module: hook_alloc_extension_module,
                module_get_dict_borrowed: hook_module_get_dict_borrowed,
                import_add_module_borrowed: hook_import_add_module_borrowed,
                module_set_attr: hook_module_set_attr,
                module_capi_register: hook_module_capi_register,
                module_capi_get_state: hook_module_capi_get_state,
                module_capi_get_def: hook_module_capi_get_def,
                module_state_add: hook_module_state_add,
                module_state_find: hook_module_state_find,
                module_state_remove: hook_module_state_remove,
                module_exec_begin: hook_module_exec_begin,
                register_c_function: Some(hook_register_c_function),
                import_module: hook_import_module,
                initialize_extension: hook_initialize_extension,
                exception_pending: hook_exception_pending,
                pending_exception_class: hook_pending_exception_class,
                number_binary_op: hook_number_binary_op,
                number_unary_op: hook_number_unary_op,
                number_power: hook_number_power,
                target_python_minor: hook_target_python_minor,
                dict_op: hook_dict_op,
                set_op: hook_set_op,
                set_new: hook_set_new,
                set_size: hook_set_size,
                set_contains: hook_set_contains,
                set_add: hook_set_add,
                set_discard: hook_set_discard,
                object_dir: hook_object_dir,
                object_call: hook_object_call,
                object_vectorcall: hook_object_vectorcall,
                method_new: Some(hook_method_new),
                method_part: hook_method_part,
                object_is_callable: crate::builtins::callable::molt_is_callable_bool,
                foreign_new: hook_foreign_new,
                report_unraisable: hook_report_unraisable,
                exception_set_field: hook_exception_set_field,
                exception_get_field: hook_exception_get_field,
                runtime_class_borrowed: Some(hook_runtime_class_borrowed),
                exception_layout_kind: hook_exception_layout_kind,
                exception_snapshot: hook_exception_snapshot,
                exception_commit_snapshot: hook_exception_commit_snapshot,
                type_is_subtype: hook_type_is_subtype,
                object_classinfo_match: Some(hook_object_classinfo_match),
                take_pending_exception: hook_take_pending_exception,
                clear_pending_exception: hook_clear_pending_exception,
                with_preserved_pending_exception: hook_with_preserved_pending_exception,
                handled_exception_get: hook_handled_exception_get,
                handled_exception_set: hook_handled_exception_set,
                native_gc_allocate: Some(hook_native_gc_allocate),
                managed_gc_control: gc_control::hook_managed_gc_control,
                managed_gc_traverse: gc_control::hook_managed_gc_traverse,
                managed_gc_clear: gc_control::hook_managed_gc_clear,
                native_gc_track: hook_native_gc_track,
                native_gc_untrack: hook_native_gc_untrack,
                native_gc_deallocate: hook_native_gc_deallocate,
                native_gc_is_tracked: hook_native_gc_is_tracked,
                native_gc_is_finalized: hook_native_gc_is_finalized,
                native_gc_claim_finalizer: hook_native_gc_claim_finalizer,
                gc_collect: hook_gc_collect,
                gc_enable: hook_gc_enable,
                gc_disable: hook_gc_disable,
                gc_is_enabled: hook_gc_is_enabled,
                check_signals: hook_check_signals,
                set_interrupt: hook_set_interrupt,
                interrupt_occurred: hook_interrupt_occurred,
                notify_pending_calls: hook_notify_pending_calls,
                sequence_check: hook_sequence_check,
                sequence_item: hook_sequence_item,
                object_length_hint: hook_object_length_hint,
                tuple_uses_length_hint: hook_tuple_uses_length_hint,
                exception_group_admit: hook_exception_group_admit,
                private_c_heap_contains: crate::c_api::molt_c_heap_contains,
                object_bytes: hook_object_bytes,
                memoryview_new: hook_memoryview_new,
                memoryview_release: hook_memoryview_release,
                memoryview_from_buffer: hook_memoryview_from_buffer,
                memoryview_snapshot: hook_memoryview_snapshot,
                slice_new: slice::hook_slice_new,
                slice_item: slice::hook_slice_item,
                object_contains: hook_object_contains,
                context_type_admit: contextvars::type_admit,
                context_new: contextvars::new,
                context_copy_current: contextvars::copy_current,
                context_copy: contextvars::copy,
                context_enter: contextvars::enter,
                context_exit: contextvars::exit,
                context_var_new: contextvars::var_new,
                context_var_get: contextvars::var_get,
                context_var_set: contextvars::var_set,
                context_var_reset: contextvars::var_reset,
            };
            // SAFETY: all fn pointers are valid for the process lifetime.
            let installed = unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) };
            assert!(
                installed,
                "CPython runtime hooks were registered by a second authority"
            );
        });
        unsafe { molt_cpython_abi::abi_types::prepare_builtin_static_type_runtime_state() };
        assert_eq!(
            unsafe { molt_cpython_abi::abi_types::ready_exception_singleton_types() },
            0,
            "CPython exception singleton types failed readiness after production hook publication"
        );
        registration.commit();
        true
    })
}

#[cfg(test)]
mod allocation_tests;
#[cfg(test)]
mod buffer_semantics_tests;
#[cfg(test)]
mod cfunction_tests;
#[cfg(test)]
mod exception_group_admission_tests;
#[cfg(test)]
mod exception_rendering_tests;
#[cfg(test)]
mod generic_attributes_tests;
#[cfg(test)]
mod immortal_ownership_tests;
#[cfg(test)]
mod inquiry_tests;
#[cfg(test)]
mod native_lifecycle_tests;
#[cfg(test)]
mod native_namespace_tests;
#[cfg(test)]
mod native_raised_state_tests;
#[cfg(test)]
mod native_test_fixture;
#[cfg(test)]
mod pending_cleanup_tests;
#[cfg(test)]
mod slice_semantics_tests;
#[cfg(test)]
mod type_watcher_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TYPE_ID_BYTEARRAY;
    use molt_cpython_abi::abi_types::{
        PyBaseExceptionObject, PyExc_AttributeError, PyExc_IndexError, PyExc_LookupError,
        PyExc_MemoryError, PyExc_RuntimeError, PyExc_TypeError, PyExc_UnicodeDecodeError,
        PyExc_UnicodeEncodeError, PyExc_ValueError, PyListObject, PyMethodDef, PyModuleDef_Base,
        PyModuleDef_Slot, PyObject, PyTypeObject,
    };
    use molt_cpython_abi::api::refcount::OwnedPyObject;
    use std::cell::UnsafeCell;
    use std::ffi::c_void;
    use std::os::raw::c_int;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize as TestAtomicUsize};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::{Duration, Instant};

    static CANONICAL_EXEC_MODULE_BITS: AtomicU64 = AtomicU64::new(0);
    static CANONICAL_EXEC_MODULE_NAME_BITS: AtomicU64 = AtomicU64::new(0);
    static CANONICAL_EXEC_MODULE_VIEW: AtomicUsize = AtomicUsize::new(0);
    static CANONICAL_EXEC_SAW_PUBLICATION: AtomicBool = AtomicBool::new(false);
    static CANONICAL_EXEC_FAIL_ONCE: AtomicBool = AtomicBool::new(false);
    static PENDING_CALL_TEST_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
    static SHUTDOWN_DRAIN_CEXT_CLOSURE_BITS: AtomicU64 = AtomicU64::new(0);
    static SHUTDOWN_DRAIN_CEXT_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn shutdown_drain_cext_reentry() {
        assert!(
            crate::concurrency::execution::current_thread_has_c_extension_execution_context(),
            "shutdown finalizer callback lost destruction execution custody"
        );
        let closure_bits = SHUTDOWN_DRAIN_CEXT_CLOSURE_BITS.swap(0, AtomicOrdering::AcqRel);
        let result = molt_cpython_abi_cext_call_trampoline_admitted(closure_bits, 0, 0);
        assert!(MoltObject::from_bits(result as u64).is_none());
        with_gil(|py| dec_ref_bits(&py, closure_bits));
        SHUTDOWN_DRAIN_CEXT_CALLBACKS.fetch_add(1, AtomicOrdering::AcqRel);
    }

    unsafe extern "C" fn pending_call_test_noop(_arg: *mut c_void) -> c_int {
        PENDING_CALL_TEST_CALLBACKS.fetch_add(1, AtomicOrdering::Relaxed);
        0
    }

    unsafe extern "C" fn pending_call_test_type_error(_arg: *mut c_void) -> c_int {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
                c"exact pending-call callback TypeError".as_ptr(),
            )
        };
        -1
    }

    unsafe extern "C" fn pending_call_test_runtime_type_error(_arg: *mut c_void) -> c_int {
        PENDING_CALL_TEST_CALLBACKS.fetch_add(1, AtomicOrdering::Relaxed);
        crate::with_gil_entry_nopanic!(_py, {
            let _ = crate::builtins::exceptions::raise_exception::<u64>(
                _py,
                "TypeError",
                "exact runtime pending-call TypeError",
            );
        });
        -1
    }

    fn borrowed_bits(result: BorrowedHandleResult) -> Option<u64> {
        match result.decode() {
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => Some(bits),
            molt_cpython_abi::hooks::DecodedHandleResult::Missing
            | molt_cpython_abi::hooks::DecodedHandleResult::Error => None,
        }
    }

    #[test]
    fn exception_snapshot_commit_rejects_before_mutation_then_publishes_whole_state() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let exception_ptr = crate::builtins::exceptions::alloc_exception(
                _py,
                "ValueError",
                "snapshot transaction",
            );
            assert!(!exception_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let before = unsafe {
                [
                    crate::exception_dict_bits(exception_ptr),
                    crate::exception_args_bits(exception_ptr),
                    crate::exception_notes_bits(exception_ptr),
                    crate::exception_trace_bits(exception_ptr),
                    crate::exception_context_bits(exception_ptr),
                    crate::exception_cause_bits(exception_ptr),
                    crate::exception_args_payload_bits(exception_ptr),
                    crate::exception_suppress_bits(exception_ptr),
                ]
            };
            let invalid_dict_ptr = alloc_string(_py, b"not a dict");
            let invalid_dict_bits = MoltObject::from_ptr(invalid_dict_ptr).bits();
            let invalid = ExceptionSnapshot {
                present_mask: EXCEPTION_SNAPSHOT_DICT | EXCEPTION_SNAPSHOT_ARGS,
                dict: invalid_dict_bits,
                args: before[1],
                ..ExceptionSnapshot::default()
            };
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const invalid) },
                -1
            );
            let after_rejection = unsafe {
                [
                    crate::exception_dict_bits(exception_ptr),
                    crate::exception_args_bits(exception_ptr),
                    crate::exception_notes_bits(exception_ptr),
                    crate::exception_trace_bits(exception_ptr),
                    crate::exception_context_bits(exception_ptr),
                    crate::exception_cause_bits(exception_ptr),
                    crate::exception_args_payload_bits(exception_ptr),
                    crate::exception_suppress_bits(exception_ptr),
                ]
            };
            assert_eq!(
                after_rejection, before,
                "rejected commit mutated exception state"
            );
            dec_ref_bits(_py, invalid_dict_bits);

            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let valid = ExceptionSnapshot {
                present_mask: EXCEPTION_SNAPSHOT_DICT | EXCEPTION_SNAPSHOT_ARGS,
                suppress_context: 1,
                dict: dict_bits,
                args: before[1],
                ..ExceptionSnapshot::default()
            };
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const valid) },
                0
            );
            assert_eq!(
                unsafe { crate::exception_dict_bits(exception_ptr) },
                dict_bits
            );
            assert!(
                crate::obj_from_bits(unsafe { crate::exception_suppress_bits(exception_ptr) })
                    .as_bool()
                    .unwrap_or(false)
            );
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, exception_bits);
        });
    }

    #[test]
    fn exception_snapshot_roundtrips_layout_and_typed_fields_atomically() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let exception_ptr = crate::builtins::exceptions::alloc_exception(
                _py,
                "AttributeError",
                "typed snapshot",
            );
            assert!(!exception_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let name_ptr = alloc_string(_py, b"before-name");
            let object_ptr = alloc_string(_py, b"before-object");
            assert!(!name_ptr.is_null() && !object_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let object_bits = MoltObject::from_ptr(object_ptr).bits();
            crate::builtins::exceptions::exception_typed_fields_replace_internal(
                _py,
                exception_bits,
                &[
                    (
                        molt_obj_model::ExceptionTypedField::AttributeErrorName,
                        name_bits,
                    ),
                    (
                        molt_obj_model::ExceptionTypedField::AttributeErrorObject,
                        object_bits,
                    ),
                ],
            )
            .expect("initialize typed AttributeError fields");
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, object_bits);

            let mut snapshot = ExceptionSnapshot::default();
            assert_eq!(
                unsafe { hook_exception_snapshot(exception_bits, &raw mut snapshot) },
                0
            );
            assert_eq!(
                snapshot.layout_kind,
                molt_obj_model::ExceptionLayoutKind::AttributeError as u8
            );
            assert_eq!(snapshot.typed_present_mask, 0b11);
            assert_ne!(snapshot.typed_handles[0], 0);
            assert_ne!(snapshot.typed_handles[1], 0);

            let mut invalid = snapshot;
            invalid.layout_kind = molt_obj_model::ExceptionLayoutKind::NameError as u8;
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const invalid) },
                -1,
                "layout kind is immutable"
            );
            let unchanged = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exception_ptr,
                molt_obj_model::ExceptionTypedField::AttributeErrorName,
            )
            .expect("AttributeError.name descriptor")
            .expect("AttributeError.name before rejected commit");
            assert_eq!(
                crate::string_obj_to_owned(crate::obj_from_bits(unchanged)).as_deref(),
                Some("before-name")
            );
            dec_ref_bits(_py, unchanged);

            let replacement_ptr = alloc_string(_py, b"after-name");
            assert!(!replacement_ptr.is_null());
            let replacement_bits = MoltObject::from_ptr(replacement_ptr).bits();
            dec_ref_bits(_py, snapshot.typed_handles[1]);
            snapshot.typed_handles[1] = replacement_bits;
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const snapshot) },
                0
            );
            let observed = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exception_ptr,
                molt_obj_model::ExceptionTypedField::AttributeErrorName,
            )
            .expect("AttributeError.name descriptor")
            .expect("AttributeError.name value");
            assert_eq!(
                crate::string_obj_to_owned(crate::obj_from_bits(observed)).as_deref(),
                Some("after-name")
            );
            dec_ref_bits(_py, observed);

            for bits in snapshot.present_handles() {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, exception_bits);
        });
    }

    #[test]
    fn raised_error_transfer_drains_superseded_channels_and_preserves_handled_state() {
        use molt_cpython_abi::api::{errors, refcount};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            let first = crate::builtins::exceptions::alloc_exception(py, "ValueError", "first");
            let second = crate::builtins::exceptions::alloc_exception(py, "LookupError", "second");
            assert!(!first.is_null() && !second.is_null());
            let first_bits = MoltObject::from_ptr(first).bits();
            let second_bits = MoltObject::from_ptr(second).bits();
            unsafe {
                let first_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(first_bits);
                let second_view = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(second_bits);
                assert!(!first_view.is_null() && !second_view.is_null());
                errors::PyErr_SetHandledException(second_view);
                refcount::Py_INCREF(first_view);
                errors::PyErr_SetRaisedException(first_view);
                crate::builtins::exceptions::molt_exception_set_last(second_bits);
                with_preserved_error(|| {
                    refcount::Py_INCREF(second_view);
                    errors::PyErr_SetRaisedException(second_view);
                    crate::builtins::exceptions::molt_exception_set_last(first_bits);
                });
                assert_eq!(crate::exception_last_bits_noinc(py), Some(second_bits));
                let restored = errors::PyErr_GetRaisedException();
                let restored_owner = OwnedPyObject::from_owned(restored);
                assert_eq!(restored, first_view);
                drop(restored_owner);
                assert!(!crate::exception_pending(py));
                for operation in 0..4 {
                    refcount::Py_INCREF(first_view);
                    errors::PyErr_SetRaisedException(first_view);
                    crate::builtins::exceptions::molt_exception_set_last(second_bits);
                    assert!(crate::exception_pending(py));
                    match operation {
                        0 => {
                            let value = errors::PyErr_GetRaisedException();
                            let value_owner = OwnedPyObject::from_owned(value);
                            assert_eq!(value, first_view);
                            drop(value_owner);
                        }
                        1 => {
                            let (mut class, mut value, mut traceback) =
                                (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                            errors::PyErr_Fetch(&raw mut class, &raw mut value, &raw mut traceback);
                            let class_owner = OwnedPyObject::from_owned(class);
                            let value_owner = OwnedPyObject::from_owned(value);
                            let traceback_owner = OwnedPyObject::from_owned(traceback);
                            assert_eq!(value, first_view);
                            drop(class_owner);
                            drop(value_owner);
                            drop(traceback_owner);
                        }
                        2 => errors::PyErr_Clear(),
                        _ => errors::PyErr_SetRaisedException(ptr::null_mut()),
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(py));
                    let handled = errors::PyErr_GetHandledException();
                    let handled_owner = OwnedPyObject::from_owned(handled);
                    assert_eq!(handled, second_view);
                    drop(handled_owner);
                }
                crate::builtins::exceptions::molt_exception_set_last(second_bits);
                let value = errors::PyErr_GetRaisedException();
                let value_owner = OwnedPyObject::from_owned(value);
                assert_eq!(
                    value, second_view,
                    "runtime-only raised error is transferred"
                );
                drop(value_owner);
                assert!(!crate::exception_pending(py));
                assert!(errors::PyErr_Occurred().is_null());
                errors::PyErr_SetHandledException(ptr::null_mut());
            }
            dec_ref_bits(py, first_bits);
            dec_ref_bits(py, second_bits);
        });
    }

    #[test]
    fn exception_rendering_managed_abi_uses_declaring_slots_and_owned_string_results() {
        use molt_cpython_abi::api::{errors, refcount, typeobj};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            let key = crate::builtins::exceptions::alloc_exception(py, "KeyError", "x");
            assert!(!key.is_null());
            let key_bits = MoltObject::from_ptr(key).bits();
            let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(key_bits) };
            assert!(!view.is_null());
            for (result, expected) in unsafe {
                [
                    (typeobj::PyObject_Str(view), "'x'"),
                    (typeobj::PyObject_Repr(view), "KeyError('x')"),
                    (errors::molt_native_exception_str(view), "x"),
                    (errors::molt_native_exception_repr(view), "KeyError('x')"),
                ]
            } {
                assert!(!result.is_null());
                let bits = GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(result)
                    .unwrap()
                    .bits();
                assert_eq!(
                    crate::string_obj_to_owned(crate::obj_from_bits(bits)).as_deref(),
                    Some(expected)
                );
                unsafe { refcount::Py_DECREF(result) };
            }
            dec_ref_bits(py, key_bits);

            let argument = alloc_string(py, &[0xed, 0xa0, 0x80]);
            assert!(!argument.is_null());
            let argument_bits = MoltObject::from_ptr(argument).bits();
            let error =
                crate::builtins::exceptions::molt_exception_new_builtin_one(5, argument_bits);
            dec_ref_bits(py, argument_bits);
            let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(error) };
            let rendered = unsafe { typeobj::PyObject_Str(view) };
            assert!(!rendered.is_null());
            let rendered_bits = GLOBAL_BRIDGE
                .observed_handle_for_pyobj(rendered)
                .unwrap()
                .bits();
            assert_eq!(
                rendered_bits, argument_bits,
                "single string args keep their string identity"
            );
            let replacement = crate::alloc_tuple(py, &[MoltObject::from_int(7).bits()]);
            let replacement_bits = MoltObject::from_ptr(replacement).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                py,
                error,
                crate::builtins::exceptions::ExceptionFieldSlot::Args,
                replacement_bits,
            )
            .unwrap();
            dec_ref_bits(py, replacement_bits);
            dec_ref_bits(py, error);
            let argument = crate::obj_from_bits(rendered_bits).as_ptr().unwrap();
            assert_eq!(
                unsafe {
                    std::slice::from_raw_parts(
                        crate::string_bytes(argument),
                        crate::string_len(argument),
                    )
                },
                &[0xed, 0xa0, 0x80]
            );
            unsafe { refcount::Py_DECREF(rendered) };
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn managed_type_name_observers_share_current_structural_metadata() {
        use molt_cpython_abi::api::{refcount, typeobj};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(py, {
            let name = MoltObject::from_ptr(alloc_string(py, b"Original")).bits();
            let class_bits = crate::molt_class_new(name);
            let class = crate::obj_from_bits(class_bits)
                .as_ptr()
                .expect("class allocation");
            let base_result =
                crate::molt_class_set_base(class_bits, crate::builtin_classes(py).object);
            crate::dec_ref_bits(py, base_result);
            unsafe { crate::object::class_finish_definition(py, class) }
                .expect("complete the class before projecting its hierarchy");
            let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(class_bits) }
                .cast::<molt_cpython_abi::abi_types::PyTypeObject>();
            assert!(!view.is_null());
            let qualified = MoltObject::from_ptr(alloc_string(py, b"Outer.Q")).bits();
            assert!(unsafe { crate::class_set_qualname_bits(py, class, qualified) });
            let observed_name = unsafe { typeobj::PyType_GetName(view) };
            let observed_qualname = unsafe { typeobj::PyType_GetQualName(view) };
            assert_eq!(
                GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(observed_name)
                    .unwrap()
                    .bits(),
                name
            );
            assert_eq!(
                GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(observed_qualname)
                    .unwrap()
                    .bits(),
                qualified
            );
            unsafe {
                refcount::Py_DECREF(observed_name);
                refcount::Py_DECREF(observed_qualname);
            }
            {
                let raw = b"prefix.Renamed".as_slice();
                let renamed = MoltObject::from_ptr(alloc_string(py, raw)).bits();
                assert!(unsafe { crate::class_set_name_bits(py, class, renamed) });
                assert_eq!(
                    unsafe { std::ffi::CStr::from_ptr((*view).tp_name) }.to_bytes(),
                    raw
                );
                let observed = unsafe { typeobj::PyType_GetName(view) };
                assert_eq!(
                    GLOBAL_BRIDGE
                        .observed_handle_for_pyobj(observed)
                        .unwrap()
                        .bits(),
                    renamed
                );
                unsafe { refcount::Py_DECREF(observed) };
                dec_ref_bits(py, renamed);
            }
            let heap = view.cast::<molt_cpython_abi::abi_types::PyHeapTypeObject>();
            let previous_name = unsafe { crate::class_name_bits(class) };
            let previous_c_name = unsafe { (*view).tp_name };
            let previous_heap_name = unsafe { (*heap).ht_name };
            let invalid = MoltObject::from_ptr(alloc_string(py, &[0xed, 0xa0, 0x80])).bits();
            assert!(!unsafe { crate::class_set_name_bits(py, class, invalid) });
            assert!(crate::exception_pending(py));
            let exception = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                exception,
                "UnicodeEncodeError"
            ));
            dec_ref_bits(py, exception);
            crate::molt_exception_clear();
            assert_eq!(unsafe { crate::class_name_bits(class) }, previous_name);
            assert_eq!(unsafe { (*view).tp_name }, previous_c_name);
            assert_eq!(unsafe { (*heap).ht_name }, previous_heap_name);
            dec_ref_bits(py, invalid);
            for bits in [class_bits, name, qualified] {
                dec_ref_bits(py, bits);
            }
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn exception_snapshot_zero_fields_survive_bidirectional_physical_projection() {
        use crate::builtins::exceptions::{
            ExceptionFieldSlot, exception_field_is_missing, exception_field_missing_bits,
            exception_replace_field_bits, exception_typed_field_get,
            exception_typed_fields_replace_internal,
        };
        use molt_cpython_abi::abi_types::PyAttributeErrorObject;
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        use molt_obj_model::ExceptionTypedField::{AttributeErrorName, AttributeErrorObject};

        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let ptr =
                crate::builtins::exceptions::alloc_exception(_py, "AttributeError", "zero fields");
            assert!(!ptr.is_null());
            let bits = MoltObject::from_ptr(ptr).bits();
            for value in [0.0f64.to_bits(), (-0.0f64).to_bits(), 0.0f64.to_bits()] {
                exception_typed_fields_replace_internal(
                    _py,
                    bits,
                    &[(AttributeErrorObject, value), (AttributeErrorName, value)],
                )
                .expect("publish typed zero fields");
                exception_replace_field_bits(_py, bits, ExceptionFieldSlot::Notes, value)
                    .expect("publish zero notes");
                let mut snapshot = ExceptionSnapshot::default();
                assert_eq!(
                    unsafe { hook_exception_snapshot(bits, &raw mut snapshot) },
                    0
                );
                assert_eq!(snapshot.typed_present_mask, 0b11);
                assert_ne!(snapshot.present_mask & EXCEPTION_SNAPSHOT_NOTES, 0);
                assert_eq!(snapshot.typed_handles[..2], [value, value]);
                assert_eq!(snapshot.notes, value);

                let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) }
                    .cast::<PyAttributeErrorObject>();
                assert!(!view.is_null());
                for field in unsafe { [(*view).obj, (*view).name, (*view).base.notes] } {
                    assert!(!field.is_null(), "present boxed zero became a NULL C field");
                    let observed = unsafe { GLOBAL_BRIDGE.molt_value_for_pyobj(field) }
                        .expect("physical field maps to a runtime value");
                    assert_eq!(observed, value);
                    dec_ref_bits(_py, observed);
                }
                assert!(GLOBAL_BRIDGE.commit_exception_view(bits));
                for name in [AttributeErrorObject, AttributeErrorName] {
                    let observed = exception_typed_field_get(_py, ptr, name)
                        .expect("typed descriptor")
                        .expect("present typed field");
                    assert_eq!(observed, value);
                    dec_ref_bits(_py, observed);
                }

                let mut malformed = snapshot;
                malformed.present_mask |= EXCEPTION_SNAPSHOT_DICT;
                malformed.dict = 0;
                assert_eq!(
                    unsafe { hook_exception_commit_snapshot(bits, &raw const malformed) },
                    -1,
                    "float zero cannot substitute for a dict"
                );
                assert_eq!(unsafe { crate::exception_notes_bits(ptr) }, value);

                // The inbound transaction must preserve the same payload even
                // after the runtime currently holds a different value.
                exception_replace_field_bits(
                    _py,
                    bits,
                    ExceptionFieldSlot::Notes,
                    MoltObject::from_int(42).bits(),
                )
                .unwrap();
                assert_eq!(
                    unsafe { hook_exception_commit_snapshot(bits, &raw const snapshot) },
                    0
                );
                assert_eq!(unsafe { crate::exception_notes_bits(ptr) }, value);
                assert!(GLOBAL_BRIDGE.refresh_exception_view(bits));
                for owned in snapshot.present_handles() {
                    dec_ref_bits(_py, owned);
                }
            }
            let none = MoltObject::none().bits();
            exception_typed_fields_replace_internal(
                _py,
                bits,
                &[(AttributeErrorObject, none), (AttributeErrorName, none)],
            )
            .unwrap();
            exception_replace_field_bits(_py, bits, ExceptionFieldSlot::Notes, none).unwrap();
            let view = unsafe { GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) }
                .cast::<PyAttributeErrorObject>();
            assert!(!view.is_null());
            assert!(unsafe {
                (*view).obj == &raw mut molt_cpython_abi::abi_types::Py_None
                    && (*view).name == &raw mut molt_cpython_abi::abi_types::Py_None
                    && (*view).base.notes == &raw mut molt_cpython_abi::abi_types::Py_None
            });
            assert!(GLOBAL_BRIDGE.commit_exception_view(bits));
            let mut snapshot = ExceptionSnapshot::default();
            assert_eq!(
                unsafe { hook_exception_snapshot(bits, &raw mut snapshot) },
                0
            );
            assert_eq!(snapshot.typed_present_mask, 0b11);
            assert_eq!(snapshot.typed_handles[..2], [none, none]);
            assert_ne!(snapshot.present_mask & EXCEPTION_SNAPSHOT_NOTES, 0);
            assert_eq!(snapshot.notes, none);
            let mut rejected = snapshot;
            rejected.layout_kind = molt_obj_model::ExceptionLayoutKind::Base as u8;
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(bits, &raw const rejected) },
                -1
            );
            assert!(GLOBAL_BRIDGE.refresh_exception_view(bits));
            assert_eq!(
                unsafe { (*view).obj },
                &raw mut molt_cpython_abi::abi_types::Py_None
            );
            for owned in snapshot.present_handles() {
                dec_ref_bits(_py, owned);
            }
            for field in [AttributeErrorObject, AttributeErrorName] {
                crate::builtins::exceptions::exception_typed_field_delete(_py, bits, field)
                    .expect("typed descriptor")
                    .expect("delete typed field");
            }
            exception_replace_field_bits(
                _py,
                bits,
                ExceptionFieldSlot::Notes,
                exception_field_missing_bits(),
            )
            .unwrap();
            assert!(GLOBAL_BRIDGE.refresh_exception_view(bits));
            assert!(unsafe {
                (*view).obj.is_null() && (*view).name.is_null() && (*view).base.notes.is_null()
            });
            assert!(GLOBAL_BRIDGE.commit_exception_view(bits));
            assert!(exception_field_is_missing(unsafe {
                crate::exception_notes_bits(ptr)
            }));
            let mut absent = ExceptionSnapshot::default();
            assert_eq!(unsafe { hook_exception_snapshot(bits, &raw mut absent) }, 0);
            assert_eq!(absent.typed_present_mask, 0);
            assert_eq!(absent.present_mask & EXCEPTION_SNAPSHOT_NOTES, 0);
            assert_eq!(absent.notes, 0);
            for owned in absent.present_handles() {
                dec_ref_bits(_py, owned);
            }
            assert!(!crate::exception_pending(_py));
            dec_ref_bits(_py, bits);
        });
    }

    #[test]
    fn exception_snapshot_roundtrips_typed_scalars_without_handle_boxing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let encoding_ptr = alloc_string(_py, b"utf-8");
            let object_ptr = alloc_string(_py, b"x");
            let reason_ptr = alloc_string(_py, b"typed scalar snapshot");
            assert!(!encoding_ptr.is_null() && !object_ptr.is_null() && !reason_ptr.is_null());
            let constructor_fields = [
                MoltObject::from_ptr(encoding_ptr).bits(),
                MoltObject::from_ptr(object_ptr).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_int(5).bits(),
                MoltObject::from_ptr(reason_ptr).bits(),
            ];
            let args_ptr = crate::alloc_tuple(_py, &constructor_fields);
            assert!(!args_ptr.is_null());
            for bits in [
                MoltObject::from_ptr(encoding_ptr).bits(),
                MoltObject::from_ptr(object_ptr).bits(),
                MoltObject::from_ptr(reason_ptr).bits(),
            ] {
                dec_ref_bits(_py, bits);
            }
            let class_bits = crate::builtins::exceptions::exception_type_bits_from_name(
                _py,
                "UnicodeEncodeError",
            );
            let exception_ptr = crate::builtins::exceptions::alloc_exception_from_class_bits(
                _py,
                class_bits,
                MoltObject::from_ptr(args_ptr).bits(),
            );
            assert!(!exception_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            crate::builtins::exceptions::exception_typed_fields_replace_internal(
                _py,
                exception_bits,
                &[
                    (
                        molt_obj_model::ExceptionTypedField::UnicodeStart,
                        MoltObject::from_int(3).bits(),
                    ),
                    (
                        molt_obj_model::ExceptionTypedField::UnicodeEnd,
                        MoltObject::from_int(5).bits(),
                    ),
                ],
            )
            .expect("initialize UnicodeError scalar fields");

            let mut snapshot = ExceptionSnapshot::default();
            assert_eq!(
                unsafe { hook_exception_snapshot(exception_bits, &raw mut snapshot) },
                0
            );
            assert_eq!(
                snapshot.layout_kind,
                molt_obj_model::ExceptionLayoutKind::Unicode as u8
            );
            assert_eq!(snapshot.unicode_start, 3);
            assert_eq!(snapshot.unicode_end, 5);
            assert_ne!(snapshot.typed_handles[0], 0, "encoding stays a handle");
            assert_ne!(snapshot.typed_handles[1], 0, "object stays a handle");
            assert_eq!(snapshot.typed_handles[2], 0, "start is never boxed");
            assert_eq!(snapshot.typed_handles[3], 0, "end is never boxed");
            assert_ne!(snapshot.typed_handles[4], 0, "reason stays a handle");
            assert!(snapshot.typed_handles[5..].iter().all(|bits| *bits == 0));
            snapshot.unicode_start = 7;
            snapshot.unicode_end = 11;
            assert_eq!(
                unsafe { hook_exception_commit_snapshot(exception_bits, &raw const snapshot) },
                0
            );
            let start = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exception_ptr,
                molt_obj_model::ExceptionTypedField::UnicodeStart,
            )
            .expect("UnicodeError.start descriptor")
            .expect("UnicodeError.start value");
            let end = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exception_ptr,
                molt_obj_model::ExceptionTypedField::UnicodeEnd,
            )
            .expect("UnicodeError.end descriptor")
            .expect("UnicodeError.end value");
            assert_eq!(crate::obj_from_bits(start).as_int(), Some(7));
            assert_eq!(crate::obj_from_bits(end).as_int(), Some(11));
            dec_ref_bits(_py, start);
            dec_ref_bits(_py, end);
            for bits in snapshot.present_handles() {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, exception_bits);
        });
    }

    #[test]
    fn exception_landing_parent_projection_fully_initializes_fresh_child() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let child_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "ValueError", "fresh child");
            let parent_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "RuntimeError", "parent");
            assert!(!child_ptr.is_null() && !parent_ptr.is_null());
            let child_bits = MoltObject::from_ptr(child_ptr).bits();
            let parent_bits = MoltObject::from_ptr(parent_ptr).bits();
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    child_bits,
                )
                .expect("install fresh exception child");
            }

            let parent_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(parent_bits)
            };
            assert!(!parent_view.is_null());
            let parent_view = parent_view.cast::<PyBaseExceptionObject>();
            let child_view = unsafe { (*parent_view).context };
            assert!(!child_view.is_null());
            assert_eq!(unsafe { (*parent_view).cause }, child_view);
            let child_view = child_view.cast::<PyBaseExceptionObject>();
            assert!(
                !unsafe { (*child_view).args }.is_null(),
                "a nested fresh exception must publish its mandatory args field"
            );
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .managed_handle_for_pyobj(unsafe { (*child_view).args }),
                Some(unsafe { crate::exception_args_bits(child_ptr) }),
                "the child physical args field must project its canonical runtime slot"
            );

            let none = MoltObject::none().bits();
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    none,
                )
                .expect("clear child edge");
            }
            dec_ref_bits(_py, parent_bits);
            dec_ref_bits(_py, child_bits);
        });
    }

    #[test]
    fn exception_physical_c_owned_fields_retire_without_an_observation_roundtrip() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            use molt_cpython_abi::abi_types::{
                exception_layout_for_type, exception_typed_object_slot,
            };
            use molt_cpython_abi::api::refcount::{Py_INCREF, Py_XDECREF};
            use molt_obj_model::ExceptionTypedField;

            for (kind, typed_field) in [
                ("RuntimeError", None),
                (
                    "AttributeError",
                    Some(ExceptionTypedField::AttributeErrorName),
                ),
                ("SystemExit", Some(ExceptionTypedField::SystemExitCode)),
                (
                    "StopIteration",
                    Some(ExceptionTypedField::StopIterationValue),
                ),
            ] {
                let exception =
                    crate::builtins::exceptions::alloc_exception(py, kind, "physical owner");
                assert!(!exception.is_null());
                let bits = MoltObject::from_ptr(exception).bits();
                let view = unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits)
                }
                .cast::<PyBaseExceptionObject>();
                assert!(!view.is_null());
                // Foreign C-owned values make over-release and leaked physical
                // edges visible without relying on the managed ledger itself.
                let mut first = PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                };
                let mut second = PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type,
                };
                unsafe {
                    let slot = if let Some(field) = typed_field {
                        let layout = exception_layout_for_type((*view).ob_base.ob_type).unwrap();
                        exception_typed_object_slot(view, layout, field).unwrap()
                    } else {
                        &raw mut (*view).notes
                    };
                    for incoming in [&raw mut first, &raw mut second, &raw mut second] {
                        Py_INCREF(incoming);
                        let old = std::mem::replace(&mut *slot, incoming);
                        Py_XDECREF(old);
                    }
                }
                assert_eq!(first.ob_refcnt, 1);
                assert_eq!(second.ob_refcnt, 2);
                // No observation/commit before terminal teardown: native
                // callbacks and failed initializers may exit on exactly this path.
                dec_ref_bits(py, bits);
                assert_eq!(first.ob_refcnt, 1);
                assert_eq!(second.ob_refcnt, 1);
                assert!(!crate::exception_pending(py));
            }
        });
    }

    #[test]
    fn exception_physical_c_only_cycle_is_collected_without_runtime_commit() {
        let _guard = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        crate::with_gil_entry_nopanic!(py, {
            let exception =
                crate::builtins::exceptions::alloc_exception(py, "RuntimeError", "C cycle");
            assert!(!exception.is_null());
            let bits = MoltObject::from_ptr(exception).bits();
            let view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) }
                    .cast::<PyBaseExceptionObject>();
            assert!(!view.is_null());
            unsafe {
                molt_cpython_abi::api::refcount::Py_INCREF(view.cast());
                let old = std::mem::replace(&mut (*view).notes, view.cast());
                molt_cpython_abi::api::refcount::Py_XDECREF(old);
            }
            dec_ref_bits(py, bits);
            assert!(unsafe { crate::object::gc::gc_is_tracked(exception) });
            unsafe { crate::object::gc::collect_cycles(py) };
            // Registry membership queries treat the retired address as opaque.
            assert!(!unsafe { crate::object::gc::gc_is_tracked(exception) });
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn exception_landing_duplicate_physical_fields_release_once_at_parent_dealloc() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let child_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "ValueError", "shared child");
            let parent_ptr =
                crate::builtins::exceptions::alloc_exception(_py, "RuntimeError", "parent");
            assert!(!child_ptr.is_null() && !parent_ptr.is_null());
            let child_bits = MoltObject::from_ptr(child_ptr).bits();
            let parent_bits = MoltObject::from_ptr(parent_ptr).bits();
            let child_baseline =
                unsafe { (*crate::header_from_obj_ptr(child_ptr)).ref_count_snapshot() };
            for field in [
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                crate::builtins::exceptions::ExceptionFieldSlot::Cause,
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(
                    _py,
                    parent_bits,
                    field,
                    child_bits,
                )
                .expect("install shared child");
            }
            let parent_view = unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(parent_bits)
            };
            assert!(!parent_view.is_null());
            let parent_view = parent_view.cast::<PyBaseExceptionObject>();
            let child_view = unsafe { (*parent_view).context };
            assert_eq!(unsafe { (*parent_view).cause }, child_view);
            assert_eq!(unsafe { (*child_view).ob_refcnt }, 3);
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(child_ptr)).ref_count_snapshot() },
                child_baseline + 3,
                "two runtime fields plus one canonical child view hold"
            );

            dec_ref_bits(_py, parent_bits);
            assert_eq!(unsafe { (*child_view).ob_refcnt }, 1);
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(child_ptr)).ref_count_snapshot() },
                child_baseline + 1,
                "parent dealloc must release both runtime and physical field occurrences"
            );
            dec_ref_bits(_py, child_bits);
        });
    }

    #[test]
    fn exception_landing_cyclic_projection_initializes_each_distinct_view_once() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        register_cpython_hooks();
        crate::with_gil_entry_nopanic!(_py, {
            let a_ptr = crate::builtins::exceptions::alloc_exception(_py, "ValueError", "a");
            let b_ptr = crate::builtins::exceptions::alloc_exception(_py, "TypeError", "b");
            assert!(!a_ptr.is_null() && !b_ptr.is_null());
            let a_bits = MoltObject::from_ptr(a_ptr).bits();
            let b_bits = MoltObject::from_ptr(b_ptr).bits();
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                a_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                b_bits,
            )
            .expect("a -> b");
            crate::builtins::exceptions::exception_replace_field_bits(
                _py,
                b_bits,
                crate::builtins::exceptions::ExceptionFieldSlot::Context,
                a_bits,
            )
            .expect("b -> a");

            let a_view =
                unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(a_bits) };
            assert!(!a_view.is_null());
            let a_view = a_view.cast::<PyBaseExceptionObject>();
            let b_view = unsafe { (*a_view).context }.cast::<PyBaseExceptionObject>();
            assert!(!b_view.is_null());
            assert_eq!(unsafe { (*b_view).context }, a_view.cast::<PyObject>());
            assert!(!unsafe { (*a_view).args }.is_null());
            assert!(!unsafe { (*b_view).args }.is_null());

            let none = MoltObject::none().bits();
            for (owner, field) in [
                (
                    a_bits,
                    crate::builtins::exceptions::ExceptionFieldSlot::Context,
                ),
                (
                    b_bits,
                    crate::builtins::exceptions::ExceptionFieldSlot::Context,
                ),
            ] {
                crate::builtins::exceptions::exception_replace_field_bits(_py, owner, field, none)
                    .expect("clear cycle edge");
            }
            dec_ref_bits(_py, a_bits);
            dec_ref_bits(_py, b_bits);
        });
    }

    struct ForeignProxyMutation(UnsafeCell<usize>);

    unsafe impl Send for ForeignProxyMutation {}
    unsafe impl Sync for ForeignProxyMutation {}

    fn fetched_exception_args(value: *mut PyObject) -> Vec<RuntimeValue> {
        use molt_cpython_abi::api::{object, refcount::OwnedPyObject, sequences};
        unsafe {
            let args =
                OwnedPyObject::from_owned(object::PyObject_GetAttrString(value, c"args".as_ptr()));
            assert!(
                !args.as_ptr().is_null(),
                "normalized exception must expose args"
            );
            let count = sequences::PyTuple_Size(args.as_ptr());
            assert!(count >= 0, "exception args must be a tuple");
            (0..count)
                .map(|index| {
                    let item = sequences::PyTuple_GetItem(args.as_ptr(), index);
                    assert!(!item.is_null());
                    runtime_value(item)
                })
                .collect()
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RuntimeValue {
        Bytes(Vec<u8>),
        Int(i64),
        Str(String),
    }

    fn runtime_value(value: *mut PyObject) -> RuntimeValue {
        use molt_cpython_abi::api::{errors, numbers, strings};
        unsafe {
            assert!(!value.is_null());
            if numbers::PyLong_Check(value) != 0 {
                let number = numbers::PyLong_AsLongLong(value);
                assert!(errors::PyErr_Occurred().is_null());
                return RuntimeValue::Int(number);
            }
            let mut length = 0;
            if strings::PyUnicode_Check(value) != 0 {
                let bytes = strings::PyUnicode_AsUTF8AndSize(value, &raw mut length);
                assert!(!bytes.is_null() && length >= 0);
                return RuntimeValue::Str(
                    String::from_utf8(
                        std::slice::from_raw_parts(bytes.cast(), length as usize).to_vec(),
                    )
                    .expect("Unicode fixture field must be valid UTF-8"),
                );
            }
            assert_ne!(
                strings::PyBytes_Check(value),
                0,
                "unexpected fixture field type"
            );
            let mut bytes = ptr::null_mut();
            assert_eq!(
                strings::PyBytes_AsStringAndSize(value, &raw mut bytes, &raw mut length),
                0
            );
            assert!(!bytes.is_null() && length >= 0);
            RuntimeValue::Bytes(std::slice::from_raw_parts(bytes.cast(), length as usize).to_vec())
        }
    }

    fn fetch_unicode_error(expected_type: *mut PyObject) -> (Vec<RuntimeValue>, Vec<RuntimeValue>) {
        let mut exc_type = ptr::null_mut();
        let mut exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &raw mut exc_type,
                &raw mut exc_value,
                ptr::null_mut(),
            )
        };
        let exc_type_owner = unsafe { OwnedPyObject::from_owned(exc_type) };
        let exc_value_owner = unsafe { OwnedPyObject::from_owned(exc_value) };
        assert!(std::ptr::eq(exc_type, expected_type));
        assert!(!exc_value.is_null());
        let args = fetched_exception_args(exc_value);
        let attrs = [c"encoding", c"object", c"start", c"end", c"reason"]
            .into_iter()
            .map(|name| unsafe {
                let attribute = molt_cpython_abi::api::refcount::OwnedPyObject::from_owned(
                    molt_cpython_abi::api::object::PyObject_GetAttrString(exc_value, name.as_ptr()),
                );
                assert!(
                    !attribute.as_ptr().is_null(),
                    "missing Unicode error attribute {name:?}"
                );
                runtime_value(attribute.as_ptr())
            })
            .collect::<Vec<_>>();
        drop(exc_type_owner);
        drop(exc_value_owner);
        (args, attrs)
    }

    fn assert_unicode_error(expected_type: *mut PyObject, expected: Vec<RuntimeValue>) {
        let (args, attrs) = fetch_unicode_error(expected_type);
        assert_eq!(
            args, expected,
            "Unicode error args lost CPython field shape"
        );
        assert_eq!(attrs, args, "Unicode error attributes drifted from args");
    }

    #[test]
    fn fetched_exception_assertion_unwind_retires_transferred_native_owner() {
        use molt_cpython_abi::api::{errors, sequences};
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            for inspect_attributes in [false, true] {
                let args = OwnedPyObject::from_owned(sequences::PyTuple_New(0));
                assert!(!args.as_ptr().is_null());
                let native = OwnedPyObject::from_owned(errors::molt_native_exception_new(
                    &raw mut PyExc_ValueError,
                    args.as_ptr(),
                    ptr::null_mut(),
                ));
                assert!(!native.as_ptr().is_null());
                let address = native.as_ptr().addr();
                assert!(crate::object::gc::native_gc_is_enrolled(address));
                errors::PyErr_SetRaisedException(native.into_ptr());
                let expected_type = if inspect_attributes {
                    (&raw mut PyExc_ValueError).cast::<PyObject>()
                } else {
                    (&raw mut PyExc_TypeError).cast::<PyObject>()
                };
                // Exercise the real helper's early identity assertion and its
                // later missing-attribute assertion with an actual C allocation.
                let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    fetch_unicode_error(expected_type)
                }));
                let failure = failure.expect_err("the deliberately incompatible fixture must fail");
                if inspect_attributes {
                    let message = failure
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| failure.downcast_ref::<&str>().copied())
                        .expect("assertion panic has a text payload");
                    assert!(
                        message.contains("missing Unicode error attribute"),
                        "{message}"
                    );
                    assert_eq!(
                        errors::PyErr_ExceptionMatches((&raw mut PyExc_AttributeError).cast()),
                        1,
                        "releasing transferred owners must preserve the inspection error"
                    );
                } else {
                    assert!(errors::PyErr_Occurred().is_null());
                    assert!(!crate::exception_pending(&py));
                }
                // This is the canonical allocation observer, not a physical
                // refcount guess or an assumption about other live GC objects.
                assert!(
                    !crate::object::gc::native_gc_is_enrolled(address),
                    "assertion unwind leaked the transferred native exception"
                );
                errors::PyErr_Clear();
                assert!(!crate::exception_pending(&py));
            }
        });
    }

    #[test]
    fn c_error_normalization_uses_cpython_argument_shapes() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
                &raw mut molt_cpython_abi::abi_types::Py_None,
            )
        };
        let mut exc_type = ptr::null_mut();
        let mut exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        let exc_type_owner = unsafe { OwnedPyObject::from_owned(exc_type) };
        let exc_value_owner = unsafe { OwnedPyObject::from_owned(exc_value) };
        assert!(std::ptr::eq(
            exc_type,
            (&raw mut PyExc_TypeError).cast::<PyObject>()
        ));
        assert!(!exc_value.is_null());
        assert!(!std::ptr::eq(
            exc_value,
            &raw mut molt_cpython_abi::abi_types::Py_None
        ));
        let args = fetched_exception_args(exc_value);
        assert!(
            args.is_empty(),
            "expected zero exception args, got {args:?}"
        );
        drop(exc_type_owner);
        drop(exc_value_owner);

        let tuple_bits = with_gil(|_py| {
            let tuple_ptr = crate::alloc_tuple(
                &_py,
                &[
                    MoltObject::from_int(11).bits(),
                    MoltObject::from_int(22).bits(),
                ],
            );
            assert!(!tuple_ptr.is_null());
            MoltObject::from_ptr(tuple_ptr).bits()
        });
        let tuple =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(tuple_bits) };
        assert!(!tuple.is_null());
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_ValueError).cast::<PyObject>(),
                tuple,
            );
            molt_cpython_abi::api::refcount::Py_DECREF(tuple);
        }
        exc_type = ptr::null_mut();
        exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        let exc_type_owner = unsafe { OwnedPyObject::from_owned(exc_type) };
        let exc_value_owner = unsafe { OwnedPyObject::from_owned(exc_value) };
        assert!(std::ptr::eq(
            exc_type,
            (&raw mut PyExc_ValueError).cast::<PyObject>()
        ));
        assert_eq!(
            fetched_exception_args(exc_value),
            vec![RuntimeValue::Int(11), RuntimeValue::Int(22)]
        );
        drop(exc_type_owner);
        drop(exc_value_owner);
    }

    #[test]
    fn c_unicode_codec_errors_preserve_cpython_args_and_attributes() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        let decode_type = (&raw mut PyExc_UnicodeDecodeError).cast::<PyObject>();
        let encode_type = (&raw mut PyExc_UnicodeEncodeError).cast::<PyObject>();
        let decode = |encoding: &str, object: &[u8], start: i64, end: i64, reason: &str| {
            assert_unicode_error(
                decode_type,
                vec![
                    RuntimeValue::Str(encoding.to_owned()),
                    RuntimeValue::Bytes(object.to_vec()),
                    RuntimeValue::Int(start),
                    RuntimeValue::Int(end),
                    RuntimeValue::Str(reason.to_owned()),
                ],
            );
        };
        let encode = |encoding: &str, object: &str, start: i64, end: i64, reason: &str| {
            assert_unicode_error(
                encode_type,
                vec![
                    RuntimeValue::Str(encoding.to_owned()),
                    RuntimeValue::Str(object.to_owned()),
                    RuntimeValue::Int(start),
                    RuntimeValue::Int(end),
                    RuntimeValue::Str(reason.to_owned()),
                ],
            );
        };

        let invalid_start = [0xff_u8];
        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_DecodeUTF8(
                    invalid_start.as_ptr().cast(),
                    invalid_start.len() as isize,
                    ptr::null(),
                )
            }
            .is_null()
        );
        decode("utf-8", &invalid_start, 0, 1, "invalid start byte");

        let invalid_continuation = [0xf0_u8, 0x9f, b'('];
        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
                    invalid_continuation.as_ptr().cast(),
                    invalid_continuation.len() as isize,
                )
            }
            .is_null()
        );
        decode(
            "utf-8",
            &invalid_continuation,
            0,
            2,
            "invalid continuation byte",
        );

        let incomplete = [0xe2_u8, 0x82];
        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_DecodeUTF8(
                    incomplete.as_ptr().cast(),
                    incomplete.len() as isize,
                    ptr::null(),
                )
            }
            .is_null()
        );
        decode("utf-8", &incomplete, 0, 2, "unexpected end of data");

        let invalid_ascii = [b'a', 0xff];
        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_DecodeASCII(
                    invalid_ascii.as_ptr().cast(),
                    invalid_ascii.len() as isize,
                    ptr::null(),
                )
            }
            .is_null()
        );
        decode("ascii", &invalid_ascii, 1, 2, "ordinal not in range(128)");

        for (object, start, end, reason) in [
            (&[0xff, 0xfe, b'A'][..], 2, 3, "truncated data"),
            (
                &[0xff, 0xfe, 0x00, 0xd8][..],
                2,
                4,
                "unexpected end of data",
            ),
            (
                &[0xff, 0xfe, 0x00, 0xd8, b'A', 0x00][..],
                2,
                4,
                "illegal UTF-16 surrogate",
            ),
            (&[0xff, 0xfe, 0x00, 0xdc][..], 2, 4, "illegal encoding"),
        ] {
            let mut byteorder = 0;
            assert!(
                unsafe {
                    molt_cpython_abi::api::strings::PyUnicode_DecodeUTF16(
                        object.as_ptr().cast(),
                        object.len() as isize,
                        ptr::null(),
                        &raw mut byteorder,
                    )
                }
                .is_null()
            );
            assert_eq!(byteorder, -1);
            decode("utf-16-le", object, start, end, reason);
        }

        let text = "aé€z";
        let unicode = unsafe {
            molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
                text.as_ptr().cast(),
                text.len() as isize,
            )
        };
        assert!(!unicode.is_null());

        assert!(
            unsafe { molt_cpython_abi::api::strings::PyUnicode_AsASCIIString(unicode) }.is_null()
        );
        encode("ascii", text, 1, 3, "ordinal not in range(128)");

        assert!(
            unsafe { molt_cpython_abi::api::strings::PyUnicode_AsLatin1String(unicode) }.is_null()
        );
        encode("latin-1", text, 2, 3, "ordinal not in range(256)");

        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_AsEncodedString(
                    unicode,
                    c"ascii".as_ptr(),
                    ptr::null(),
                )
            }
            .is_null()
        );
        encode("ascii", text, 1, 3, "ordinal not in range(128)");

        assert!(
            unsafe {
                molt_cpython_abi::api::strings::PyUnicode_AsEncodedString(
                    unicode,
                    c"latin-1".as_ptr(),
                    ptr::null(),
                )
            }
            .is_null()
        );
        encode("latin-1", text, 2, 3, "ordinal not in range(256)");
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(unicode) };
    }

    #[test]
    fn c_error_normalization_preserves_subclass_instance_and_actual_type() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let index_handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj((&raw mut PyExc_IndexError).cast::<PyObject>())
            .expect("IndexError singleton must be bound")
            .bits();
        let type_error_handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj((&raw mut PyExc_TypeError).cast::<PyObject>())
            .expect("TypeError singleton must be bound")
            .bits();
        with_gil(|_py| {
            assert_eq!(crate::class_name_for_error(index_handle), "IndexError");
            assert_eq!(crate::class_name_for_error(type_error_handle), "TypeError");
        });
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Clear();
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut PyExc_IndexError).cast::<PyObject>(),
            );
        }
        let mut original_type = ptr::null_mut();
        let mut original_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut original_type,
                &mut original_value,
                ptr::null_mut(),
            )
        };
        let original_type_owner = unsafe { OwnedPyObject::from_owned(original_type) };
        let original_value_owner = unsafe { OwnedPyObject::from_owned(original_value) };
        let normalized_detail = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(original_value)
            .map(|handle| {
                with_gil(|_py| {
                    crate::obj_from_bits(handle.bits())
                        .as_ptr()
                        .map(|exc_ptr| {
                            format!(
                                "{}: {}",
                                crate::class_name_for_error(unsafe {
                                    crate::object_class_bits(exc_ptr)
                                }),
                                crate::format_exception_message(&_py, exc_ptr)
                            )
                        })
                        .unwrap_or_else(|| "<inline error>".to_owned())
                })
            })
            .unwrap_or_else(|| "<foreign error>".to_owned());
        assert!(
            std::ptr::eq(
                original_type,
                (&raw mut PyExc_IndexError).cast::<PyObject>()
            ),
            "PyErr_SetNone normalized to {} {:p}, expected IndexError {:p}; value={:p}; detail={}",
            molt_cpython_abi::abi_types::exc_singleton_name(original_type)
                .unwrap_or("<non-singleton>"),
            original_type,
            (&raw mut PyExc_IndexError).cast::<PyObject>(),
            original_value,
            normalized_detail,
        );
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetObject(
                (&raw mut PyExc_LookupError).cast::<PyObject>(),
                original_value,
            );
            drop(original_type_owner);
        }
        let mut normalized_type = ptr::null_mut();
        let mut normalized_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut normalized_type,
                &mut normalized_value,
                ptr::null_mut(),
            )
        };
        let normalized_type_owner = unsafe { OwnedPyObject::from_owned(normalized_type) };
        let normalized_value_owner = unsafe { OwnedPyObject::from_owned(normalized_value) };
        assert!(std::ptr::eq(
            normalized_type,
            (&raw mut PyExc_IndexError).cast::<PyObject>()
        ));
        assert!(std::ptr::eq(normalized_value, original_value));
        drop(original_value_owner);
        drop(normalized_type_owner);
        drop(normalized_value_owner);
    }

    #[test]
    fn c_error_restore_and_managed_traceback_get_set_roundtrip_identity() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Clear();
            molt_cpython_abi::api::errors::PyErr_SetNone(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
            );
        }
        let mut exc_type = ptr::null_mut();
        let mut exc_value = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut exc_type,
                &mut exc_value,
                ptr::null_mut(),
            )
        };
        let exc_type_owner = unsafe { OwnedPyObject::from_owned(exc_type) };
        let exc_value_owner = unsafe { OwnedPyObject::from_owned(exc_value) };
        let traceback_bits = with_gil(|_py| unsafe {
            let traceback_class = crate::builtin_classes(&_py).traceback;
            let traceback_class_ptr = crate::obj_from_bits(traceback_class)
                .as_ptr()
                .expect("traceback class must be initialized");
            crate::alloc_instance_for_class(&_py, traceback_class_ptr)
        });
        assert!(!crate::obj_from_bits(traceback_bits).is_none());
        let traceback = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(traceback_bits)
        };
        let traceback_owner = unsafe { OwnedPyObject::from_owned(traceback) };
        assert!(!traceback.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::errors::PyException_SetTraceback(exc_value, traceback)
            },
            0
        );
        let direct = unsafe { molt_cpython_abi::api::errors::PyException_GetTraceback(exc_value) };
        let direct_owner = unsafe { OwnedPyObject::from_owned(direct) };
        assert!(std::ptr::eq(direct, traceback));
        unsafe {
            drop(direct_owner);
            let restored_traceback_owner = OwnedPyObject::from_borrowed(traceback);
            molt_cpython_abi::api::errors::PyErr_Restore(
                exc_type_owner.into_ptr(),
                exc_value_owner.into_ptr(),
                restored_traceback_owner.into_ptr(),
            );
        }
        let mut fetched_type = ptr::null_mut();
        let mut fetched_value = ptr::null_mut();
        let mut fetched_traceback = ptr::null_mut();
        unsafe {
            molt_cpython_abi::api::errors::PyErr_Fetch(
                &mut fetched_type,
                &mut fetched_value,
                &mut fetched_traceback,
            )
        };
        let fetched_type_owner = unsafe { OwnedPyObject::from_owned(fetched_type) };
        let fetched_value_owner = unsafe { OwnedPyObject::from_owned(fetched_value) };
        let fetched_traceback_owner = unsafe { OwnedPyObject::from_owned(fetched_traceback) };
        assert!(std::ptr::eq(
            fetched_type,
            (&raw mut PyExc_TypeError).cast::<PyObject>()
        ));
        assert!(std::ptr::eq(fetched_value, exc_value));
        assert!(std::ptr::eq(fetched_traceback, traceback));
        drop(fetched_type_owner);
        drop(fetched_value_owner);
        drop(fetched_traceback_owner);
        drop(traceback_owner);
    }

    #[test]
    #[ignore = "process-global GIL custody stress; run as the sole selected test"]
    fn gil_custody_recursive_ensure_and_allow_threads_make_progress() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let runtime_guard = GilGuard::new();
        crate::concurrency::gil::hold_runtime_gil(runtime_guard);

        let outer = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        let inner = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        assert_eq!(outer, PY_GIL_STATE_LOCKED);
        assert_eq!(inner, PY_GIL_STATE_LOCKED);
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
            1
        );
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(inner) };
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(outer) };

        const WORKERS: usize = 8;
        const ACQUISITIONS: usize = 2_000;
        let mutation = Arc::new(ForeignProxyMutation(UnsafeCell::new(0)));
        let active = Arc::new(TestAtomicUsize::new(0));
        let max_active = Arc::new(TestAtomicUsize::new(0));
        let started = Arc::new(TestAtomicUsize::new(0));
        let finished = Arc::new(TestAtomicUsize::new(0));
        let stop_watchdog = Arc::new(AtomicBool::new(false));

        let watchdog_finished = Arc::clone(&finished);
        let watchdog_stop = Arc::clone(&stop_watchdog);
        let watchdog = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !watchdog_stop.load(AtomicOrdering::Acquire) {
                assert!(
                    Instant::now() < deadline,
                    "GIL custody stress made no progress"
                );
                if watchdog_finished.load(AtomicOrdering::Acquire) == WORKERS {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let mutation = Arc::clone(&mutation);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let started = Arc::clone(&started);
            let finished = Arc::clone(&finished);
            workers.push(std::thread::spawn(move || {
                started.fetch_add(1, AtomicOrdering::Release);
                for _ in 0..ACQUISITIONS {
                    let state = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                    assert_eq!(
                        state, PY_GIL_STATE_UNLOCKED,
                        "fresh worker acquisition must report unlocked"
                    );
                    let recursive = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
                    assert_eq!(
                        recursive, PY_GIL_STATE_LOCKED,
                        "recursive Ensure must report locked"
                    );
                    unsafe { molt_cpython_abi::api::object::PyGILState_Release(recursive) };
                    let now = active.fetch_add(1, AtomicOrdering::AcqRel) + 1;
                    max_active.fetch_max(now, AtomicOrdering::AcqRel);
                    unsafe {
                        let value = mutation.0.get();
                        *value = (*value).wrapping_add(1);
                    }
                    active.fetch_sub(1, AtomicOrdering::AcqRel);
                    unsafe { molt_cpython_abi::api::object::PyGILState_Release(state) };
                }
                finished.fetch_add(1, AtomicOrdering::Release);
            }));
        }

        while started.load(AtomicOrdering::Acquire) != WORKERS {
            std::thread::yield_now();
        }
        while finished.load(AtomicOrdering::Acquire) != WORKERS {
            let _extension_call = RuntimeExecutionGuard::enter();
            let saved = unsafe { molt_cpython_abi::api::object::PyEval_SaveThread() };
            std::thread::yield_now();
            unsafe { molt_cpython_abi::api::object::PyEval_RestoreThread(saved) };
        }

        {
            let _release = GilReleaseGuard::suspend();
            for worker in workers {
                worker.join().expect("worker panicked");
            }
        }
        stop_watchdog.store(true, AtomicOrdering::Release);
        watchdog.join().expect("watchdog panicked");

        assert_eq!(max_active.load(AtomicOrdering::Acquire), 1);
        assert_eq!(
            unsafe { *mutation.0.get() },
            WORKERS * ACQUISITIONS,
            "foreign-proxy mutation lost updates despite GIL custody"
        );
        crate::concurrency::gil::release_runtime_gil();
    }

    #[test]
    fn gil_ensure_scalar_custody_is_nested_and_exact() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        let baseline = molt_cpython_abi::api::object::runtime_execution_attachment_count();
        let retained_baseline =
            molt_cpython_abi::api::object::runtime_retained_thread_state_count();

        std::thread::spawn(move || {
            assert!(!gil_owned_by_current_thread());
            let outer = gil_ensure_unit();
            assert_eq!(outer, PY_GIL_STATE_UNLOCKED);
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_attachment_count(),
                baseline + 1,
                "outer Ensure must establish exactly one runtime attachment"
            );
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                retained_baseline + 1,
                "first-ever Ensure must create exactly one retained state"
            );
            let inner = gil_ensure_unit();
            assert_eq!(inner, PY_GIL_STATE_LOCKED);
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut PyExc_MemoryError).cast::<PyObject>(),
                    c"outer PyGILState custody".as_ptr(),
                );
            }
            gil_leave_unit(inner);
            assert!(gil_owned_by_current_thread());
            assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
            gil_leave_unit(outer);
            assert!(!gil_owned_by_current_thread());
            assert_eq!(
                molt_cpython_abi::api::object::runtime_execution_attachment_count(),
                baseline,
                "outer Release must detach exactly once"
            );
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                retained_baseline,
                "final Release must destroy state created by first-ever Ensure"
            );
        })
        .join()
        .unwrap();

        std::thread::spawn(move || {
            let ordinary = RuntimeExecutionGuard::enter();
            let preexisting = unsafe { molt_cpython_abi::api::object::PyThreadState_Get() };
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut PyExc_MemoryError).cast::<PyObject>(),
                    c"pre-existing runtime state".as_ptr(),
                );
            }
            drop(ordinary);

            let ensured = gil_ensure_unit();
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::PyThreadState_Get() },
                preexisting,
                "Ensure must reuse an ordinary pre-existing thread state"
            );
            gil_leave_unit(ensured);

            let observer = RuntimeExecutionGuard::enter();
            assert_eq!(
                unsafe { molt_cpython_abi::api::object::PyThreadState_Get() },
                preexisting
            );
            assert!(
                !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
                "final Release must preserve state it did not create"
            );
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            drop(observer);
            drop(RuntimeExecutionGuard::enter_with_worker_cleanup());
        })
        .join()
        .unwrap();

        assert!(
            crate::test_support::catch_expected_unwind(|| gil_leave_unit(PY_GIL_STATE_LOCKED))
                .is_err(),
            "unmatched PyGILState_Release must fail closed"
        );
    }

    #[test]
    fn cpython_thread_state_is_stable_attached_and_save_restore_exact() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
            1
        );
        assert_eq!(
            unsafe { hook_attached_runtime_context() },
            molt_cpython_abi::hooks::AttachedRuntimeContextKind::Detached as u32,
            "initialized main thread is not attached until an execution boundary says so"
        );

        let ensure_state = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        let expected_attached = if cfg!(feature = "free-threaded") {
            molt_cpython_abi::hooks::AttachedRuntimeContextKind::NativeFreeThreaded
        } else {
            molt_cpython_abi::hooks::AttachedRuntimeContextKind::NativeGil
        };
        assert_eq!(
            unsafe { hook_attached_runtime_context() },
            expected_attached as u32
        );
        let tstate = unsafe { molt_cpython_abi::api::object::PyThreadState_Get() };
        assert!(!tstate.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() },
            tstate
        );
        let tstate_id = unsafe { molt_cpython_abi::api::object::PyThreadState_GetID(tstate) };
        assert_ne!(tstate_id, 0);
        let interp = unsafe { molt_cpython_abi::api::object::PyThreadState_GetInterpreter(tstate) };
        assert!(!interp.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyInterpreterState_GetID(interp) },
            0
        );
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::object::PyInterpreterState_GetIDFromThreadState(tstate)
            },
            0
        );
        assert!(unsafe { molt_cpython_abi::api::object::PyThreadState_GetFrame(tstate) }.is_null());

        let saved = unsafe { molt_cpython_abi::api::object::PyEval_SaveThread() };
        assert_eq!(saved, tstate);
        assert!(unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        unsafe { molt_cpython_abi::api::object::PyEval_RestoreThread(saved) };
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyThreadState_Get() },
            tstate
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyThreadState_GetID(tstate) },
            tstate_id
        );
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(ensure_state) };
        assert!(unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        assert_eq!(
            unsafe { hook_attached_runtime_context() },
            molt_cpython_abi::hooks::AttachedRuntimeContextKind::Detached as u32
        );
    }

    #[test]
    fn pending_call_failure_ends_in_exactly_one_boundary_exception_domain() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        PENDING_CALL_TEST_CALLBACKS.store(0, AtomicOrdering::Relaxed);

        assert_eq!(
            unsafe { hook_attached_runtime_context() },
            molt_cpython_abi::hooks::AttachedRuntimeContextKind::Detached as u32
        );
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(pending_call_test_noop),
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            molt_cpython_abi::api::pending_calls::Py_MakePendingCalls(),
            -1
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>()
        );
        assert_eq!(unsafe { hook_exception_pending() }, 0);
        assert_eq!(PENDING_CALL_TEST_CALLBACKS.load(AtomicOrdering::Relaxed), 0);
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        assert_eq!(
            unsafe { hook_exception_pending() },
            0,
            "clearing the direct C boundary must not reveal stale runtime residue"
        );

        let attachment = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        assert_eq!(
            molt_cpython_abi::api::pending_calls::Py_MakePendingCalls(),
            0
        );
        assert_eq!(PENDING_CALL_TEST_CALLBACKS.load(AtomicOrdering::Relaxed), 1);

        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(pending_call_test_type_error),
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending(),
            1,
            "drain rc must branch while the C TypeError moves into runtime custody"
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "the runtime safepoint must consume the C indicator"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "observing the pending type must not create a C-owned error"
        );
        assert_eq!(pending_exception_type_for_assertion(), "TypeError");
        assert_eq!(
            pending_exception_message_for_assertion(),
            "exact pending-call callback TypeError"
        );

        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(pending_call_test_runtime_type_error),
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            crate::builtins::exceptions::molt_async_work_poll_and_exception_pending(),
            1
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "a runtime-originated TypeError must stay in runtime custody"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "observing the pending type must not create a C-owned error"
        );
        assert_eq!(pending_exception_type_for_assertion(), "TypeError");
        assert_eq!(
            pending_exception_message_for_assertion(),
            "exact runtime pending-call TypeError"
        );
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(attachment) };
    }

    #[test]
    fn fused_finally_poll_services_callback_and_returns_owned_pending_exception() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        PENDING_CALL_TEST_CALLBACKS.store(0, AtomicOrdering::Relaxed);
        let attachment = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };

        assert_eq!(
            unsafe {
                molt_cpython_abi::api::pending_calls::Py_AddPendingCall(
                    Some(pending_call_test_runtime_type_error),
                    ptr::null_mut(),
                )
            },
            0
        );
        let exc_bits =
            crate::builtins::exceptions::molt_async_work_poll_and_exception_last_pending();
        assert_eq!(PENDING_CALL_TEST_CALLBACKS.load(AtomicOrdering::Relaxed), 1);
        assert!(
            !MoltObject::from_bits(exc_bits).is_none(),
            "the fused observer must return the callback exception object"
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "the runtime safepoint must consume the C exception indicator"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
        );
        assert!(
            molt_cpython_abi::api::errors::take_current_error().is_none(),
            "observing the pending type must not create a C-owned error"
        );

        with_gil(|_py| {
            assert!(crate::exception_pending(&_py));
            let exc_ptr = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .expect("fused observer returned a heap exception");
            assert_eq!(
                crate::format_exception_message(&_py, exc_ptr),
                "exact runtime pending-call TypeError"
            );

            crate::clear_exception(&_py);
            assert!(
                !crate::exception_pending(&_py),
                "finally arbitration owns whether the pending slot is cleared"
            );
            assert_eq!(
                crate::format_exception_message(&_py, exc_ptr),
                "exact runtime pending-call TypeError",
                "the fused result must own the exception independently of the pending slot"
            );
            crate::dec_ref_bits(&_py, exc_bits);
        });
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(attachment) };
    }

    #[test]
    fn cpython_gilstate_restores_preexisting_internal_custody_without_attachment_leak() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let internal = GilGuard::new();
        assert!(unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        let outer = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        let inner = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
        assert_eq!(outer, PY_GIL_STATE_LOCKED);
        assert_eq!(inner, PY_GIL_STATE_LOCKED);
        assert!(!unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(inner) };
        assert!(!unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        unsafe { molt_cpython_abi::api::object::PyGILState_Release(outer) };
        assert!(unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
        assert!(gil_owned_by_current_thread());
        drop(internal);
    }

    #[test]
    fn cpython_thread_state_metadata_accepts_live_foreign_state_but_not_custody() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let release = Arc::new(std::sync::Barrier::new(2));
        let worker_release = Arc::clone(&release);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let state = unsafe { molt_cpython_abi::api::object::PyGILState_Ensure() };
            let tstate = unsafe { molt_cpython_abi::api::object::PyThreadState_Get() };
            let id = unsafe { molt_cpython_abi::api::object::PyThreadState_GetID(tstate) };
            tx.send((tstate as usize, id)).unwrap();
            worker_release.wait();
            unsafe { molt_cpython_abi::api::object::PyGILState_Release(state) };
        });

        let (foreign, id) = rx.recv().unwrap();
        let foreign = foreign as *mut molt_cpython_abi::abi_types::PyThreadState;
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyThreadState_GetID(foreign) },
            id
        );
        let interp =
            unsafe { molt_cpython_abi::api::object::PyThreadState_GetInterpreter(foreign) };
        assert_eq!(
            unsafe { molt_cpython_abi::api::object::PyInterpreterState_GetID(interp) },
            0
        );
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::object::PyInterpreterState_GetIDFromThreadState(foreign)
            },
            0
        );
        assert!(
            unsafe { molt_cpython_abi::api::object::PyThreadState_GetFrame(foreign) }.is_null()
        );
        release.wait();
        worker.join().unwrap();
    }

    #[test]
    fn save_thread_single_slot_rejects_nested_and_unmatched_transitions() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(
            crate::test_support::catch_expected_unwind(gil_save_thread).is_err(),
            "PyEval_SaveThread without GIL custody must fail closed"
        );
        let guard = GilGuard::new();
        gil_save_thread();
        assert!(!gil_owned_by_current_thread());
        assert!(
            crate::test_support::catch_expected_unwind(gil_save_thread).is_err(),
            "nested PyEval_SaveThread must fail before changing custody"
        );
        gil_restore_thread();
        assert!(gil_owned_by_current_thread());
        assert!(
            crate::test_support::catch_expected_unwind(gil_restore_thread).is_err(),
            "unmatched PyEval_RestoreThread must fail closed"
        );
        drop(guard);
    }

    #[cfg(feature = "l7-attestation-probe")]
    #[test]
    fn cpython_abi_gil_custody_is_allocation_free_after_warmup() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        for _ in 0..64 {
            let state = gil_ensure_unit();
            gil_leave_unit(state);
            let guard = GilGuard::new();
            gil_save_thread();
            gil_restore_thread();
            drop(guard);
        }

        crate::attestation_probe::reset();
        crate::attestation_probe::set_tracking(true);
        for _ in 0..10_000 {
            let state = gil_ensure_unit();
            gil_leave_unit(state);
            let guard = GilGuard::new();
            gil_save_thread();
            gil_restore_thread();
            drop(guard);
        }
        crate::attestation_probe::set_tracking(false);

        let observed = crate::attestation_probe::snapshot();
        assert_eq!(observed.allocations, 0, "{observed:?}");
        assert_eq!(observed.allocated_bytes, 0, "{observed:?}");
    }

    unsafe extern "C" fn gil_bench_noargs(
        _self: *mut PyObject,
        _args: *mut PyObject,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(
                &raw mut molt_cpython_abi::abi_types::Py_None,
            )
        };
        &raw mut molt_cpython_abi::abi_types::Py_None
    }

    #[cfg(any(windows, target_os = "linux"))]
    type CExtBenchmarkCall = extern "C" fn(u64, u64, u64) -> i64;

    #[cfg(windows)]
    fn configure_cext_benchmark_process() -> serde_json::Value {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetPriorityClass, GetProcessAffinityMask, SetPriorityClass,
            SetProcessAffinityMask,
        };

        let logical_cpu = std::env::var("MOLT_CEXT_BENCH_TARGET_CPU")
            .expect("benchmark orchestrator must select a child logical CPU")
            .parse::<u32>()
            .expect("MOLT_CEXT_BENCH_TARGET_CPU must be a u32");
        let expected_mask = std::env::var("MOLT_CEXT_BENCH_TARGET_MASK")
            .expect("benchmark orchestrator must select a child affinity mask")
            .parse::<usize>()
            .expect("MOLT_CEXT_BENCH_TARGET_MASK must be a usize");
        let expected_priority = std::env::var("MOLT_CEXT_BENCH_EXPECTED_PRIORITY")
            .expect("benchmark orchestrator must select a child priority")
            .parse::<u32>()
            .expect("MOLT_CEXT_BENCH_EXPECTED_PRIORITY must be a u32");
        let process = unsafe { GetCurrentProcess() };
        assert_ne!(
            unsafe { SetProcessAffinityMask(process, expected_mask) },
            0,
            "timed child could not acquire its isolated logical CPU"
        );
        assert_ne!(
            unsafe { SetPriorityClass(process, expected_priority) },
            0,
            "timed child could not acquire its requested priority class"
        );
        let mut actual_mask = 0_usize;
        let mut system_mask = 0_usize;
        assert_ne!(
            unsafe { GetProcessAffinityMask(process, &mut actual_mask, &mut system_mask) },
            0,
            "timed child could not query its process affinity"
        );
        let actual_priority = unsafe { GetPriorityClass(process) };
        assert_eq!(actual_mask, expected_mask, "timed child affinity drifted");
        assert_eq!(
            actual_priority, expected_priority,
            "timed child priority class drifted"
        );
        assert_eq!(
            actual_mask.count_ones(),
            1,
            "timed child must own exactly one logical CPU"
        );
        assert_eq!(
            actual_mask.trailing_zeros(),
            logical_cpu,
            "timed child acquired the wrong logical CPU"
        );
        serde_json::json!({
            "pid": std::process::id(),
            "platform": "windows",
            "logical_cpu": logical_cpu,
            "affinity_mask": actual_mask,
            "system_affinity_mask": system_mask,
            "priority_class": actual_priority,
            "verified_before_warmup": true,
        })
    }

    #[cfg(target_os = "linux")]
    fn configure_cext_benchmark_process() -> serde_json::Value {
        let logical_cpu = std::env::var("MOLT_CEXT_BENCH_TARGET_CPU")
            .expect("benchmark orchestrator must select a child logical CPU")
            .parse::<usize>()
            .expect("MOLT_CEXT_BENCH_TARGET_CPU must be a usize");
        let expected_nice = std::env::var("MOLT_CEXT_BENCH_EXPECTED_PRIORITY")
            .expect("benchmark orchestrator must name the inherited child nice value")
            .parse::<i32>()
            .expect("MOLT_CEXT_BENCH_EXPECTED_PRIORITY must be an i32");
        let mut requested = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
        unsafe {
            libc::CPU_ZERO(&mut requested);
            libc::CPU_SET(logical_cpu, &mut requested);
        }
        assert_eq!(
            unsafe {
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &requested)
            },
            0,
            "timed child could not acquire its isolated logical CPU"
        );
        let mut actual = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
        assert_eq!(
            unsafe {
                libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut actual)
            },
            0,
            "timed child could not query its process affinity"
        );
        let actual_cpus: Vec<usize> = (0..libc::CPU_SETSIZE as usize)
            .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &actual) })
            .collect();
        assert_eq!(
            actual_cpus,
            vec![logical_cpu],
            "timed child affinity drifted"
        );
        let actual_nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
        assert_eq!(
            actual_nice, expected_nice,
            "timed child inherited an unexpected priority"
        );
        serde_json::json!({
            "pid": std::process::id(),
            "platform": "linux",
            "logical_cpu": logical_cpu,
            "affinity_cpus": actual_cpus,
            "nice": actual_nice,
            "verified_before_warmup": true,
        })
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[inline(never)]
    fn measure_cext_call(call: CExtBenchmarkCall, closure_bits: u64, iterations: usize) -> f64 {
        let call = std::hint::black_box(call);
        let start = Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(call(closure_bits, 0, 0));
        }
        start.elapsed().as_nanos() as f64 / iterations as f64
    }

    fn owned_noargs_test_closure() -> u64 {
        let name = b"closure_lifetime_test";
        let function = unsafe {
            hook_register_c_function(
                gil_bench_noargs as *const () as usize as u64,
                METH_NOARGS,
                MoltObject::none().bits(),
                true,
                MoltObject::none().bits(),
                name.as_ptr(),
                name.len(),
            )
        };
        assert_ne!(function, 0);
        with_gil(|py| unsafe {
            let closure = crate::object::layout::function_closure_bits(
                MoltObject::from_bits(function).as_ptr().unwrap(),
            );
            inc_ref_bits(&py, closure);
            dec_ref_bits(&py, function);
            closure
        })
    }

    #[test]
    fn cext_qualnames_use_native_classes_and_live_receiver_types() {
        use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyType_Type};
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let qualname = crate::attr_name_bits_from_bytes(&py, b"__qualname__").unwrap();
            let register_and_check = |receiver, self_is_null, expected: &str| {
                let function = hook_register_c_function(
                    gil_bench_noargs as *const () as usize as u64,
                    METH_NOARGS,
                    receiver,
                    self_is_null,
                    MoltObject::none().bits(),
                    b"method".as_ptr(),
                    b"method".len(),
                );
                assert_ne!(function, 0);
                let name = crate::attr_lookup_ptr(
                    &py,
                    MoltObject::from_bits(function).as_ptr().unwrap(),
                    qualname,
                )
                .expect("public native callable qualified name");
                assert_eq!(
                    crate::string_obj_to_owned(MoltObject::from_bits(name)).as_deref(),
                    Some(expected)
                );
                dec_ref_bits(&py, name);
                dec_ref_bits(&py, function);
            };
            let mut first: PyTypeObject = std::mem::zeroed();
            first.ob_base.ob_base.ob_refcnt = 1;
            first.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            first.tp_base = &raw mut PyBaseObject_Type;
            first.tp_name = c"native_fixture.CallableFirst".as_ptr();
            let mut second: PyTypeObject = std::mem::zeroed();
            second.ob_base.ob_base.ob_refcnt = 1;
            second.ob_base.ob_base.ob_type = &raw mut PyType_Type;
            second.tp_base = &raw mut PyBaseObject_Type;
            second.tp_name = c"native_fixture.CallableSecond".as_ptr();
            let mut value = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut first,
            };
            let class = GLOBAL_BRIDGE
                .molt_value_for_pyobj((&raw mut first).cast())
                .unwrap();
            let instance = GLOBAL_BRIDGE.molt_value_for_pyobj(&raw mut value).unwrap();
            register_and_check(class, false, "CallableFirst.method");
            register_and_check(instance, false, "CallableFirst.method");
            value.ob_type = &raw mut second;
            register_and_check(instance, false, "CallableSecond.method");

            let module_name = MoltObject::from_ptr(alloc_string(&py, b"callable_module")).bits();
            let module = alloc_module_obj(&py, module_name);
            assert!(!module.is_null());
            register_and_check(MoltObject::from_ptr(module).bits(), false, "method");
            register_and_check(MoltObject::none().bits(), true, "method");
            assert!(!crate::exception_pending(&py));
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            for bits in [
                MoltObject::from_ptr(module).bits(),
                module_name,
                instance,
                class,
                qualname,
            ] {
                dec_ref_bits(&py, bits);
            }
            assert_eq!(value.ob_refcnt, 1);
            assert_eq!(first.ob_base.ob_base.ob_refcnt, 1);
            assert_eq!(second.ob_base.ob_base.ob_refcnt, 1);
        });
    }

    #[test]
    fn cext_receiver_is_owned_by_the_traced_function_closure() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        with_gil(|py| unsafe {
            let receiver = alloc_dict_with_pairs(&py, &[]);
            assert!(!receiver.is_null());
            let receiver_bits = MoltObject::from_ptr(receiver).bits();
            let baseline = (*header_from_obj_ptr(receiver)).ref_count_snapshot();
            let function = hook_register_c_function(
                gil_bench_noargs as *const () as usize as u64,
                METH_NOARGS,
                receiver_bits,
                false,
                MoltObject::none().bits(),
                b"receiver_owner".as_ptr(),
                b"receiver_owner".len(),
            );
            assert_ne!(function, 0);
            let function_ptr = MoltObject::from_bits(function).as_ptr().unwrap();
            assert_eq!(
                crate::object_class_bits(function_ptr),
                crate::builtin_classes(&py).builtin_function_or_method,
                "C-extension callables must publish builtin function identity"
            );
            let closure = crate::object::layout::function_closure_bits(function_ptr);
            let closure_ptr = MoltObject::from_bits(closure).as_ptr().unwrap();
            let context = CExtCallableContext::from_bits(closure).unwrap();
            assert_eq!(context.receiver, receiver_bits);
            assert_eq!(context.defining_class, MoltObject::none().bits());
            assert_eq!(
                (*header_from_obj_ptr(receiver)).ref_count_snapshot(),
                baseline + 1
            );
            let mut receiver_edges = 0;
            crate::object::heap_lifecycle::visit_owned_edges(&py, closure_ptr, &mut |child| {
                receiver_edges += usize::from(child == receiver);
            });
            assert_eq!(
                receiver_edges, 1,
                "cycle GC must see the receiver ownership edge"
            );
            dec_ref_bits(&py, function);
            assert_eq!(
                (*header_from_obj_ptr(receiver)).ref_count_snapshot(),
                baseline
            );
            dec_ref_bits(&py, receiver_bits);
        });
    }

    #[test]
    fn cext_runtime_restart_rebinds_static_types_and_rebuilds_fields() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            for cycle in 0..3 {
                let _execution = RuntimeExecutionGuard::enter();
                let py = _execution.token();
                let runtime = crate::runtime_state(&py);
                assert_eq!(
                    *runtime.cpython.registration.lock().unwrap(),
                    HookRegistrationState::Ready
                );
                let bindings = runtime.cpython.static_bindings.lock().unwrap().clone();
                for binding in bindings {
                    let observed = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(binding.pointer)
                        .expect("every owned static shell must remain bound")
                        .bits();
                    assert_eq!(observed, binding.bits);
                }
                unsafe {
                    super::native_lifecycle_tests::assert_c_exception_symbols_preserve_pending();
                }
                let type_error = (&raw mut PyExc_TypeError).cast::<PyObject>();
                let group_bits = crate::builtin_classes(&py).exception_group;
                unsafe {
                    let view = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .handle_to_borrowed_pyobj(group_bits)
                        .cast::<PyTypeObject>();
                    let heap = view.cast::<molt_cpython_abi::abi_types::PyHeapTypeObject>();
                    assert_ne!(
                        (*view).tp_flags & molt_cpython_abi::abi_types::Py_TPFLAGS_HEAPTYPE,
                        0
                    );
                    assert_eq!(
                        (*view).tp_flags & molt_cpython_abi::abi_types::Py_TPFLAGS_IMMUTABLETYPE,
                        0
                    );
                    assert!(!(*heap).ht_name.is_null() && !(*heap).ht_qualname.is_null());
                    assert_eq!(
                        std::ffi::CStr::from_ptr((*view).tp_name).to_bytes(),
                        b"ExceptionGroup"
                    );
                    let renamed =
                        crate::attr_name_bits_from_bytes(&py, b"RenamedExceptionGroup").unwrap();
                    assert!(crate::class_set_name_bits(
                        &py,
                        crate::obj_from_bits(group_bits).as_ptr().unwrap(),
                        renamed
                    ));
                    crate::dec_ref_bits(&py, renamed);
                    assert_eq!(
                        std::ffi::CStr::from_ptr((*view).tp_name).to_bytes(),
                        b"RenamedExceptionGroup"
                    );
                }
                // A non-static canonical type has a generic managed ABI view,
                // not a process-shell binding. Even direct C references to
                // that view have interpreter lifetime and must not survive
                // retirement as identities in the next runtime.
                let descriptor_class = crate::builtins::types::member_descriptor_class(&py);
                let descriptor_view = unsafe {
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .handle_to_borrowed_pyobj(descriptor_class)
                };
                assert!(!descriptor_view.is_null());
                assert_eq!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .managed_handle_for_pyobj(descriptor_view),
                    Some(descriptor_class)
                );
                unsafe { molt_cpython_abi::api::refcount::Py_INCREF(descriptor_view) };
                assert!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(descriptor_class)
                );
                // Exercise the actual type -> MRO -> type cycle, plus an MRO
                // alias held by the C thread-state dictionary. That owner must
                // drain before forced cohort retirement can free any C view.
                let (descriptor_mro, mro_alias) = unsafe {
                    let mro = (*descriptor_view.cast::<PyTypeObject>()).tp_mro;
                    assert!(!mro.is_null());
                    assert_eq!(
                        molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, 0),
                        descriptor_view
                    );
                    molt_cpython_abi::api::refcount::Py_INCREF(mro);
                    let alias = molt_cpython_abi::api::sequences::PyTuple_New(1);
                    assert!(!alias.is_null());
                    assert_eq!(
                        molt_cpython_abi::api::sequences::PyTuple_SetItem(alias, 0, mro),
                        0
                    );
                    let state_dict = molt_cpython_abi::api::sys::PyThreadState_GetDict();
                    assert!(!state_dict.is_null());
                    assert_eq!(
                        molt_cpython_abi::api::mapping::PyDict_SetItemString(
                            state_dict,
                            c"__restart_mro_owner__".as_ptr(),
                            alias,
                        ),
                        0
                    );
                    // Locals retain only opaque addresses. No external owned
                    // pointer or Rust guard may decref after finalization.
                    molt_cpython_abi::api::refcount::Py_DECREF(alias);
                    molt_cpython_abi::api::refcount::Py_DECREF(descriptor_view);
                    (mro, alias)
                };
                assert!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .mirrored_c_refcount(descriptor_view.addr())
                        > 0
                );
                assert_eq!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(type_error)
                        .unwrap()
                        .bits(),
                    crate::exception_type_bits_from_name(&py, "TypeError"),
                );
                unsafe {
                    let shell = type_error.cast::<PyTypeObject>();
                    assert_ne!(
                        (*shell).tp_flags & molt_cpython_abi::abi_types::Py_TPFLAGS_READY,
                        0
                    );
                    assert!(!(*shell).tp_dict.is_null());
                    assert!(!(*shell).tp_mro.is_null());
                    assert!(
                        molt_cpython_abi::api::mapping::PyDict_GetItemString(
                            (*shell).tp_dict,
                            c"__restart_sentinel__".as_ptr(),
                        )
                        .is_null(),
                        "retired type dictionary survived into cycle {cycle}"
                    );
                    assert_eq!(
                        molt_cpython_abi::api::mapping::PyDict_SetItemString(
                            (*shell).tp_dict,
                            c"__restart_sentinel__".as_ptr(),
                            (&raw mut molt_cpython_abi::abi_types::Py_None).cast(),
                        ),
                        0
                    );
                    molt_cpython_abi::api::errors::PyErr_SetString(
                        type_error,
                        c"restart exception identity".as_ptr(),
                    );
                    let (mut error_type, mut value, mut traceback) =
                        (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                    molt_cpython_abi::api::errors::PyErr_Fetch(
                        &mut error_type,
                        &mut value,
                        &mut traceback,
                    );
                    molt_cpython_abi::api::errors::PyErr_NormalizeException(
                        &mut error_type,
                        &mut value,
                        &mut traceback,
                    );
                    let error_type_owner = OwnedPyObject::from_owned(error_type);
                    let value_owner = OwnedPyObject::from_owned(value);
                    let traceback_owner = OwnedPyObject::from_owned(traceback);
                    assert_eq!(error_type, type_error);
                    assert!(!value.is_null());
                    drop(error_type_owner);
                    drop(value_owner);
                    drop(traceback_owner);
                }
                assert!(!crate::exception_pending(&py));
                drop(_execution);
                assert_eq!(crate::state::runtime_state::molt_runtime_shutdown(), 1);
                // Query the opaque address map only: dereferencing/decrefing
                // an interpreter-owned pointer after finalization is invalid.
                for pointer in [descriptor_view, descriptor_mro, mro_alias] {
                    assert!(
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .managed_handle_for_pyobj(pointer)
                            .is_none()
                    );
                    assert_eq!(
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE.mirrored_c_refcount(pointer.addr()),
                        0
                    );
                }
                assert_eq!(
                    molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                    0
                );
                assert!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(type_error)
                        .is_none()
                );
                if cycle != 2 {
                    crate::state::runtime_state::molt_runtime_reset_for_testing();
                    assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
                }
            }
        });
    }

    #[test]
    fn checked_extension_trampoline_owns_cold_external_ingress() {
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let lease_baseline = crate::state::runtime_state::active_runtime_execution_lease_count();
        let closure_bits = owned_noargs_test_closure();

        let result = std::thread::spawn(move || {
            assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
            assert!(!molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
            let result = molt_cpython_abi_cext_call_trampoline(closure_bits, 0, 0);
            assert!(!crate::state::runtime_state::current_thread_holds_runtime_execution_lease());
            assert!(!molt_cpython_abi::api::object::runtime_execution_thread_is_attached());
            result
        })
        .join()
        .unwrap();

        assert!(MoltObject::from_bits(result as u64).is_none());
        with_gil(|py| dec_ref_bits(&py, closure_bits));
        assert_eq!(
            crate::state::runtime_state::active_runtime_execution_lease_count(),
            lease_baseline,
            "checked external ingress leaked lifecycle admission"
        );
    }

    #[test]
    fn shutdown_owned_drain_reenters_c_extension_with_pending_dict_and_context_edges() {
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            register_cpython_hooks();
            SHUTDOWN_DRAIN_CEXT_CLOSURE_BITS
                .store(owned_noargs_test_closure(), AtomicOrdering::Release);
            SHUTDOWN_DRAIN_CEXT_CALLBACKS.store(0, AtomicOrdering::Release);

            {
                let _execution = RuntimeExecutionGuard::enter();
                let dict = unsafe { molt_cpython_abi::api::sys::PyThreadState_GetDict() };
                assert!(
                    !dict.is_null(),
                    "shutdown proof requires a retained state dict"
                );
                let var = unsafe {
                    molt_cpython_abi::api::contextvars::PyContextVar_New(
                        c"shutdown-drain".as_ptr(),
                        ptr::null_mut(),
                    )
                };
                assert!(!var.is_null());
                let token = unsafe {
                    molt_cpython_abi::api::contextvars::PyContextVar_Set(
                        var,
                        (&raw mut molt_cpython_abi::abi_types::Py_None).cast::<PyObject>(),
                    )
                };
                assert!(!token.is_null(), "shutdown proof requires a context edge");
                unsafe {
                    molt_cpython_abi::api::refcount::Py_DECREF(token);
                    molt_cpython_abi::api::refcount::Py_DECREF(var);
                }
                unsafe {
                    molt_cpython_abi::api::errors::PyErr_SetString(
                        (&raw mut PyExc_MemoryError).cast::<PyObject>(),
                        c"shutdown-owned pending error".as_ptr(),
                    );
                }
            }
            assert!(
                molt_cpython_abi::api::object::current_thread_has_retained_runtime_state(),
                "managed edge proof did not retain the shutdown owner's state"
            );
            molt_cpython_abi::api::object::set_thread_state_drain_reentry_test_hook(Some(
                shutdown_drain_cext_reentry,
            ));
            assert_eq!(crate::state::runtime_state::molt_runtime_shutdown(), 1);
            assert_eq!(
                SHUTDOWN_DRAIN_CEXT_CALLBACKS.load(AtomicOrdering::Acquire),
                1,
                "shutdown-owned managed-edge drain must reenter exactly once"
            );
            assert_eq!(
                molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
                0
            );
        });
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    #[ignore = "fresh-process release benchmark sample; orchestrated by tools/benchmark_cext_trampoline.py"]
    fn single_thread_extension_call_preemption_bench() {
        let process_execution_contract = configure_cext_benchmark_process();
        let _test_guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let closure_bits = owned_noargs_test_closure();
        let baseline_call: CExtBenchmarkCall =
            std::hint::black_box(molt_cpython_abi_cext_call_trampoline_baseline);
        let checked_call: CExtBenchmarkCall =
            std::hint::black_box(molt_cpython_abi_cext_call_trampoline);
        let admitted_call: CExtBenchmarkCall =
            std::hint::black_box(molt_cpython_abi_cext_call_trampoline_admitted);
        let iterations = std::env::var("MOLT_CEXT_BENCH_ITERATIONS")
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .filter(|&value| value >= 100_000)
            .unwrap_or(1_000_000);
        let order =
            std::env::var("MOLT_CEXT_BENCH_ORDER").unwrap_or_else(|_| "baseline-first".to_string());
        let candidate_name =
            std::env::var("MOLT_CEXT_BENCH_CANDIDATE").unwrap_or_else(|_| "admitted".to_string());
        assert!(
            matches!(order.as_str(), "baseline-first" | "candidate-first"),
            "MOLT_CEXT_BENCH_ORDER must be baseline-first or candidate-first"
        );
        assert!(
            matches!(candidate_name.as_str(), "admitted" | "checked-nested"),
            "MOLT_CEXT_BENCH_CANDIDATE must be admitted or checked-nested"
        );
        let warmup_iterations = (iterations / 20).max(10_000);
        let _ = measure_cext_call(baseline_call, closure_bits, warmup_iterations);
        let _ = measure_cext_call(checked_call, closure_bits, warmup_iterations);
        let cold_checked_ns = measure_cext_call(checked_call, closure_bits, iterations);
        crate::concurrency::ensure_persistent_runtime_execution();
        let candidate_call = if candidate_name == "admitted" {
            admitted_call
        } else {
            checked_call
        };
        let _ = measure_cext_call(candidate_call, closure_bits, warmup_iterations);
        const PAIR_ROUNDS: usize = 120;
        let round_iterations = iterations / PAIR_ROUNDS;
        let measured_iterations = round_iterations * PAIR_ROUNDS;
        let mut baseline_rounds = Vec::with_capacity(PAIR_ROUNDS);
        let mut candidate_rounds = Vec::with_capacity(PAIR_ROUNDS);
        for round in 0..PAIR_ROUNDS {
            let baseline_pattern = matches!(round % 4, 0 | 3);
            let baseline_first = (order == "baseline-first") == baseline_pattern;
            if baseline_first {
                baseline_rounds.push(measure_cext_call(
                    baseline_call,
                    closure_bits,
                    round_iterations,
                ));
                candidate_rounds.push(measure_cext_call(
                    candidate_call,
                    closure_bits,
                    round_iterations,
                ));
            } else {
                candidate_rounds.push(measure_cext_call(
                    candidate_call,
                    closure_bits,
                    round_iterations,
                ));
                baseline_rounds.push(measure_cext_call(
                    baseline_call,
                    closure_bits,
                    round_iterations,
                ));
            }
        }
        let baseline_ns = baseline_rounds.iter().sum::<f64>() / PAIR_ROUNDS as f64;
        let candidate_ns = candidate_rounds.iter().sum::<f64>() / PAIR_ROUNDS as f64;

        #[cfg(feature = "l7-attestation-probe")]
        let (
            baseline_allocations,
            baseline_allocated_bytes,
            candidate_allocations,
            candidate_allocated_bytes,
        ) = {
            let allocation_iterations = 10_000;
            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..allocation_iterations {
                std::hint::black_box(baseline_call(closure_bits, 0, 0));
            }
            crate::attestation_probe::set_tracking(false);
            let baseline = crate::attestation_probe::snapshot();

            crate::attestation_probe::reset();
            crate::attestation_probe::set_tracking(true);
            for _ in 0..allocation_iterations {
                std::hint::black_box(candidate_call(closure_bits, 0, 0));
            }
            crate::attestation_probe::set_tracking(false);
            let candidate = crate::attestation_probe::snapshot();
            (
                baseline.allocations,
                baseline.allocated_bytes,
                candidate.allocations,
                candidate.allocated_bytes,
            )
        };
        #[cfg(not(feature = "l7-attestation-probe"))]
        let (
            baseline_allocations,
            baseline_allocated_bytes,
            candidate_allocations,
            candidate_allocated_bytes,
        ) = (0_u64, 0_u64, 0_u64, 0_u64);

        assert!(crate::concurrency::release_persistent_runtime_execution());
        let sample = serde_json::json!({
            "candidate": candidate_name,
            "order": order,
            "pair_order_pattern": "ABBA+BAAB",
            "iterations": measured_iterations,
            "pair_rounds": PAIR_ROUNDS,
            "round_iterations": round_iterations,
            "baseline_rounds_ns_per_call": baseline_rounds,
            "candidate_rounds_ns_per_call": candidate_rounds,
            "baseline_ns_per_call": baseline_ns,
            "candidate_ns_per_call": candidate_ns,
            "cold_checked_ns_per_call": cold_checked_ns,
            "allocation_iterations": 10_000,
            "baseline_allocations": baseline_allocations,
            "baseline_allocated_bytes": baseline_allocated_bytes,
            "candidate_allocations": candidate_allocations,
            "candidate_allocated_bytes": candidate_allocated_bytes,
            "allocation_probe_enabled": cfg!(feature = "l7-attestation-probe"),
            "process_execution_contract": process_execution_contract,
        });
        with_gil(|py| dec_ref_bits(&py, closure_bits));
        println!("MOLT_CEXT_BENCH_SAMPLE {sample}");
    }

    fn pending_exception_message_for_assertion() -> String {
        with_gil(|_py| {
            let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
            if MoltObject::from_bits(exc_bits).is_none() {
                return "no pending exception".to_string();
            }
            let message = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .map(|exc_ptr| crate::format_exception_message(&_py, exc_ptr))
                .unwrap_or_else(|| "pending exception handle was not a heap object".to_string());
            crate::clear_exception(&_py);
            dec_ref_bits(&_py, exc_bits);
            message
        })
    }

    pub(super) fn pending_exception_type_for_assertion() -> String {
        with_gil(|_py| {
            let exc_bits = crate::builtins::exceptions::molt_exception_last_pending();
            let type_name = MoltObject::from_bits(exc_bits)
                .as_ptr()
                .and_then(|exc_ptr| {
                    let class_bits = unsafe { crate::object_class_bits(exc_ptr) };
                    MoltObject::from_bits(class_bits)
                        .as_ptr()
                        .and_then(|class_ptr| {
                            let name_bits = unsafe { crate::class_name_bits(class_ptr) };
                            crate::string_obj_to_owned(MoltObject::from_bits(name_bits))
                        })
                })
                .unwrap_or_else(|| "<unknown>".to_string());
            dec_ref_bits(&_py, exc_bits);
            type_name
        })
    }

    static RAW_GETATTRO_CALLS: AtomicUsize = AtomicUsize::new(0);
    static mut RAW_GETATTRO_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: std::ptr::null_mut(),
    };

    unsafe extern "C" fn raw_type_getattro(
        _obj: *mut PyObject,
        _name: *mut PyObject,
    ) -> *mut PyObject {
        RAW_GETATTRO_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        &raw mut RAW_GETATTRO_RESULT
    }

    #[test]
    fn raw_c_object_getattr_bypasses_molt_value_dispatch() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|_py| crate::clear_exception(&_py));
        RAW_GETATTRO_CALLS.store(0, AtomicOrdering::SeqCst);

        let mut raw_type: PyTypeObject = unsafe { std::mem::zeroed() };
        raw_type.tp_name = c"numpy.ndarray".as_ptr();
        raw_type.tp_getattro = Some(raw_type_getattro);
        let mut raw_obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut raw_type,
        };
        let raw_ptr = &raw mut raw_obj;
        let wrapper = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_value_for_pyobj(raw_ptr)
                .expect("foreign crossing must acquire one runtime owner")
        };

        let result = unsafe {
            molt_cpython_abi::api::object::PyObject_GetAttrString(
                raw_ptr,
                c"__array_finalize__".as_ptr(),
            )
        };

        assert_eq!(result, &raw mut RAW_GETATTRO_RESULT);
        assert_eq!(RAW_GETATTRO_CALLS.load(AtomicOrdering::SeqCst), 1);
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "raw C identity handles must not be decoded as Molt floats before tp_getattro"
        );
        with_gil(|py| dec_ref_bits(&py, wrapper));
        assert_eq!(raw_obj.ob_refcnt, 1);
    }

    #[test]
    fn native_constructor_ownership_is_acquired_only_at_runtime_crossing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let capsule = molt_cpython_abi::api::capsule::PyCapsule_New(
                (&raw mut RAW_GETATTRO_RESULT).cast(),
                ptr::null(),
                None,
            );
            let mut getset = molt_cpython_abi::abi_types::PyGetSetDef {
                name: c"ownership_getset".as_ptr(),
                get: None,
                set: None,
                doc: ptr::null(),
                closure: ptr::null_mut(),
            };
            let mut member = molt_cpython_abi::abi_types::PyMemberDef {
                name: c"ownership_member".as_ptr(),
                type_: 1,
                offset: 0,
                flags: 0,
                doc: ptr::null(),
            };
            let owner = &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type;
            let getset = molt_cpython_abi::api::typeobj::PyDescr_NewGetSet(owner, &raw mut getset);
            let member = molt_cpython_abi::api::typeobj::PyDescr_NewMember(owner, &raw mut member);
            for object in [capsule, getset, member] {
                assert!(!object.is_null());
                assert_eq!(
                    (*object).ob_refcnt,
                    1,
                    "constructor leaked a runtime wrapper"
                );
                let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                let first = bridge.molt_value_for_pyobj(object).expect("first crossing");
                let second = bridge
                    .molt_value_for_pyobj(object)
                    .expect("second crossing");
                assert_eq!(first, second, "one canonical foreign identity per C object");
                assert_eq!(
                    (*object).ob_refcnt,
                    2,
                    "one C hold per wrapper, not per crossing"
                );
                assert_eq!(
                    object_type_id(crate::obj_from_bits(first).as_ptr().unwrap()),
                    crate::TYPE_ID_FOREIGN
                );
                dec_ref_bits(&py, first);
                assert_eq!((*object).ob_refcnt, 2);
                dec_ref_bits(&py, second);
                assert_eq!(
                    (*object).ob_refcnt,
                    1,
                    "last runtime owner must release its C hold"
                );
                molt_cpython_abi::api::refcount::Py_DECREF(object);
            }
            assert!(!crate::exception_pending(&py));
        });
    }

    #[test]
    fn rejected_foreign_crossing_preserves_error_without_publishing_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let mut native_type: PyTypeObject = std::mem::zeroed();
            native_type.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_HAVE_GC;
            let mut object = PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut native_type,
            };
            let pointer = &raw mut object;
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            for _ in 0..2 {
                assert!(bridge.molt_value_for_pyobj(pointer).is_none());
                assert_eq!(object.ob_refcnt, 1);
                assert!(bridge.molt_handle_for_pyobj(pointer).is_none());
                let mut error_type = ptr::null_mut();
                let mut value = ptr::null_mut();
                let mut traceback = ptr::null_mut();
                molt_cpython_abi::api::errors::PyErr_Fetch(
                    &raw mut error_type,
                    &raw mut value,
                    &raw mut traceback,
                );
                let error_type_owner = OwnedPyObject::from_owned(error_type);
                let value_owner = OwnedPyObject::from_owned(value);
                let traceback_owner = OwnedPyObject::from_owned(traceback);
                assert_eq!(error_type, (&raw mut PyExc_TypeError).cast());
                drop(error_type_owner);
                drop(value_owner);
                drop(traceback_owner);
                assert!(!crate::exception_pending(&py));
            }
            // The rejected reservation must be gone; a later admissible
            // crossing of the same address gets one real, releasable wrapper.
            (*(*pointer).ob_type).tp_flags = 0;
            let bits = bridge
                .molt_value_for_pyobj(pointer)
                .expect("fresh admitted crossing");
            assert_eq!(
                object_type_id(crate::obj_from_bits(bits).as_ptr().unwrap()),
                crate::TYPE_ID_FOREIGN
            );
            dec_ref_bits(&py, bits);
            assert_eq!(object.ob_refcnt, 1);
        });
    }

    unsafe extern "C" fn count_crossing_capsule_deallocation(capsule: *mut PyObject) {
        let counter =
            unsafe { molt_cpython_abi::api::capsule::PyCapsule_GetPointer(capsule, ptr::null()) }
                .cast::<TestAtomicUsize>();
        assert!(!counter.is_null());
        unsafe { (*counter).fetch_add(1, AtomicOrdering::SeqCst) };
    }

    #[test]
    fn failed_managed_observation_cannot_acquire_a_foreign_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{errors, mapping, modules, refcount, sequences};
            let list = sequences::PyList_New(0);
            let module = modules::PyModule_New(c"failed_observation".as_ptr());
            assert!(!list.is_null() && !module.is_null());
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let bits = bridge.molt_handle_for_pyobj(list).unwrap().bits();
            let count = (*list).ob_refcnt;
            let layout = list.cast::<PyListObject>();
            (*layout).ob_base.ob_size = -1;
            assert!(bridge.molt_value_for_pyobj(list).is_none());
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
            );
            errors::PyErr_Clear();
            assert_eq!(
                modules::PyModule_AddObjectRef(module, c"payload".as_ptr(), list),
                -1
            );
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
            );
            (*layout).ob_base.ob_size = 0;
            errors::PyErr_Clear();
            assert_eq!((*list).ob_refcnt, count);
            assert_eq!(bridge.molt_handle_for_pyobj(list).unwrap().bits(), bits);
            let dict = modules::PyModule_GetDict(module);
            assert!(mapping::PyDict_GetItemString(dict, c"payload".as_ptr()).is_null());
            let observed = bridge
                .molt_value_for_pyobj(list)
                .expect("restored managed observation");
            assert_eq!(observed, bits);
            dec_ref_bits(&py, observed);
            refcount::Py_DECREF(list);
            refcount::Py_DECREF(module);
            assert!(!crate::exception_pending(&py));
        });
    }

    #[test]
    fn incomplete_list_projection_rejections_release_bridge_locks_before_raising() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{errors, refcount, sequences};
            let list = sequences::PyList_New(1);
            let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let bits = bridge.molt_handle_for_pyobj(list).unwrap().bits();
            assert!(
                bridge
                    .prepare_list_insert(bits, MoltObject::none().bits())
                    .is_none()
            );
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
            assert!(bridge.detach_list_projection_for_sort(bits).is_none());
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
            assert!(!bridge.publish_list_swap(bits, 0, 0));
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
            refcount::Py_DECREF(list);
            assert!(!crate::exception_pending(&py));
        });
    }

    #[test]
    fn module_and_generic_mapping_crossings_release_native_owners() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{capsule, mapping, modules, object, refcount, strings};
            let destroyed = TestAtomicUsize::new(0);
            let new_capsule = || {
                capsule::PyCapsule_New(
                    (&raw const destroyed).cast_mut().cast(),
                    ptr::null(),
                    Some(count_crossing_capsule_deallocation),
                )
            };
            let module = modules::PyModule_New(c"crossing_ownership".as_ptr());
            assert!(!module.is_null());
            let dict = modules::PyModule_GetDict(module);
            assert!(!dict.is_null());
            for (index, steal) in [true, false].into_iter().enumerate() {
                let value = new_capsule();
                assert!(!value.is_null());
                let rc = if steal {
                    modules::PyModule_AddObject(module, c"payload".as_ptr(), value)
                } else {
                    modules::PyModule_AddObjectRef(module, c"payload".as_ptr(), value)
                };
                assert_eq!(rc, 0);
                assert_eq!((*value).ob_refcnt, if steal { 1 } else { 2 });
                assert_eq!(mapping::PyDict_DelItemString(dict, c"payload".as_ptr()), 0);
                if !steal {
                    assert_eq!(destroyed.load(AtomicOrdering::SeqCst), index);
                    assert_eq!((*value).ob_refcnt, 1);
                    refcount::Py_DECREF(value);
                }
                assert_eq!(destroyed.load(AtomicOrdering::SeqCst), index + 1);
            }
            let value = new_capsule();
            assert_eq!(
                modules::PyModule_AddObject(
                    (&raw mut molt_cpython_abi::abi_types::Py_None).cast(),
                    c"payload".as_ptr(),
                    value
                ),
                -1
            );
            assert_eq!(
                (*value).ob_refcnt,
                1,
                "failed insertion must preserve caller ownership"
            );
            assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::errors::PyErr_Clear();
            refcount::Py_DECREF(value);
            assert_eq!(destroyed.load(AtomicOrdering::SeqCst), 3);

            let key = strings::PyUnicode_FromString(c"payload".as_ptr());
            let value = new_capsule();
            assert_eq!(object::PyObject_SetItem(dict, key, value), 0);
            refcount::Py_DECREF(value);
            let fetched = object::PyObject_GetItem(dict, key);
            assert_eq!(fetched, value);
            refcount::Py_DECREF(fetched);
            assert_eq!(object::PyObject_DelItem(dict, key), 0);
            assert_eq!(destroyed.load(AtomicOrdering::SeqCst), 4);
            assert_eq!(object::PyObject_DelItem(dict, key), -1);
            assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::errors::PyErr_Clear();
            refcount::Py_DECREF(key);
            refcount::Py_DECREF(module);
            let index = molt_cpython_abi::api::numbers::PyLong_FromLong(-1);
            for (ordinal, list) in [true, false].into_iter().enumerate() {
                let value = new_capsule();
                let container = if list {
                    molt_cpython_abi::api::sequences::PyList_New(1)
                } else {
                    molt_cpython_abi::api::sequences::PyTuple_New(1)
                };
                assert!(!container.is_null());
                let rc = if list {
                    molt_cpython_abi::api::sequences::PyList_SetItem(container, 0, value)
                } else {
                    molt_cpython_abi::api::sequences::PyTuple_SetItem(container, 0, value)
                };
                assert_eq!(rc, 0);
                let fetched = object::PyObject_GetItem(container, index);
                assert_eq!(
                    fetched, value,
                    "generic indexing must preserve the physical C identity"
                );
                refcount::Py_DECREF(fetched);
                refcount::Py_DECREF(container);
                assert_eq!(destroyed.load(AtomicOrdering::SeqCst), 5 + ordinal);
            }
            refcount::Py_DECREF(index);
            assert!(!crate::exception_pending(&py));
        });
    }

    #[repr(C)]
    struct ForeignCrossingProbe {
        object: PyObject,
        calls: usize,
        deletes: usize,
        value: Option<u64>,
        result: Option<u64>,
        fail: bool,
        error_value: *mut PyObject,
    }

    unsafe fn foreign_probe_failure(probe: &mut ForeignCrossingProbe) -> bool {
        if !probe.fail {
            return false;
        }
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast(),
                c"foreign probe failure".as_ptr(),
            );
        }
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        probe.error_value = error.value;
        molt_cpython_abi::api::errors::restore_current_error_exact(error);
        true
    }

    unsafe extern "C" fn foreign_probe_getattr(
        object: *mut PyObject,
        name: *mut PyObject,
    ) -> *mut PyObject {
        let probe = unsafe { &mut *object.cast::<ForeignCrossingProbe>() };
        probe.calls += 1;
        if unsafe { foreign_probe_failure(probe) } {
            return ptr::null_mut();
        }
        match probe.result {
            Some(bits) => unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)
            },
            None => unsafe { molt_cpython_abi::api::object::Py_NewRef(name) },
        }
    }

    unsafe extern "C" fn foreign_probe_setattr(
        object: *mut PyObject,
        _name: *mut PyObject,
        value: *mut PyObject,
    ) -> c_int {
        let probe = unsafe { &mut *object.cast::<ForeignCrossingProbe>() };
        probe.calls += 1;
        if unsafe { foreign_probe_failure(probe) } {
            return -1;
        }
        probe.value = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(value)
            .map(|value| value.bits());
        probe.deletes += usize::from(value.is_null());
        0
    }

    unsafe extern "C" fn foreign_probe_call(
        object: *mut PyObject,
        args: *mut PyObject,
        kwargs: *mut PyObject,
    ) -> *mut PyObject {
        let probe = unsafe { &mut *object.cast::<ForeignCrossingProbe>() };
        probe.calls += 1;
        if unsafe { foreign_probe_failure(probe) } {
            return ptr::null_mut();
        }
        match probe.result {
            Some(bits) => unsafe {
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)
            },
            None => unsafe {
                molt_cpython_abi::api::object::Py_NewRef(if kwargs.is_null() {
                    args
                } else {
                    kwargs
                })
            },
        }
    }

    #[test]
    fn c_api_attribute_assignment_distinguishes_zero_from_deletion() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        unsafe {
            use molt_cpython_abi::api::{errors, modules, numbers, object, refcount};
            let module = modules::PyModule_New(c"attribute_hook_consumer".as_ptr());
            let zero = numbers::PyFloat_FromDouble(0.0);
            assert!(!module.is_null());
            assert!(!zero.is_null());
            assert_eq!(
                object::PyObject_SetAttrString(module, c"value".as_ptr(), zero),
                0
            );
            assert!(errors::PyErr_Occurred().is_null());
            let assigned = object::PyObject_GetAttrString(module, c"value".as_ptr());
            assert!(
                !assigned.is_null(),
                "float +0.0 must be assigned, not deleted"
            );
            assert_eq!(
                numbers::PyFloat_AsDouble(assigned).to_bits(),
                0.0f64.to_bits()
            );
            refcount::Py_DECREF(assigned);
            assert_eq!(
                object::PyObject_SetAttrString(module, c"value".as_ptr(), ptr::null_mut()),
                0,
            );
            assert_eq!(
                object::PyObject_HasAttrStringWithError(module, c"value".as_ptr()),
                0,
            );
            assert!(errors::PyErr_Occurred().is_null());
            refcount::Py_DECREF(zero);
            refcount::Py_DECREF(module);
        }
    }

    #[test]
    fn foreign_slot_crossings_borrow_arguments_and_preserve_exact_errors() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::errors;
            use molt_cpython_abi::bridge::{
                GLOBAL_BRIDGE, molt_foreign_call, molt_foreign_getattr, molt_foreign_setattr,
            };
            let mut typ: PyTypeObject = std::mem::zeroed();
            typ.tp_name = c"ForeignCrossingProbe".as_ptr();
            typ.tp_getattro = Some(foreign_probe_getattr);
            typ.tp_setattro = Some(foreign_probe_setattr);
            typ.tp_call = Some(foreign_probe_call);
            let mut probe = ForeignCrossingProbe {
                object: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut typ,
                },
                calls: 0,
                deletes: 0,
                value: None,
                result: None,
                fail: false,
                error_value: ptr::null_mut(),
            };
            let address = (&raw mut probe.object).expose_provenance();
            let name = MoltObject::from_ptr(crate::alloc_string(&py, b"borrowed_payload")).bits();
            let value = MoltObject::from_ptr(crate::alloc_list(&py, &[])).bits();
            let args = MoltObject::from_ptr(crate::alloc_tuple(&py, &[value])).bits();
            let kwargs = MoltObject::from_ptr(crate::alloc_dict_with_pairs(&py, &[])).bits();
            let owners = [name, value, args, kwargs];
            // Keep canonical physical views live so a stolen borrowed runtime
            // reference is detected by exact counts before it becomes a UAF.
            for bits in owners {
                assert!(!GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits).is_null());
            }
            let counts = owners.map(|bits| hook_ref_count(bits));
            for fail in [false, true] {
                probe.fail = fail;
                for operation in 0..3 {
                    let before = probe.calls;
                    let result = match operation {
                        0 => molt_foreign_getattr(address, name, AttributeAccess::Normal),
                        1 => {
                            let rc = molt_foreign_setattr(
                                address,
                                name,
                                Some(value),
                                AttributeMutation::Normal,
                            );
                            assert_eq!(rc, if fail { -1 } else { 0 });
                            if !fail {
                                assert_eq!(probe.value, Some(value));
                            }
                            if fail {
                                OwnedHandleResult::error()
                            } else {
                                OwnedHandleResult::ok(MoltObject::none().bits())
                            }
                        }
                        _ => molt_foreign_call(address, args, kwargs),
                    };
                    assert_eq!(probe.calls, before + 1);
                    if fail {
                        assert!(matches!(result.decode(), DecodedHandleResult::Error));
                        let error =
                            errors::take_current_error().expect("callee error survives cleanup");
                        assert_eq!(error.exc_type, (&raw mut PyExc_TypeError).cast());
                        assert_eq!(error.value, probe.error_value);
                        drop(error);
                        errors::PyErr_Clear();
                    } else if operation != 1 {
                        let DecodedHandleResult::Ok(bits) = result.decode() else {
                            panic!("foreign result must succeed");
                        };
                        assert_eq!(bits, if operation == 0 { name } else { kwargs });
                        dec_ref_bits(&py, bits);
                    }
                    assert_eq!(
                        owners.map(|bits| hook_ref_count(bits)),
                        counts,
                        "foreign operation {operation} must not consume borrowed arguments (fail={fail})"
                    );
                    assert!(errors::PyErr_Occurred().is_null());
                }
            }
            probe.fail = false;
            let wrapper = GLOBAL_BRIDGE
                .molt_value_for_pyobj(&raw mut probe.object)
                .unwrap();
            // Actual runtime attribute dispatch must distinguish float +0.0
            // (raw bits zero) from deletion and share the same C slot path.
            crate::molt_set_attr_name(wrapper, name, MoltObject::from_float(0.0).bits());
            assert_eq!(probe.value, Some(0));
            assert_eq!(probe.deletes, 0);
            crate::molt_del_attr_name(wrapper, name);
            assert_eq!(probe.deletes, 1);
            for scalar in [0.0, -0.0, 1.5] {
                let bits = MoltObject::from_float(scalar).bits();
                probe.result = Some(bits);
                assert_eq!(crate::molt_get_attr_name(wrapper, name), bits);
                assert!(!crate::exception_pending(&py));
                assert_eq!(
                    crate::molt_call_bind(wrapper, crate::molt_callargs_new(0, 0)),
                    bits
                );
                assert!(!crate::exception_pending(&py));
                let DecodedHandleResult::Ok(called) = hook_object_call(wrapper, 0, 0).decode()
                else {
                    panic!("typed object-call result rejected a successful scalar");
                };
                assert_eq!(called, bits);
            }
            probe.fail = true;
            let calls_before_failure = probe.calls;
            let failed_call = crate::molt_call_bind(wrapper, crate::molt_callargs_new(0, 0));
            assert!(MoltObject::from_bits(failed_call).is_none());
            assert_eq!(probe.calls, calls_before_failure + 1);
            assert_eq!(pending_exception_type_for_assertion(), "TypeError");
            let message = pending_exception_message_for_assertion();
            assert!(message.contains("foreign probe failure"), "{message}");
            assert!(
                !message.contains("not callable"),
                "the foreign C error must not be replaced by a generic call error: {message}",
            );
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!(owners.map(|bits| hook_ref_count(bits)), counts);
            dec_ref_bits(&py, wrapper);
            assert_eq!(probe.object.ob_refcnt, 1);
            for bits in owners.into_iter().rev() {
                dec_ref_bits(&py, bits);
            }
            assert!(!crate::exception_pending(&py));
        });
    }

    #[test]
    fn module_and_mapping_reference_edges_allow_valid_incomplete_containers() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{errors, mapping, modules, numbers, refcount, sequences};
            let module = modules::PyModule_New(c"construction_edges".as_ptr());
            let dict = modules::PyModule_GetDict(module);
            let list = sequences::PyList_New(1);
            assert_eq!(
                modules::PyModule_AddObjectRef(module, c"list".as_ptr(), list),
                0
            );
            assert_eq!(
                mapping::PyDict_SetItemString(dict, c"alias".as_ptr(), list),
                0
            );
            // Storing a reference does not make uninitialized values readable.
            assert!(sequences::PyList_GetItem(list, 0).is_null());
            assert!(!errors::PyErr_Occurred().is_null());
            errors::PyErr_Clear();
            assert_eq!(
                sequences::PyList_SetItem(list, 0, numbers::PyLong_FromLong(7)),
                0
            );
            assert_eq!(mapping::PyDict_GetItemString(dict, c"list".as_ptr()), list);
            assert_eq!(mapping::PyDict_GetItemString(dict, c"alias".as_ptr()), list);
            refcount::Py_DECREF(list);
            refcount::Py_DECREF(module);
            assert!(!crate::exception_pending(&py));
        });
    }

    #[test]
    fn c_api_container_results_distinguish_float_zero_from_missing_slots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{
                abstract_number, errors, mapping, numbers, refcount, sequences,
            };
            let scalar = numbers::PyFloat_FromDouble(0.0);
            let tuple = sequences::PyTuple_New(1);
            let list = sequences::PyList_New(0);
            let dict = mapping::PyDict_New();
            assert!(!scalar.is_null() && !tuple.is_null() && !list.is_null() && !dict.is_null());
            assert!(sequences::PyTuple_GetItem(tuple, 0).is_null());
            assert!(errors::PyErr_Occurred().is_null());
            refcount::Py_INCREF(scalar);
            assert_eq!(sequences::PyTuple_SetItem(tuple, 0, scalar), 0);
            assert_eq!(sequences::PyList_Append(list, scalar), 0);
            assert_eq!(
                mapping::PyDict_SetItemString(dict, c"zero".as_ptr(), scalar),
                0
            );
            for result in [
                sequences::PyTuple_GetItem(tuple, 0),
                sequences::PyList_GetItem(list, 0),
                mapping::PyDict_GetItemString(dict, c"zero".as_ptr()),
            ] {
                assert!(
                    !result.is_null(),
                    "successful float zero is not NULL/missing"
                );
                assert_eq!(numbers::PyFloat_AsDouble(result).to_bits(), 0);
            }
            let computed = abstract_number::PyNumber_Add(scalar, scalar);
            assert!(
                !computed.is_null(),
                "numeric result status is independent of bits0"
            );
            assert_eq!(numbers::PyFloat_AsDouble(computed).to_bits(), 0);
            refcount::Py_DECREF(computed);
            for object in [scalar, tuple, list, dict] {
                refcount::Py_DECREF(object);
            }
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    #[test]
    fn c_api_power_preserves_absent_none_and_zero_modulus_semantics() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            use molt_cpython_abi::abi_types::Py_None;
            use molt_cpython_abi::api::{abstract_number, errors, numbers, refcount};

            let base = numbers::PyLong_FromLong(2);
            let exponent = numbers::PyLong_FromLong(3);
            let positive_zero = numbers::PyFloat_FromDouble(0.0);
            let negative_zero = numbers::PyFloat_FromDouble(-0.0);
            let integer_zero = numbers::PyLong_FromLong(0);
            let modulus = numbers::PyLong_FromLong(5);
            assert!(
                [
                    base,
                    exponent,
                    positive_zero,
                    negative_zero,
                    integer_zero,
                    modulus
                ]
                .into_iter()
                .all(|value| !value.is_null())
            );

            for power in [
                abstract_number::PyNumber_Power,
                abstract_number::PyNumber_InPlacePower,
            ] {
                for (value, expected) in [
                    (std::ptr::null_mut(), 8),
                    (&raw mut Py_None, 8),
                    (modulus, 3),
                ] {
                    let result = power(base, exponent, value);
                    assert!(!result.is_null());
                    assert_eq!(numbers::PyLong_AsLong(result), expected);
                    refcount::Py_DECREF(result);
                    assert!(errors::PyErr_Occurred().is_null());
                }
                for (value, exception) in [
                    (positive_zero, (&raw mut PyExc_TypeError).cast::<PyObject>()),
                    (negative_zero, (&raw mut PyExc_TypeError).cast::<PyObject>()),
                    (integer_zero, (&raw mut PyExc_ValueError).cast::<PyObject>()),
                ] {
                    assert!(
                        power(base, exponent, value).is_null(),
                        "present zero is not an omitted modulus"
                    );
                    assert_eq!(errors::PyErr_ExceptionMatches(exception), 1);
                    errors::PyErr_Clear();
                    crate::clear_exception(&py);
                }
            }
            for object in [
                base,
                exponent,
                positive_zero,
                negative_zero,
                integer_zero,
                modulus,
            ] {
                refcount::Py_DECREF(object);
            }
            assert!(!crate::exception_pending(&py));
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    #[test]
    fn raw_c_type_getattr_resolves_own_type_dict() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|_py| crate::clear_exception(&_py));

        let mut raw_type: PyTypeObject = unsafe { std::mem::zeroed() };
        raw_type.ob_base.ob_base.ob_refcnt = 1;
        raw_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        raw_type.tp_name = c"numpy.ndarray".as_ptr();
        raw_type.tp_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
        assert!(!raw_type.tp_dict.is_null());
        let finalize = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(17) };
        assert!(!finalize.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::mapping::PyDict_SetItemString(
                    raw_type.tp_dict,
                    c"__array_finalize__".as_ptr(),
                    finalize,
                )
            },
            0
        );
        let raw_type_ptr = (&raw mut raw_type).cast::<PyObject>();
        assert!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(raw_type_ptr)
                .is_none(),
            "a raw C type must remain on its native metatype slot path"
        );

        let result = unsafe {
            molt_cpython_abi::api::object::PyObject_GetAttrString(
                raw_type_ptr,
                c"__array_finalize__".as_ptr(),
            )
        };

        assert!(
            !result.is_null(),
            "raw type lookup failed: c_error={:p} runtime_pending={} type_getattro={} dict={:p}",
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            with_gil(|_py| crate::exception_pending(&_py)),
            unsafe { (*raw_type.ob_base.ob_base.ob_type).tp_getattro.is_some() },
            raw_type.tp_dict,
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(result) },
            17
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "type_getattro must resolve the class MRO dictionary without a stale AttributeError"
        );
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(result);
            molt_cpython_abi::api::refcount::Py_DECREF(finalize);
            molt_cpython_abi::api::refcount::Py_DECREF(raw_type.tp_dict);
        }
    }

    #[test]
    fn dict_hook_set_publishes_every_sparse_table_entry_at_index_78() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        let dict_bits = unsafe { hook_alloc_dict() };
        assert_ne!(dict_bits, 0);
        let dict_ptr = MoltObject::from_bits(dict_bits).as_ptr().unwrap();

        with_gil(|_py| unsafe {
            for index in 0..78 {
                let key_bits = MoltObject::from_int(index).bits();
                let value_bits = MoltObject::from_int(index * 10).bits();
                dict_set_in_place(&_py, dict_ptr, key_bits, value_bits);
            }
        });

        let key_bits = MoltObject::from_int(78).bits();
        let value_bits = MoltObject::from_int(780).bits();
        unsafe {
            hook_dict_mutate(
                dict_bits,
                key_bits,
                value_bits,
                0,
                None,
                std::ptr::null_mut(),
            )
        };

        with_gil(|_py| unsafe {
            assert_eq!(crate::dict_len(dict_ptr), 79);
            assert_eq!(crate::dict_live_entries(dict_ptr).count(), 79);
            // Query the table produced by the actual mutation. Rebuilding it
            // inside the test would mask a stale or missing index publication.
            for index in 0..79 {
                assert_eq!(
                    dict_get_in_place(&_py, dict_ptr, MoltObject::from_int(index).bits()),
                    Some(MoltObject::from_int(index * 10).bits())
                );
            }
        });
    }

    #[test]
    fn c_dict_cursor_skips_actual_deleted_rows_and_preserves_outputs() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        with_gil(|py| unsafe {
            use molt_cpython_abi::api::{errors, mapping, numbers, refcount};
            let dictionary = mapping::PyDict_New();
            assert!(!dictionary.is_null());
            let mut keys = Vec::new();
            for index in 0..5 {
                let key = numbers::PyLong_FromLong(index);
                let value = numbers::PyLong_FromLong(index * 10);
                assert_eq!(mapping::PyDict_SetItem(dictionary, key, value), 0);
                keys.push(key);
                refcount::Py_DECREF(value);
            }
            for index in [0, 2, 4] {
                assert_eq!(mapping::PyDict_DelItem(dictionary, keys[index]), 0);
            }
            assert_eq!(mapping::PyDict_Size(dictionary), 2);
            let mut position = 0;
            let (mut key, mut value) = (std::ptr::null_mut(), std::ptr::null_mut());
            assert_eq!(
                mapping::PyDict_Next(dictionary, &mut position, &mut key, &mut value),
                1
            );
            assert_eq!(
                (
                    position,
                    numbers::PyLong_AsLong(key),
                    numbers::PyLong_AsLong(value)
                ),
                (2, 1, 10)
            );
            let replacement = numbers::PyLong_FromLong(333);
            assert_eq!(mapping::PyDict_SetItem(dictionary, keys[3], replacement), 0);
            refcount::Py_DECREF(replacement);
            assert_eq!(
                mapping::PyDict_Next(dictionary, &mut position, &mut key, &mut value),
                1
            );
            assert_eq!(
                (
                    position,
                    numbers::PyLong_AsLong(key),
                    numbers::PyLong_AsLong(value)
                ),
                (4, 3, 333)
            );
            let last = (key, value);
            for initial in [4, 5, -1, isize::MAX] {
                position = initial;
                assert_eq!(
                    mapping::PyDict_Next(dictionary, &mut position, &mut key, &mut value),
                    0
                );
                assert_eq!((position, key, value), (initial, last.0, last.1));
            }
            position = 0;
            assert_eq!(
                mapping::PyDict_Next(
                    dictionary,
                    &mut position,
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                1
            );
            assert_eq!(position, 2);
            assert!(errors::PyErr_Occurred().is_null());
            assert!(!crate::exception_pending(&py));
            for key in keys {
                refcount::Py_DECREF(key);
            }
            refcount::Py_DECREF(dictionary);
        });
    }

    struct ModuleCacheRestore {
        name_bits: u64,
        previous_bits: u64,
    }

    impl ModuleCacheRestore {
        fn new(_py: &crate::PyToken<'_>, name_bits: u64) -> Self {
            let previous_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
            let _ = crate::molt_exception_clear();
            let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
            let _ = crate::molt_exception_clear();
            Self {
                name_bits,
                previous_bits,
            }
        }
    }

    impl Drop for ModuleCacheRestore {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(_py, {
                let _ = crate::molt_exception_clear();
                let _ = crate::builtins::modules::molt_module_cache_del(self.name_bits);
                let _ = crate::molt_exception_clear();
                if !MoltObject::from_bits(self.previous_bits).is_none() {
                    let restore_bits = crate::builtins::modules::molt_module_cache_set(
                        self.name_bits,
                        self.previous_bits,
                    );
                    if !MoltObject::from_bits(restore_bits).is_none() {
                        dec_ref_bits(_py, restore_bits);
                    }
                    let _ = crate::molt_exception_clear();
                    dec_ref_bits(_py, self.previous_bits);
                }
                dec_ref_bits(_py, self.name_bits);
            });
        }
    }

    fn static_extension_cache_restore(name: &str) -> ModuleCacheRestore {
        with_gil(|py| {
            let name_ptr = alloc_string(&py, name.as_bytes());
            assert!(!name_ptr.is_null());
            ModuleCacheRestore::new(&py, MoltObject::from_ptr(name_ptr).bits())
        })
    }

    fn assert_static_extension_cached(name_bits: u64, expected_bits: u64) {
        let cached_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
        assert_eq!(cached_bits, expected_bits);
        with_gil(|py| dec_ref_bits(&py, cached_bits));
    }

    fn assert_static_extension_qualified_identity(module_bits: u64, qualified_name: &str) {
        unsafe {
            let module =
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(module_bits);
            assert!(!module.is_null());
            let name = molt_cpython_abi::api::modules::PyModule_GetName(module);
            assert!(!name.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(name).to_str().unwrap(),
                qualified_name
            );
            let spec =
                molt_cpython_abi::api::object::PyObject_GetAttrString(module, c"__spec__".as_ptr());
            assert!(!spec.is_null(), "extension module must expose __spec__");
            assert_eq!(
                molt_cpython_abi::api::modules::PyModule_Check(spec),
                0,
                "ModuleSpec must not be a module-shaped substitute"
            );
            let spec_type = molt_cpython_abi::api::typeobj::PyObject_Type(spec);
            assert!(!spec_type.is_null());
            let type_name = molt_cpython_abi::api::object::PyObject_GetAttrString(
                spec_type,
                c"__name__".as_ptr(),
            );
            assert!(!type_name.is_null());
            let type_name_utf8 = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(type_name);
            assert!(!type_name_utf8.is_null());
            assert_eq!(std::ffi::CStr::from_ptr(type_name_utf8), c"ModuleSpec");
            molt_cpython_abi::api::refcount::Py_DECREF(type_name);
            molt_cpython_abi::api::refcount::Py_DECREF(spec_type);
            let spec_name =
                molt_cpython_abi::api::object::PyObject_GetAttrString(spec, c"name".as_ptr());
            assert!(
                !spec_name.is_null(),
                "extension ModuleSpec must have a name"
            );
            let spec_name_utf8 = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(spec_name);
            assert!(!spec_name_utf8.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(spec_name_utf8).to_str().unwrap(),
                qualified_name,
            );
            let package = molt_cpython_abi::api::object::PyObject_GetAttrString(
                module,
                c"__package__".as_ptr(),
            );
            assert!(
                !package.is_null(),
                "extension module must expose __package__"
            );
            let package_utf8 = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(package);
            assert!(!package_utf8.is_null());
            let expected_package = qualified_name
                .rsplit_once('.')
                .map_or("", |(parent, _)| parent);
            assert_eq!(
                std::ffi::CStr::from_ptr(package_utf8).to_str().unwrap(),
                expected_package,
            );
            molt_cpython_abi::api::refcount::Py_DECREF(package);
            molt_cpython_abi::api::refcount::Py_DECREF(spec_name);
            molt_cpython_abi::api::refcount::Py_DECREF(spec);
        }
    }

    #[test]
    fn pyimport_importmodule_routes_through_runtime_import_pipeline() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let module = unsafe {
            molt_cpython_abi::api::imports::PyImport_ImportModule(
                c"molt_test_definitely_absent_module".as_ptr(),
            )
        };

        assert!(module.is_null());
        // The runtime import pipeline owns the failure: the pending error is
        // the real ModuleNotFoundError, never the standalone ABI stub text.
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("molt_test_definitely_absent_module"),
            "runtime import pipeline must name the missing module: {message}"
        );
        assert!(
            !message.contains("standalone molt-cpython-abi"),
            "registered hooks must not surface the standalone stub error: {message}"
        );
    }

    #[test]
    fn pysys_getobject_missing_sys_module_clears_speculative_import_failure() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _cache_restore = with_gil(|_py| {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            ModuleCacheRestore::new(&_py, name_bits)
        });

        let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        assert!(
            flags.is_null(),
            "PySys_GetObject(flags) must fail closed when sys is not linked"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "speculative sys import failure must not leak into the C-API caller"
        );
    }

    #[test]
    fn pysys_getobject_reads_interpreter_sys_module_attribute() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let (_cache_restore, expected_flags_bits, sys_module_bits) = with_gil(|_py| unsafe {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(&_py, name_bits);

            let module_ptr = alloc_module_obj(&_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let flags_ptr = crate::alloc_tuple(&_py, &[MoltObject::from_int(7).bits()]);
            assert!(!flags_ptr.is_null());
            let flags_bits = MoltObject::from_ptr(flags_ptr).bits();

            let flags_name_ptr = alloc_string(&_py, b"flags");
            assert!(!flags_name_ptr.is_null());
            let flags_name_bits = MoltObject::from_ptr(flags_name_ptr).bits();
            let dict_bits = module_dict_bits(module_ptr);
            let dict_ptr = MoltObject::from_bits(dict_bits)
                .as_ptr()
                .expect("sys module dict pointer");
            assert_eq!(object_type_id(dict_ptr), TYPE_ID_DICT);
            dict_set_in_place(&_py, dict_ptr, flags_name_bits, flags_bits);
            let zero_name = MoltObject::from_ptr(alloc_string(&_py, b"molt_zero_probe")).bits();
            dict_set_in_place(
                &_py,
                dict_ptr,
                zero_name,
                MoltObject::from_float(0.0).bits(),
            );
            dec_ref_bits(&_py, zero_name);
            dec_ref_bits(&_py, flags_name_bits);
            assert!(
                !crate::exception_pending(&_py),
                "test sys.flags registration must not leave an exception"
            );

            let result_bits =
                crate::builtins::module_table::publish_interpreter_sys_for_test(&_py, module_bits);
            if !MoltObject::from_bits(result_bits).is_none() {
                dec_ref_bits(&_py, result_bits);
            }
            assert!(
                !crate::exception_pending(&_py),
                "test sys module registration must not leave an exception"
            );

            (cache_restore, flags_bits, module_bits)
        });

        let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        let zero =
            unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"molt_zero_probe".as_ptr()) };
        assert!(
            !zero.is_null(),
            "sys value +0.0 must not become a missing attribute"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(zero) }.to_bits(),
            0
        );
        assert!(
            !flags.is_null(),
            "PySys_GetObject(flags) must resolve through the interpreter sys module"
        );
        let flags_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(flags)
            .map(|value| value.bits())
            .unwrap_or(0);
        assert_eq!(
            flags_bits, expected_flags_bits,
            "PySys_GetObject must prefer sys.flags over the raw flags payload"
        );
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);

        let flags_again = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
        assert_eq!(flags_again, flags);
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(flags) };
        assert_eq!(unsafe { (*flags).ob_refcnt }, 2);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(flags) };
        assert_eq!(unsafe { (*flags).ob_refcnt }, 1);
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(flags),
            molt_cpython_abi::bridge::PyObjRelease::ManagedViewRetired
        );

        with_gil(|_py| {
            let flags_ptr = MoltObject::from_bits(expected_flags_bits)
                .as_ptr()
                .expect("sys.flags test object must remain live");
            assert_eq!(unsafe { object_type_id(flags_ptr) }, TYPE_ID_TUPLE);
            assert_eq!(unsafe { tuple_len(flags_ptr) }, 1);
            dec_ref_bits(&_py, expected_flags_bits);
            dec_ref_bits(&_py, sys_module_bits);
        });
    }

    // C sys lookup observes only the rooted dictionary. Missing keys stay
    // missing without invoking the public module attribute protocol.

    #[test]
    fn pysys_getobject_missing_attr_on_cold_module_fails_closed() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let (_cache_restore, sys_module_bits) = with_gil(|_py| {
            let name_ptr = alloc_string(&_py, b"sys");
            assert!(!name_ptr.is_null());
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let cache_restore = ModuleCacheRestore::new(&_py, name_bits);

            let module_ptr = alloc_module_obj(&_py, name_bits);
            assert!(!module_ptr.is_null());
            let module_bits = MoltObject::from_ptr(module_ptr).bits();

            let result_bits =
                crate::builtins::module_table::publish_interpreter_sys_for_test(&_py, module_bits);
            if !MoltObject::from_bits(result_bits).is_none() {
                dec_ref_bits(&_py, result_bits);
            }
            assert!(
                !crate::exception_pending(&_py),
                "test sys module registration must not leave an exception"
            );
            (cache_restore, module_bits)
        });

        let missing = unsafe {
            molt_cpython_abi::api::sys::PySys_GetObject(c"molt_cold_absent_attr".as_ptr())
        };
        assert!(
            missing.is_null(),
            "PySys_GetObject must fail closed for an attribute absent from a cold sys dict"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "dictionary miss must not leak a pending exception"
        );

        with_gil(|_py| {
            dec_ref_bits(&_py, sys_module_bits);
        });
    }

    static SYS_LOOKUP_MODE: AtomicUsize = AtomicUsize::new(0);
    static SYS_LOOKUP_CALLS: AtomicUsize = AtomicUsize::new(0);
    static SYS_LOOKUP_REPORTS: AtomicUsize = AtomicUsize::new(0);
    static SYS_LOOKUP_MODULE: AtomicU64 = AtomicU64::new(0);
    static SYS_LOOKUP_GETATTR_CALLS: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn sys_lookup_key_eq(_self: u64, _other: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            SYS_LOOKUP_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
            match SYS_LOOKUP_MODE.swap(0, AtomicOrdering::SeqCst) {
                1 => crate::raise_exception::<u64>(py, "RuntimeError", "sys key equality failed"),
                2 => {
                    // Drop public, private and registry owners during equality.
                    // Only the interpreter role may keep this namespace alive.
                    let name = crate::attr_name_bits_from_bytes(py, b"sys").unwrap();
                    crate::builtins::modules::molt_module_cache_del(name);
                    let sys = SYS_LOOKUP_MODULE.load(AtomicOrdering::SeqCst);
                    let modules = crate::builtins::modules::sys_modules_dict_bits(py, sys).unwrap();
                    unsafe {
                        dict_set_in_place(
                            py,
                            MoltObject::from_bits(modules).as_ptr().unwrap(),
                            name,
                            MoltObject::from_int(91).bits(),
                        );
                    }
                    let nested = unsafe {
                        molt_cpython_abi::api::sys::PySys_GetObject(c"molt_sys_probe".as_ptr())
                    };
                    assert!(!nested.is_null());
                    dec_ref_bits(py, modules);
                    dec_ref_bits(py, name);
                    MoltObject::from_bool(true).bits()
                }
                _ => MoltObject::from_bool(true).bits(),
            }
        })
    }

    extern "C" fn sys_lookup_unraisable(_args: u64) -> u64 {
        SYS_LOOKUP_REPORTS.fetch_add(1, AtomicOrdering::SeqCst);
        MoltObject::none().bits()
    }

    extern "C" fn sys_lookup_getattr_trap(_name: u64) -> u64 {
        SYS_LOOKUP_GETATTR_CALLS.fetch_add(1, AtomicOrdering::SeqCst);
        crate::with_gil_entry_nopanic!(py, {
            crate::raise_exception::<u64>(
                py,
                "RuntimeError",
                "PySys_GetObject invoked sys.__getattr__",
            )
        })
    }

    fn sys_lookup_function(py: &crate::PyToken<'_>, target: *const (), arity: u64) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::provenance::abi::expose_function_address(target),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn sys_lookup_key(py: &crate::PyToken<'_>) -> (u64, u64) {
        let name = crate::attr_name_bits_from_bytes(py, b"SysLookupKey").unwrap();
        let class = crate::molt_class_new(name);
        crate::molt_class_set_base(class, crate::builtin_classes(py).object);
        dec_ref_bits(py, name);
        let eq = crate::attr_name_bits_from_bytes(py, b"__eq__").unwrap();
        let function = sys_lookup_function(py, sys_lookup_key_eq as *const (), 2);
        crate::molt_set_attr_name(class, eq, function);
        dec_ref_bits(py, eq);
        dec_ref_bits(py, function);
        let ptr = MoltObject::from_bits(class).as_ptr().unwrap();
        unsafe {
            crate::object::class_finish_definition(py, ptr).unwrap();
        }
        let key = unsafe { crate::alloc_instance_for_class(py, ptr) };
        (key, class)
    }

    #[test]
    fn pysys_getobject_equality_owns_namespace_across_public_replacement_and_deletion() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        with_gil(|py| unsafe {
            let name = crate::attr_name_bits_from_bytes(&py, b"sys").unwrap();
            let _restore = ModuleCacheRestore::new(&py, name);
            let sys = MoltObject::from_ptr(alloc_module_obj(&py, name)).bits();
            // Same-named publication has no interpreter authority.
            crate::builtins::modules::molt_module_cache_set(name, sys);
            assert!(crate::builtins::modules::interpreter_sys_module(&py).is_none());
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            assert_eq!(
                crate::builtins::modules::interpreter_sys_module(&py),
                Some(sys)
            );
            SYS_LOOKUP_MODULE.store(sys, AtomicOrdering::SeqCst);
            let dictionary = module_dict_bits(MoltObject::from_bits(sys).as_ptr().unwrap());
            let dict = MoltObject::from_bits(dictionary).as_ptr().unwrap();
            let (key, class) = sys_lookup_key(&py);
            let value =
                MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(17).bits()]))
                    .bits();
            crate::object::ops::dict_set_with_hash_in_place(
                &py,
                dict,
                key,
                value,
                crate::object::ops_hash::hash_string_bytes(&py, b"molt_sys_probe") as u64,
            );
            dec_ref_bits(&py, value);
            dec_ref_bits(&py, sys);
            SYS_LOOKUP_CALLS.store(0, AtomicOrdering::SeqCst);
            SYS_LOOKUP_MODE.store(2, AtomicOrdering::SeqCst);
            let result = molt_cpython_abi::api::sys::PySys_GetObject(c"molt_sys_probe".as_ptr());
            assert!(!result.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(result)
                    .unwrap()
                    .bits(),
                value
            );
            assert_eq!((*result).ob_refcnt, 1, "result is borrowed");
            assert_eq!(
                SYS_LOOKUP_CALLS.load(AtomicOrdering::SeqCst),
                2,
                "one outer and one nested lookup"
            );
            let replacement = crate::builtins::modules::molt_module_cache_get(name);
            assert_eq!(MoltObject::from_bits(replacement).as_int(), Some(91));
            dec_ref_bits(&py, replacement);
            crate::builtins::modules::molt_module_cache_del(name);
            assert!(
                MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(name))
                    .is_none()
            );
            assert_eq!(
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_sys_probe".as_ptr()),
                result
            );
            assert_eq!((*result).ob_refcnt, 1);
            assert!(!crate::exception_pending(&py));
            dec_ref_bits(&py, key);
            dec_ref_bits(&py, class);
            SYS_LOOKUP_MODULE.store(0, AtomicOrdering::SeqCst);
        });
    }

    #[test]
    fn pysys_getobject_preserves_exact_error_and_gates_unraisable_reporting() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        with_gil(|py| unsafe {
            let state = crate::runtime_state(&py);
            let previous = crate::object::ops_sys::runtime_target_python_info(state);
            let name = crate::attr_name_bits_from_bytes(&py, b"sys").unwrap();
            let _restore = ModuleCacheRestore::new(&py, name);
            let sys = MoltObject::from_ptr(alloc_module_obj(&py, name)).bits();
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            let dict = MoltObject::from_bits(module_dict_bits(
                MoltObject::from_bits(sys).as_ptr().unwrap(),
            ))
            .as_ptr()
            .unwrap();
            let (key, class) = sys_lookup_key(&py);
            crate::object::ops::dict_set_with_hash_in_place(
                &py,
                dict,
                key,
                MoltObject::from_int(17).bits(),
                crate::object::ops_hash::hash_string_bytes(&py, b"molt_sys_probe") as u64,
            );
            let hook_name = crate::attr_name_bits_from_bytes(&py, b"unraisablehook").unwrap();
            let hook = sys_lookup_function(&py, sys_lookup_unraisable as *const (), 1);
            crate::builtins::modules::molt_module_set_attr(sys, hook_name, hook);
            dec_ref_bits(&py, hook_name);
            dec_ref_bits(&py, hook);
            for minor in [12, 13, 14] {
                let mut target = previous.clone();
                target.minor = minor;
                *state.sys_version_info.lock().unwrap() = Some(target);
                SYS_LOOKUP_REPORTS.store(0, AtomicOrdering::SeqCst);
                for raising in [false, true] {
                    molt_cpython_abi::api::errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
                        c"incoming exact exception".as_ptr(),
                    );
                    let (mut kind, mut value, mut traceback) =
                        (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                    molt_cpython_abi::api::errors::PyErr_Fetch(
                        &raw mut kind,
                        &raw mut value,
                        &raw mut traceback,
                    );
                    let kind_owner = OwnedPyObject::from_owned(kind);
                    let value_owner = OwnedPyObject::from_owned(value);
                    let traceback_owner = OwnedPyObject::from_owned(traceback);
                    assert!(!value.is_null());
                    molt_cpython_abi::api::errors::PyErr_Restore(
                        kind_owner.into_ptr(),
                        value_owner.into_ptr(),
                        traceback_owner.into_ptr(),
                    );
                    SYS_LOOKUP_MODE.store(usize::from(raising), AtomicOrdering::SeqCst);
                    let result =
                        molt_cpython_abi::api::sys::PySys_GetObject(c"molt_sys_probe".as_ptr());
                    assert_eq!(result.is_null(), raising);
                    let (mut restored_kind, mut restored_value, mut restored_traceback) =
                        (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
                    molt_cpython_abi::api::errors::PyErr_Fetch(
                        &raw mut restored_kind,
                        &raw mut restored_value,
                        &raw mut restored_traceback,
                    );
                    let restored_kind_owner = OwnedPyObject::from_owned(restored_kind);
                    let restored_value_owner = OwnedPyObject::from_owned(restored_value);
                    let restored_traceback_owner = OwnedPyObject::from_owned(restored_traceback);
                    assert_eq!(
                        (restored_kind, restored_value, restored_traceback),
                        (kind, value, traceback)
                    );
                    drop(restored_kind_owner);
                    drop(restored_value_owner);
                    drop(restored_traceback_owner);
                    assert!(!crate::exception_pending(&py));
                }
                assert_eq!(
                    SYS_LOOKUP_REPORTS.load(AtomicOrdering::SeqCst),
                    usize::from(minor >= 13)
                );
            }
            // The sibling import API deliberately propagates; it must not use
            // PySys_GetObject's suppression or unraisable reporting policy.
            let modules_name = crate::attr_name_bits_from_bytes(&py, b"modules").unwrap();
            crate::dict_del_in_place(&py, dict, modules_name);
            let (modules_key, modules_class) = sys_lookup_key(&py);
            crate::object::ops::dict_set_with_hash_in_place(
                &py,
                dict,
                modules_key,
                MoltObject::from_int(0).bits(),
                crate::object::ops_hash::hash_string_bytes(&py, b"modules") as u64,
            );
            SYS_LOOKUP_MODE.store(1, AtomicOrdering::SeqCst);
            let reports = SYS_LOOKUP_REPORTS.load(AtomicOrdering::SeqCst);
            assert!(molt_cpython_abi::api::imports::PyImport_GetModuleDict().is_null());
            assert_eq!(
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast::<PyObject>()
                ),
                1
            );
            assert_eq!(SYS_LOOKUP_REPORTS.load(AtomicOrdering::SeqCst), reports);
            molt_cpython_abi::api::errors::PyErr_Clear();
            crate::clear_exception(&py);
            // Release the adversarial namespace before cache restoration.
            crate::dict_clear_in_place(&py, dict);
            *state.sys_version_info.lock().unwrap() = Some(previous);
            for bits in [modules_name, modules_key, modules_class, key, class, sys] {
                dec_ref_bits(&py, bits);
            }
        });
    }

    #[test]
    fn pysys_getobject_never_calls_module_getattr_or_restores_deleted_entries() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        with_gil(|py| unsafe {
            let name = crate::attr_name_bits_from_bytes(&py, b"sys").unwrap();
            let _restore = ModuleCacheRestore::new(&py, name);
            let sys = MoltObject::from_ptr(alloc_module_obj(&py, name)).bits();
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            let hook_name = crate::attr_name_bits_from_bytes(&py, b"__getattr__").unwrap();
            let hook = sys_lookup_function(&py, sys_lookup_getattr_trap as *const (), 1);
            crate::builtins::modules::molt_module_set_attr(sys, hook_name, hook);
            SYS_LOOKUP_GETATTR_CALLS.store(0, AtomicOrdering::SeqCst);
            assert!(
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_namespace_probe".as_ptr())
                    .is_null()
            );
            assert_eq!(SYS_LOOKUP_GETATTR_CALLS.load(AtomicOrdering::SeqCst), 0);
            assert!(!crate::exception_pending(&py));

            let key = crate::attr_name_bits_from_bytes(&py, b"molt_namespace_probe").unwrap();
            let stored =
                MoltObject::from_ptr(crate::alloc_tuple(&py, &[MoltObject::from_int(33).bits()]))
                    .bits();
            crate::builtins::modules::molt_module_set_attr(sys, key, stored);
            let value =
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_namespace_probe".as_ptr());
            assert!(!value.is_null());
            assert_eq!((*value).ob_refcnt, 1);
            assert_eq!(
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_namespace_probe".as_ptr()),
                value
            );
            let dict = MoltObject::from_bits(module_dict_bits(
                MoltObject::from_bits(sys).as_ptr().unwrap(),
            ))
            .as_ptr()
            .unwrap();
            let _ = crate::dict_del_in_place(&py, dict, key);
            assert!(
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_namespace_probe".as_ptr())
                    .is_null()
            );
            let replacement = MoltObject::from_int(77).bits();
            crate::builtins::modules::molt_module_set_attr(sys, key, replacement);
            let replaced =
                molt_cpython_abi::api::sys::PySys_GetObject(c"molt_namespace_probe".as_ptr());
            assert!(!replaced.is_null());
            assert_eq!(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(replaced)
                    .unwrap()
                    .bits(),
                replacement
            );
            assert_eq!(SYS_LOOKUP_GETATTR_CALLS.load(AtomicOrdering::SeqCst), 0);
            assert!(!crate::exception_pending(&py));
            for bits in [key, stored, hook_name, hook, sys] {
                dec_ref_bits(&py, bits);
            }
        });
    }

    #[test]
    fn abi_public_bytearray_constructor_roundtrips_through_python_operations() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|py| unsafe {
            use molt_cpython_abi::abi_types::{Py_buffer, PyBUF_WRITABLE};
            use molt_cpython_abi::api::{buffer, errors, refcount, strings};
            let object = strings::PyByteArray_FromStringAndSize(c"abc".as_ptr(), 3);
            assert!(!object.is_null());
            let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(object)
                .unwrap()
                .bits();
            let ptr = MoltObject::from_bits(bits).as_ptr().unwrap();
            assert_eq!(object_type_id(ptr), TYPE_ID_BYTEARRAY);
            assert_eq!(
                MoltObject::from_bits(crate::molt_len(bits)).as_int(),
                Some(3)
            );
            assert_eq!(
                MoltObject::from_bits(crate::object::ops::molt_getitem_method(
                    bits,
                    MoltObject::from_int(1).bits()
                ))
                .as_int(),
                Some(98)
            );
            crate::object::ops::molt_store_index(
                bits,
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(90).bits(),
            );
            assert!(!crate::exception_pending(&py));
            let data = strings::PyByteArray_AsString(object).cast::<u8>();
            assert_eq!(std::slice::from_raw_parts(data, 4), b"aZc\0");
            let mut view: Py_buffer = std::mem::zeroed();
            assert_eq!(
                buffer::PyObject_GetBuffer(object, &mut view, PyBUF_WRITABLE),
                0
            );
            assert_eq!(view.buf, data.cast());
            view.buf.cast::<u8>().add(2).write(b'!');
            assert_eq!(
                MoltObject::from_bits(crate::object::ops::molt_getitem_method(
                    bits,
                    MoltObject::from_int(2).bits()
                ))
                .as_int(),
                Some(33)
            );
            buffer::PyBuffer_Release(&mut view);
            let copied = strings::PyByteArray_FromObject(object);
            assert!(!copied.is_null());
            assert_ne!(strings::PyByteArray_AsString(copied).cast::<u8>(), data);
            let joined = strings::PyByteArray_Concat(object, copied);
            assert!(!joined.is_null());
            assert_eq!(
                std::slice::from_raw_parts(strings::PyByteArray_AsString(joined).cast::<u8>(), 7),
                b"aZ!aZ!\0"
            );
            assert!(!crate::object::buffer_exports::bytearray_is_exported(ptr));
            assert!(strings::PyByteArray_Concat(object, std::ptr::null_mut()).is_null());
            assert!(!crate::object::buffer_exports::bytearray_is_exported(ptr));
            errors::PyErr_Clear();
            crate::clear_exception(&py);
            for len in [0, 3] {
                let zeroed = strings::PyByteArray_FromStringAndSize(std::ptr::null(), len);
                assert!(!zeroed.is_null());
                assert_eq!(strings::PyByteArray_Size(zeroed), len);
                assert_eq!(
                    std::slice::from_raw_parts(
                        strings::PyByteArray_AsString(zeroed).cast::<u8>(),
                        len as usize + 1
                    ),
                    vec![0; len as usize + 1]
                );
                refcount::Py_DECREF(zeroed);
            }
            for value in [joined, copied, object] {
                refcount::Py_DECREF(value);
            }
            assert!(errors::PyErr_Occurred().is_null());
        });
    }

    #[test]
    fn abi_bytearray_storage_aliases_managed_exact_and_subclass_backing() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|py| unsafe {
            use crate::object::native_instance::NativePayload;
            use molt_cpython_abi::abi_types::{Py_buffer, PyBUF_WRITABLE, PyExc_BufferError};
            use molt_cpython_abi::api::{buffer, errors, refcount, strings};
            let name = MoltObject::from_ptr(alloc_string(&py, b"AbiBytearrayStorage")).bits();
            let subclass = crate::molt_class_new(name);
            dec_ref_bits(&py, name);
            crate::molt_class_set_base(subclass, NativePayload::Bytearray.owner(&py));
            crate::object::class_finish_definition(
                &py,
                MoltObject::from_bits(subclass).as_ptr().unwrap(),
            )
            .unwrap();
            for class in [NativePayload::Bytearray.owner(&py), subclass] {
                let input = MoltObject::from_ptr(alloc_bytes(&py, b"abc")).bits();
                let bits =
                    crate::call::bind::call_bind_borrowed(&py, class, None, &[input], &[], &[]);
                dec_ref_bits(&py, input);
                assert!(!crate::exception_pending(&py));
                let ptr = MoltObject::from_bits(bits).as_ptr().unwrap();
                assert_eq!(object_type_id(ptr), TYPE_ID_BYTEARRAY);
                let object = molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits);
                assert!(!object.is_null());
                assert_eq!(strings::PyByteArray_Check(object), 1);
                assert_eq!(
                    strings::PyByteArray_CheckExact(object),
                    i32::from(class != subclass)
                );
                let data = strings::PyByteArray_AsString(object).cast::<u8>();
                let mut direct_len = 0;
                assert_eq!(
                    crate::c_api::molt_bytearray_as_ptr(bits, &mut direct_len),
                    data
                );
                assert_eq!(direct_len, 3);
                assert_eq!(std::slice::from_raw_parts(data, 4), b"abc\0");
                let python_view = crate::object::ops_memoryview::molt_memoryview_new(bits);
                assert!(!crate::exception_pending(&py));
                let mut exported: Py_buffer = std::mem::zeroed();
                assert_eq!(
                    buffer::PyObject_GetBuffer(object, &mut exported, PyBUF_WRITABLE),
                    0
                );
                assert_eq!(exported.buf, data.cast());
                exported.buf.cast::<u8>().add(1).write(b'Z');
                assert_eq!(crate::object::layout::bytearray_vec_ref(ptr), b"aZc");
                assert!(
                    crate::object::buffer_exports::bytearray_mutate(&py, ptr, 3, |bytes| bytes
                        [2] =
                        b'!')
                    .is_some()
                );
                assert_eq!(std::slice::from_raw_parts(data, 4), b"aZ!\0");
                assert_eq!(strings::PyByteArray_Resize(object, 3), 0);
                for len in [0, 2, 8] {
                    assert_eq!(strings::PyByteArray_Resize(object, len), -1);
                    assert_eq!(
                        errors::PyErr_ExceptionMatches(
                            (&raw mut PyExc_BufferError).cast::<PyObject>()
                        ),
                        1
                    );
                    errors::PyErr_Clear();
                    crate::clear_exception(&py);
                    assert_eq!(strings::PyByteArray_AsString(object).cast::<u8>(), data);
                }
                buffer::PyBuffer_Release(&mut exported);
                // The independent Python memoryview is still a live counted pin.
                assert_eq!(strings::PyByteArray_Resize(object, 8), -1);
                errors::PyErr_Clear();
                crate::clear_exception(&py);
                crate::object::ops_memoryview::molt_memoryview_release(python_view);
                dec_ref_bits(&py, python_view);
                assert_eq!(strings::PyByteArray_Resize(object, 5), 0);
                assert_eq!(
                    std::slice::from_raw_parts(
                        strings::PyByteArray_AsString(object).cast::<u8>(),
                        6
                    ),
                    b"aZ!\0\0\0"
                );
                crate::object::ops_bytes::molt_bytearray_resize(
                    bits,
                    MoltObject::from_int(1).bits(),
                );
                assert!(!crate::exception_pending(&py));
                assert_eq!(
                    std::slice::from_raw_parts(
                        strings::PyByteArray_AsString(object).cast::<u8>(),
                        2
                    ),
                    b"a\0"
                );
                assert_eq!(strings::PyByteArray_Resize(object, 0), 0);
                assert_eq!(strings::PyByteArray_Size(object), 0);
                assert_eq!(*strings::PyByteArray_AsString(object), 0);
                refcount::Py_DECREF(object);
                assert!(errors::PyErr_Occurred().is_null());
            }
            dec_ref_bits(&py, subclass);
        });
    }

    #[test]
    fn cpython_abi_buffer_view_layout_matches_runtime_descriptor() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        macro_rules! assert_field {
            ($field:ident) => {
                assert_eq!(
                    std::mem::offset_of!(AbiMoltBufferView, $field),
                    std::mem::offset_of!(crate::MoltBufferView, $field),
                    concat!("MoltBufferView field offset drift: ", stringify!($field)),
                );
            };
        }

        assert_eq!(
            std::mem::size_of::<AbiMoltBufferView>(),
            std::mem::size_of::<crate::MoltBufferView>()
        );
        assert_eq!(
            std::mem::align_of::<AbiMoltBufferView>(),
            std::mem::align_of::<crate::MoltBufferView>()
        );
        assert_field!(data);
        assert_field!(len);
        assert_field!(readonly);
        assert_field!(ndim);
        assert_field!(itemsize);
        assert_field!(offset);
        assert_field!(owner);
        assert_field!(base);
        assert_field!(shape);
        assert_field!(strides);
        assert_field!(format);
    }

    #[test]
    fn pyinit_module_to_bits_accepts_static_module_def_pointer() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.static_def_module");
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"static_def_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: 0,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(
            crate::provenance::abi::expose_address(pyinit_result),
            cache_restore.name_bits,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("PyModuleDef pointer must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_static_extension_cached(cache_restore.name_bits, bits);
        assert_static_extension_qualified_identity(bits, "pkg.static_def_module");
        let def_ptr = (&mut def as *mut PyModuleDef) as usize;
        assert_eq!(
            crate::c_api::molt_module_state_find(def_ptr),
            0,
            "returned definitions use multi-phase initialization, not the single-phase registry"
        );
        with_gil(|_py| dec_ref_bits(&_py, bits));
        drop(cache_restore);
    }

    #[test]
    fn pyinit_module_to_bits_rejects_untyped_and_same_named_type_lookalikes() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.split_wasm_static_def_module");
        let mut app_moduledef_type: PyTypeObject = unsafe { std::mem::zeroed() };
        app_moduledef_type.tp_name = c"moduledef".as_ptr();
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &mut app_moduledef_type,
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"split_wasm_static_def_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: 0,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        for marker in [ptr::null_mut(), &raw mut app_moduledef_type] {
            def.m_base.ob_base.ob_type = marker;
            def.m_base.ob_base.ob_refcnt = 1;
            let bits = molt_cpython_abi_pyinit_module_to_bits(
                crate::provenance::abi::expose_address(&mut def as *mut PyModuleDef),
                cache_restore.name_bits,
            );
            assert!(MoltObject::from_bits(bits).is_none());
            assert_eq!(pending_exception_type_for_assertion(), "SystemError");
            assert!(
                pending_exception_message_for_assertion()
                    .contains("did not return an extension module")
            );
            with_gil(|py| crate::clear_exception(&py));
            assert!(
                MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(
                    cache_restore.name_bits
                ))
                .is_none()
            );
        }
    }

    unsafe extern "C" fn canonical_exec_records_module(module_obj: *mut PyObject) -> c_int {
        if module_obj.is_null() {
            return -1;
        }
        let module_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(module_obj)
            .map(|value| value.bits())
            .unwrap_or(0);
        let Some(module_ptr) = MoltObject::from_bits(module_bits).as_ptr() else {
            return -1;
        };
        if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
            return -1;
        }
        let name_bits = CANONICAL_EXEC_MODULE_NAME_BITS.load(AtomicOrdering::Relaxed);
        let cached_bits = crate::builtins::modules::molt_module_cache_get(name_bits);
        CANONICAL_EXEC_SAW_PUBLICATION.store(cached_bits == module_bits, AtomicOrdering::Relaxed);
        if !MoltObject::from_bits(cached_bits).is_none() {
            with_gil(|py| dec_ref_bits(&py, cached_bits));
        }
        CANONICAL_EXEC_MODULE_BITS.store(module_bits, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_VIEW.store(module_obj.addr(), AtomicOrdering::Relaxed);
        0
    }

    #[test]
    fn pyinit_module_to_bits_executes_canonically_initialized_module_definition() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore =
            static_extension_cache_restore("pkg.source_recompiled_structural_module");
        CANONICAL_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_NAME_BITS.store(cache_restore.name_bits, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_VIEW.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_SAW_PUBLICATION.store(false, AtomicOrdering::Relaxed);
        let mut slots = [
            PyModuleDef_Slot {
                slot: 2,
                value: canonical_exec_records_module as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"source_recompiled_structural_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: 0,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let initialized = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&raw mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(
            crate::provenance::abi::expose_address(initialized),
            cache_restore.name_bits,
        );
        let module_ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("initialized PyModuleDef must convert to a Molt module");

        assert_eq!(unsafe { object_type_id(module_ptr) }, TYPE_ID_MODULE);
        assert_eq!(
            CANONICAL_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            bits
        );
        assert!(
            CANONICAL_EXEC_SAW_PUBLICATION.load(AtomicOrdering::Relaxed),
            "the caller-qualified cache entry must exist before Py_mod_exec",
        );
        assert_static_extension_cached(cache_restore.name_bits, bits);
        assert_static_extension_qualified_identity(bits, "pkg.source_recompiled_structural_module");
        let original_view = CANONICAL_EXEC_MODULE_VIEW.swap(0, AtomicOrdering::Relaxed);
        assert_ne!(original_view, 0);
        let view =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        assert_eq!(
            view.addr(),
            original_view,
            "Py_mod_exec C view must survive commit"
        );
        CANONICAL_EXEC_MODULE_NAME_BITS.store(0, AtomicOrdering::Relaxed);
        let def_ptr = (&mut def as *mut PyModuleDef) as usize;
        assert_eq!(crate::c_api::molt_module_state_find(def_ptr), 0);
        with_gil(|py| dec_ref_bits(&py, bits));
        drop(cache_restore);
    }

    #[test]
    fn dynamic_extension_hook_uses_the_same_qualified_publication_transaction() {
        unsafe extern "C" fn init() -> *mut PyObject {
            static mut DEF: PyModuleDef = PyModuleDef {
                m_base: PyModuleDef_Base {
                    ob_base: PyObject {
                        ob_refcnt: 1,
                        ob_type: std::ptr::null_mut(),
                    },
                    m_init: None,
                    m_index: 0,
                    m_copy: std::ptr::null_mut(),
                },
                m_name: c"dynamic_extension".as_ptr(),
                m_doc: std::ptr::null(),
                m_size: -1,
                m_methods: std::ptr::null_mut(),
                m_slots: std::ptr::null_mut(),
                m_traverse: std::ptr::null_mut(),
                m_clear: std::ptr::null_mut(),
                m_free: std::ptr::null_mut(),
            };
            unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&raw mut DEF, 0) }
        }
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.dynamic_extension");
        let origin_bits = with_gil(|py| {
            MoltObject::from_ptr(alloc_string(&py, b"/extensions/dynamic.so")).bits()
        });
        let result = unsafe {
            (molt_cpython_abi::hooks::hooks_or_stubs().initialize_extension)(
                init,
                cache_restore.name_bits,
                origin_bits,
                MoltObject::none().bits(),
                false,
            )
        };
        let molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) = result.decode() else {
            panic!("dynamic hook must return the published module");
        };
        let module =
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits) };
        assert_static_extension_cached(cache_restore.name_bits, bits);
        assert_static_extension_qualified_identity(bits, "pkg.dynamic_extension");
        unsafe {
            let file =
                molt_cpython_abi::api::object::PyObject_GetAttrString(module, c"__file__".as_ptr());
            assert!(
                !file.is_null(),
                "dynamic extension must expose its origin as __file__"
            );
            let file_utf8 = molt_cpython_abi::api::strings::PyUnicode_AsUTF8(file);
            assert!(!file_utf8.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(file_utf8).to_bytes(),
                b"/extensions/dynamic.so",
            );
            molt_cpython_abi::api::refcount::Py_DECREF(file);
        }
        assert_eq!(
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(module)
                .map(|value| value.bits()),
            Some(bits),
            "the original dynamic-extension C view must survive publication",
        );
        let def = unsafe { molt_cpython_abi::api::modules::PyModule_GetDef(module) };
        assert!(!def.is_null());
        assert_eq!(
            unsafe { molt_cpython_abi::api::modules::PyState_RemoveModule(def) },
            0
        );
        with_gil(|py| {
            dec_ref_bits(&py, bits);
            dec_ref_bits(&py, origin_bits);
        });
        drop(cache_restore);
    }

    unsafe extern "C" fn canonical_exec_sets_runtime_import_error(
        module_obj: *mut PyObject,
    ) -> c_int {
        if unsafe { canonical_exec_records_module(module_obj) } != 0 {
            return -1;
        }
        if !CANONICAL_EXEC_FAIL_ONCE.swap(false, AtomicOrdering::Relaxed) {
            return 0;
        }
        let import_error =
            with_gil(|_py| crate::exception_type_bits_from_name(&_py, "ImportError"));
        let message = b"numpy.core._multiarray_umath._ARRAY_API capsule import failed";
        unsafe {
            crate::c_api::molt_err_set(import_error, message.as_ptr(), message.len() as u64);
        }
        -1
    }

    #[test]
    fn r0_static_extension_moduledef_exec_failure_rolls_back_and_retries() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.moduledef_exec_error_module");
        CANONICAL_EXEC_MODULE_NAME_BITS.store(cache_restore.name_bits, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_VIEW.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_SAW_PUBLICATION.store(false, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_FAIL_ONCE.store(true, AtomicOrdering::Relaxed);
        let mut slots = [
            PyModuleDef_Slot {
                slot: 2,
                value: canonical_exec_sets_runtime_import_error as *mut c_void,
            },
            PyModuleDef_Slot {
                slot: 0,
                value: std::ptr::null_mut(),
            },
        ];
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: c"moduledef_exec_error_module".as_ptr(),
            m_doc: std::ptr::null(),
            m_size: 0,
            m_methods: std::ptr::null_mut(),
            m_slots: slots.as_mut_ptr(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(
            crate::provenance::abi::expose_address(pyinit_result),
            cache_restore.name_bits,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        assert!(
            CANONICAL_EXEC_SAW_PUBLICATION.load(AtomicOrdering::Relaxed),
            "the failed exec must observe its module published before callback",
        );
        let message = pending_exception_message_for_assertion();
        assert_eq!(
            message,
            "numpy.core._multiarray_umath._ARRAY_API capsule import failed"
        );
        assert_eq!(
            crate::c_api::molt_module_state_find((&mut def as *mut PyModuleDef) as usize),
            0,
            "failed Py_mod_exec must unregister the def->module state before retry"
        );
        assert!(
            MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(
                cache_restore.name_bits,
            ))
            .is_none(),
            "failed Py_mod_exec must remove its caller-qualified cache entry",
        );

        CANONICAL_EXEC_MODULE_BITS.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_MODULE_VIEW.store(0, AtomicOrdering::Relaxed);
        CANONICAL_EXEC_SAW_PUBLICATION.store(false, AtomicOrdering::Relaxed);
        let retry = molt_cpython_abi_pyinit_module_to_bits(
            crate::provenance::abi::expose_address(pyinit_result),
            cache_restore.name_bits,
        );
        assert!(MoltObject::from_bits(retry).as_ptr().is_some());
        assert!(
            CANONICAL_EXEC_SAW_PUBLICATION.load(AtomicOrdering::Relaxed),
            "the retry must publish its new module before exec",
        );
        assert_eq!(
            CANONICAL_EXEC_MODULE_BITS.load(AtomicOrdering::Relaxed),
            retry
        );
        assert_static_extension_cached(cache_restore.name_bits, retry);
        assert_static_extension_qualified_identity(retry, "pkg.moduledef_exec_error_module");
        let original_view = CANONICAL_EXEC_MODULE_VIEW.swap(0, AtomicOrdering::Relaxed);
        assert_eq!(
            unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(retry) }
                .addr(),
            original_view,
            "the retry must retain the C view seen by Py_mod_exec",
        );
        CANONICAL_EXEC_MODULE_NAME_BITS.store(0, AtomicOrdering::Relaxed);
        with_gil(|py| dec_ref_bits(&py, retry));
        drop(cache_restore);
    }

    unsafe extern "C" fn fastcall_null_with_type_error(
        _self_obj: *mut PyObject,
        _args: *mut *mut PyObject,
        _nargs: Py_ssize_t,
    ) -> *mut PyObject {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_TypeError).cast::<PyObject>(),
                c"numpy fastcall detail".as_ptr(),
            );
        }
        std::ptr::null_mut()
    }

    unsafe extern "C" fn fastcall_null_without_exception(
        _self_obj: *mut PyObject,
        _args: *mut *mut PyObject,
        _nargs: Py_ssize_t,
    ) -> *mut PyObject {
        std::ptr::null_mut()
    }

    #[test]
    fn c_api_cfunction_module_metadata_tracks_assignment_and_deletion() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        unsafe {
            use molt_cpython_abi::api::refcount::OwnedPyObject;
            use molt_cpython_abi::api::{errors, modules, object, strings};
            let module_owner =
                OwnedPyObject::from_owned(modules::PyModule_New(c"pkg.method_owner".as_ptr()));
            let module = module_owner.as_ptr();
            let module_name_owner = OwnedPyObject::from_owned(strings::PyUnicode_FromString(
                c"pkg.method_owner".as_ptr(),
            ));
            let module_name = module_name_owner.as_ptr();
            assert!(!module.is_null() && !module_name.is_null());
            let mut method = PyMethodDef {
                ml_name: c"method".as_ptr(),
                ml_meth: Some(gil_bench_noargs),
                ml_flags: METH_NOARGS,
                ml_doc: std::ptr::null(),
            };
            let function_owner = OwnedPyObject::from_owned(object::PyCFunction_NewEx(
                &raw mut method,
                module,
                module_name,
            ));
            let function = function_owner.as_ptr();
            assert!(!function.is_null());
            let physical = function.cast::<molt_cpython_abi::abi_types::PyCFunctionObject>();
            assert_eq!((*physical).m_module, module_name);

            let initial_owner = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                function,
                c"__module__".as_ptr(),
            ));
            let initial = initial_owner.as_ptr();
            assert!(!initial.is_null());
            let initial_name = strings::PyUnicode_AsUTF8(initial);
            assert!(!initial_name.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(initial_name).to_bytes(),
                b"pkg.method_owner"
            );
            drop(initial_owner);

            let replacement_owner =
                OwnedPyObject::from_owned(strings::PyUnicode_FromString(c"pkg.rebound".as_ptr()));
            let replacement = replacement_owner.as_ptr();
            assert!(!replacement.is_null());
            assert_eq!(
                object::PyObject_SetAttrString(function, c"__module__".as_ptr(), replacement),
                0,
            );
            let rebound_owner = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                function,
                c"__module__".as_ptr(),
            ));
            let rebound = rebound_owner.as_ptr();
            assert!(!rebound.is_null());
            let rebound_name = strings::PyUnicode_AsUTF8(rebound);
            assert!(!rebound_name.is_null());
            assert_eq!(
                std::ffi::CStr::from_ptr(rebound_name).to_bytes(),
                b"pkg.rebound"
            );
            assert_eq!((*physical).m_module, replacement);
            drop(rebound_owner);

            // Assignment of Python None owns a non-null Py_None. Deletion
            // owns no C pointer, even though both public reads return None.
            let none = (&raw mut molt_cpython_abi::abi_types::Py_None).cast();
            assert_eq!(
                object::PyObject_SetAttrString(function, c"__module__".as_ptr(), none),
                0,
            );
            assert_eq!((*physical).m_module, none);
            let assigned_none = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                function,
                c"__module__".as_ptr(),
            ));
            assert_eq!(assigned_none.as_ptr(), none);
            assert!(errors::PyErr_Occurred().is_null());
            drop(assigned_none);

            assert_eq!(
                object::PyObject_SetAttrString(function, c"__module__".as_ptr(), ptr::null_mut()),
                0,
            );
            let deleted_owner = OwnedPyObject::from_owned(object::PyObject_GetAttrString(
                function,
                c"__module__".as_ptr(),
            ));
            let deleted = deleted_owner.as_ptr();
            assert_eq!(
                deleted,
                (&raw mut molt_cpython_abi::abi_types::Py_None).cast()
            );
            assert!((*physical).m_module.is_null());
            assert!(errors::PyErr_Occurred().is_null());
            drop(deleted_owner);
            drop(replacement_owner);
            drop(function_owner);
            drop(module_name_owner);
            drop(module_owner);
        }
    }

    #[test]
    fn cext_callable_allocation_failure_never_publishes_an_abi_fallback() {
        use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                set_tracker(Box::new(UnlimitedTracker));
            }
        }
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();
        with_gil(|py| {
            let _ = crate::builtin_classes(&py);
        });
        let before = with_cext_callable_registry(<[CExtCallable]>::len);
        let mut method = PyMethodDef {
            ml_name: c"allocation_failure".as_ptr(),
            ml_meth: Some(gil_bench_noargs),
            ml_flags: METH_NOARGS,
            ml_doc: std::ptr::null(),
        };
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(0),
            ..Default::default()
        })));
        let restore = RestoreTracker;
        let result = unsafe {
            molt_cpython_abi::api::object::PyCFunction_NewEx(
                &mut method,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        drop(restore);
        assert!(
            result.is_null(),
            "allocation failure must not return an ABI-only callable"
        );
        assert_eq!(with_cext_callable_registry(<[CExtCallable]>::len), before);
        // The zero-allocation resource limit uses the runtime's emergency
        // MemoryError channel, not a fabricated heap exception/C indicator.
        // The failure boundary must preserve that channel without allocating
        // an ABI-only fallback callable or claiming a registry entry.
        assert_eq!(unsafe { hook_exception_pending() }, 1);
        assert!(cpython_error_is_pending());
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        // Once an actual exception instance exists, construction must preserve
        // that exact instance as well as the allocation-free emergency case.
        with_gil(|py| {
            crate::raise_exception::<u64>(&py, "MemoryError", "preserved callable failure");
        });
        let expected = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(MoltObject::from_bits(expected).as_ptr().is_some());
        let result = unsafe {
            molt_cpython_abi::api::object::PyCFunction_NewEx(
                &mut method,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert!(result.is_null());
        assert_eq!(with_cext_callable_registry(<[CExtCallable]>::len), before);
        let observed = crate::builtins::exceptions::molt_exception_last_pending();
        assert_eq!(observed, expected);
        let _ = crate::molt_exception_clear();
        with_gil(|py| {
            dec_ref_bits(&py, observed);
            dec_ref_bits(&py, expected);
        });
    }

    #[test]
    fn cext_null_result_propagates_pending_exception() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();
        let mapped_type_error = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj((&raw mut PyExc_TypeError).cast::<PyObject>())
            .expect("TypeError singleton must retain its runtime binding")
            .bits();
        with_gil(|py| {
            assert_eq!(
                mapped_type_error,
                crate::exception_type_bits_from_name(&py, "TypeError"),
                "C exception singleton must resolve to the canonical runtime class"
            );
            let class_ptr = crate::obj_from_bits(mapped_type_error)
                .as_ptr()
                .expect("TypeError binding must be a heap class");
            assert_eq!(
                unsafe { crate::object_type_id(class_ptr) },
                crate::TYPE_ID_TYPE
            );
            assert!(
                crate::issubclass_bits(
                    mapped_type_error,
                    crate::builtin_classes(&py).base_exception
                ),
                "canonical TypeError must retain its BaseException ancestry"
            );
        });
        let method_bits = unsafe {
            hook_register_c_function(
                crate::provenance::abi::expose_function_address(
                    fastcall_null_with_type_error as *const (),
                ),
                METH_FASTCALL,
                MoltObject::none().bits(),
                true,
                MoltObject::none().bits(),
                b"masked_fastcall".as_ptr(),
                b"masked_fastcall".len(),
            )
        };
        let method_ptr = MoltObject::from_bits(method_bits)
            .as_ptr()
            .expect("registered C extension callable must be a heap function");
        assert_eq!(
            unsafe { crate::object::layout::function_call_target_ptr(method_ptr) },
            molt_cpython_abi_cext_call_trampoline_admitted as *const (),
            "registered extension callables must use the admitted internal trampoline"
        );

        let execution = RuntimeExecutionGuard::enter();
        let out_bits = unsafe {
            crate::call::function::call_function_obj_bound_vec(&execution.token(), method_bits, &[])
        };
        drop(execution);

        assert_eq!(out_bits, 0);
        assert_eq!(pending_exception_type_for_assertion(), "TypeError");
        let message = pending_exception_message_for_assertion();
        assert!(message.contains("numpy fastcall detail"), "{message}");
        assert!(
            !message.contains("returned NULL for convention"),
            "{message}"
        );
        unsafe { hook_dec_ref(method_bits) };
    }

    #[test]
    fn cext_null_result_without_exception_raises_system_error() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();
        let method_bits = unsafe {
            hook_register_c_function(
                crate::provenance::abi::expose_function_address(
                    fastcall_null_without_exception as *const (),
                ),
                METH_FASTCALL,
                MoltObject::none().bits(),
                true,
                MoltObject::none().bits(),
                b"broken_fastcall".as_ptr(),
                b"broken_fastcall".len(),
            )
        };

        let execution = RuntimeExecutionGuard::enter();
        let out_bits = unsafe {
            crate::call::function::call_function_obj_bound_vec(&execution.token(), method_bits, &[])
        };
        drop(execution);

        assert_eq!(out_bits, 0);
        assert_eq!(pending_exception_type_for_assertion(), "SystemError");
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("returned NULL without setting an exception"),
            "{message}"
        );
        assert!(message.contains("0x80"), "{message}");
        unsafe { hook_dec_ref(method_bits) };
    }

    #[test]
    fn pyinit_module_to_bits_reports_static_pyinit_error_state() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.pyinit_error");
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast::<PyObject>(),
                c"missing PyArray primitive".as_ptr(),
            );
        }

        let bits = molt_cpython_abi_pyinit_module_to_bits(0, cache_restore.name_bits);

        assert!(MoltObject::from_bits(bits).is_none());
        assert_eq!(pending_exception_type_for_assertion(), "RuntimeError");
        assert_eq!(
            pending_exception_message_for_assertion(),
            "missing PyArray primitive"
        );
    }

    #[test]
    fn pyinit_module_to_bits_reports_invalid_handle_error_state() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        let cache_restore = static_extension_cache_restore("pkg.invalid_module_def");
        let mut def = PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: std::ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: std::ptr::null_mut(),
            },
            m_name: std::ptr::null(),
            m_doc: std::ptr::null(),
            m_size: -1,
            m_methods: std::ptr::null_mut(),
            m_slots: std::ptr::null_mut(),
            m_traverse: std::ptr::null_mut(),
            m_clear: std::ptr::null_mut(),
            m_free: std::ptr::null_mut(),
        };
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_RuntimeError).cast::<PyObject>(),
                c"module definition missing name".as_ptr(),
            );
        }

        let pyinit_result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };
        let bits = molt_cpython_abi_pyinit_module_to_bits(
            crate::provenance::abi::expose_address(pyinit_result),
            cache_restore.name_bits,
        );

        assert!(MoltObject::from_bits(bits).is_none());
        let message = pending_exception_message_for_assertion();
        assert!(
            message.contains("extension PyInit returned an invalid module definition"),
            "{message}"
        );
        assert!(
            message.contains("module definition missing name"),
            "{message}"
        );
    }

    // ── PySet_* CPython ABI hook coverage ────────────────────────────────────
    //
    // These exercise the real set primitive end to end: the ABI `PySet_*`
    // functions route through the registered `set_*` hooks to the runtime set
    // authority (`crate::c_api::PySet_*` → hashed set object). They prove
    // membership, dedup, size, and discard semantics match CPython
    // (docs.python.org/3/c-api/set.html, Objects/setobject.c), not the prior
    // fail-closed sentinels.

    /// Wrap raw runtime handle bits into a bridge-managed `PyObject*` the ABI
    /// set functions accept as an argument.
    fn bridge_pyobj_from_bits(bits: u64) -> *mut PyObject {
        // SAFETY: handle_to_pyobj materializes a bridge PyObject entry for a
        // live runtime handle; `bits` here always comes from a hook that just
        // allocated the object, so it is valid for the bridge round-trip.
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
    }

    fn bridge_int_pyobj(value: i64) -> *mut PyObject {
        let bits = unsafe { hook_int_from_i64(value) };
        assert!(bits != 0, "hook_int_from_i64 must allocate an int");
        bridge_pyobj_from_bits(bits)
    }

    fn release_bridge_pyobj(ptr: *mut PyObject) {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_pyobj(ptr);
    }

    #[test]
    fn pyset_add_contains_size_discard_round_trip() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // PySet_New(NULL) creates a real, empty runtime set — not a list.
        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(std::ptr::null_mut()) };
        assert!(!set.is_null(), "PySet_New(NULL) must return a set object");
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Check(set) },
            1,
            "PySet_New must produce a set (PySet_Check == 1)"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            0,
            "a fresh set is empty"
        );

        let key7 = bridge_int_pyobj(7);
        let key9 = bridge_int_pyobj(9);

        // Absent before add.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            0,
            "key must be absent before add"
        );

        // Add 7 → success (0), then present (1), size 1.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key7) },
            0,
            "PySet_Add success returns 0"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            1,
            "PySet_Contains after add returns 1"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1,
            "size is 1 after one add"
        );

        // Dedup: adding an equal value twice keeps size at 1.
        let key7_dup = bridge_int_pyobj(7);
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key7_dup) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1,
            "dedup: adding an equal element must not grow the set"
        );

        // Add 9 → size 2.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, key9) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            2
        );

        // Discard present key → 1, then absent, size back to 1.
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(set, key7) },
            1,
            "PySet_Discard of a present key returns 1"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(set, key7) },
            0,
            "discarded key is absent"
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            1
        );

        // Discard absent key → 0 (no error, no KeyError).
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(set, key7) },
            0,
            "PySet_Discard of an absent key returns 0"
        );
        assert!(
            !with_gil(|_py| crate::exception_pending(&_py)),
            "PySet_Discard of an absent key must not raise: {}",
            pending_exception_message_for_assertion()
        );

        release_bridge_pyobj(key7);
        release_bridge_pyobj(key7_dup);
        release_bridge_pyobj(key9);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_new_from_iterable_dedups() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // Build a list [3, 3, 5] via the runtime list authority, bridge it, and
        // feed it to PySet_New — the result must be a set of size 2 (deduped).
        let list_bits = unsafe { hook_alloc_list() };
        assert!(list_bits != 0);
        let three = unsafe { hook_int_from_i64(3) };
        let five = unsafe { hook_int_from_i64(5) };
        unsafe {
            hook_list_append(list_bits, three, std::ptr::null_mut());
            hook_list_append(list_bits, three, std::ptr::null_mut());
            hook_list_append(list_bits, five, std::ptr::null_mut());
        }
        let list = bridge_pyobj_from_bits(list_bits);

        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(list) };
        assert!(
            !set.is_null(),
            "PySet_New(iterable) must succeed: {}",
            pending_exception_message_for_assertion()
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Check(set) },
            1
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(set) },
            2,
            "PySet_New from [3,3,5] dedups to {{3,5}} (size 2)"
        );

        release_bridge_pyobj(list);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_ops_fail_closed_on_non_set() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        // A dict is a bridge-managed object but not a set: every mutating/query
        // op must fail closed with the CPython error sentinel + SystemError,
        // never silently succeed.
        let dict_bits = unsafe { hook_alloc_dict() };
        assert!(dict_bits != 0);
        let not_a_set = bridge_pyobj_from_bits(dict_bits);
        let key = bridge_int_pyobj(1);

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Size(not_a_set) },
            -1,
            "PySet_Size on a non-set returns -1"
        );
        assert!(
            with_gil(|_py| crate::exception_pending(&_py)),
            "PySet_Size on a non-set must set an exception"
        );
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Contains(not_a_set, key) },
            -1,
            "PySet_Contains on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(not_a_set, key) },
            -1,
            "PySet_Add on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Discard(not_a_set, key) },
            -1,
            "PySet_Discard on a non-set returns -1"
        );
        assert!(with_gil(|_py| crate::exception_pending(&_py)));
        let _ = crate::molt_exception_clear();

        release_bridge_pyobj(key);
        release_bridge_pyobj(not_a_set);
    }

    #[test]
    fn pyset_add_unhashable_key_raises_typeerror() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let set = unsafe { molt_cpython_abi::api::sequences::PySet_New(std::ptr::null_mut()) };
        assert!(!set.is_null());

        // A list is unhashable — PySet_Add must raise TypeError and return -1,
        // matching CPython, not silently drop the element.
        let list_bits = unsafe { hook_alloc_list() };
        assert!(list_bits != 0);
        let unhashable = bridge_pyobj_from_bits(list_bits);

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PySet_Add(set, unhashable) },
            -1,
            "PySet_Add of an unhashable key returns -1"
        );
        let message = pending_exception_message_for_assertion();
        assert!(
            message.to_lowercase().contains("unhashable"),
            "PySet_Add of an unhashable key must raise a TypeError mentioning 'unhashable', got: {message}"
        );

        release_bridge_pyobj(unhashable);
        release_bridge_pyobj(set);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pyset_stub_hooks_fail_closed() {
        // Mutation guard: the pre-init STUB_HOOKS set ops must return the
        // CPython error sentinel (0 / -1). If a future edit swapped a stub for a
        // fake-success value, this catches it before it could mask a missing
        // runtime authority.
        use molt_cpython_abi::hooks::STUB_HOOKS;
        assert_eq!(
            unsafe { (STUB_HOOKS.set_new)(BorrowedHandleResult::missing(), false) },
            0
        );
        assert_eq!(unsafe { (STUB_HOOKS.set_size)(0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_contains)(0, 0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_add)(0, 0) }, -1);
        assert_eq!(unsafe { (STUB_HOOKS.set_discard)(0, 0) }, -1);
    }

    #[test]
    fn abi_list_projection_retains_heap_fill_and_promotes_inline_storage() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|py| unsafe {
            let fill = crate::int_bits_from_i64(&py, 1_i64 << 62);
            let ptr = crate::object::builders::alloc_list_int_from_fill(&py, 3, fill).unwrap();
            let bits = MoltObject::from_ptr(ptr).bits();
            assert_eq!(object_type_id(ptr), TYPE_ID_LIST);
            assert_eq!(hook_try_mark_abi_view(bits, 1), 1);
            assert_eq!(borrowed_bits(hook_list_item(bits, 0)), Some(fill));
            assert_eq!(borrowed_bits(hook_list_item(bits, 2)), Some(fill));
            assert_eq!(hook_try_mark_abi_view(bits, 0), 1);
            dec_ref_bits(&py, bits);
            assert_eq!(
                (*header_from_obj_ptr(MoltObject::from_bits(fill).as_ptr().unwrap()))
                    .ref_count_snapshot(),
                1
            );
            dec_ref_bits(&py, fill);

            let ptr = crate::object::builders::alloc_list_int_from_raw_slice(&py, &[1, 2]).unwrap();
            let bits = MoltObject::from_ptr(ptr).bits();
            assert_eq!(hook_try_mark_abi_view(bits, 1), 1);
            assert_eq!(object_type_id(ptr), TYPE_ID_LIST);
            assert_eq!(
                borrowed_bits(hook_list_item(bits, 1)),
                Some(MoltObject::from_int(2).bits())
            );
            assert_eq!(hook_try_mark_abi_view(bits, 0), 1);
            dec_ref_bits(&py, bits);
        });
    }

    #[test]
    fn hook_list_family_supports_specialized_int_and_bool_storage() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|_py| unsafe {
            let int_ptr = crate::object::builders::alloc_list_int_from_raw_slice(&_py, &[1, 2, 3])
                .expect("specialized int-list allocation");
            let int_bits = MoltObject::from_ptr(int_ptr).bits();
            assert_eq!(hook_classify_heap(int_bits), MoltTypeTag::List as u8);
            hook_list_append(
                int_bits,
                MoltObject::from_int(99).bits(),
                std::ptr::null_mut(),
            );
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST_INT);
            assert_eq!(hook_list_len(int_bits), 4);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 3))
                    .and_then(|bits| MoltObject::from_bits(bits).as_int()),
                Some(99)
            );

            let old = match hook_list_set(int_bits, 1, MoltObject::from_int(42).bits()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => bits,
                _ => panic!("specialized list store failed"),
            };
            assert_eq!(MoltObject::from_bits(old).as_int(), Some(2));
            dec_ref_bits(&_py, old);
            assert_eq!(object_type_id(int_ptr), TYPE_ID_LIST);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 1))
                    .and_then(|bits| MoltObject::from_bits(bits).as_int()),
                Some(42)
            );

            let bool_ptr = crate::object::builders::alloc_list_bool_from_raw_slice(&_py, &[1, 0])
                .expect("specialized bool-list allocation");
            let bool_bits = MoltObject::from_ptr(bool_ptr).bits();
            assert_eq!(hook_classify_heap(bool_bits), MoltTypeTag::List as u8);
            hook_list_append(
                bool_bits,
                MoltObject::from_bool(true).bits(),
                std::ptr::null_mut(),
            );
            assert_eq!(object_type_id(bool_ptr), TYPE_ID_LIST_BOOL);
            assert_eq!(hook_list_len(bool_bits), 3);
            assert_eq!(
                borrowed_bits(hook_list_item(bool_bits, 2))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(true)
            );

            assert_eq!(
                hook_list_set_slice(
                    int_bits,
                    0,
                    2,
                    [
                        MoltObject::from_bool(true).bits(),
                        MoltObject::from_bool(false).bits(),
                        MoltObject::from_bool(true).bits()
                    ]
                    .as_ptr(),
                    3,
                    std::ptr::null(),
                    0
                ),
                0
            );
            assert_eq!(hook_list_len(int_bits), 5);
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 0))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(true)
            );
            assert_eq!(
                borrowed_bits(hook_list_item(int_bits, 1))
                    .and_then(|bits| MoltObject::from_bits(bits).as_bool()),
                Some(false)
            );

            hook_tuple_set(int_bits, 0, MoltObject::from_int(7).bits(), ptr::null_mut());
            assert_eq!(borrowed_bits(hook_tuple_item(int_bits, 0)), None);
            dec_ref_bits(&_py, bool_bits);
            dec_ref_bits(&_py, int_bits);
        });
    }

    #[test]
    fn published_list_scalar_mutations_update_runtime_and_physical_views() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let list_bits = unsafe { hook_alloc_list() };
        assert_ne!(list_bits, 0);
        assert_eq!(
            unsafe {
                hook_list_append(
                    list_bits,
                    MoltObject::from_int(1).bits(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            unsafe {
                hook_list_append(
                    list_bits,
                    MoltObject::from_int(2).bits(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let list = bridge_pyobj_from_bits(list_bits);
        let physical = list.cast::<PyListObject>();

        let physical_values = |expected_len: usize| {
            assert_eq!(
                unsafe { (*physical).ob_base.ob_size },
                expected_len as isize
            );
            (0..expected_len)
                .map(|index| {
                    let item = unsafe {
                        molt_cpython_abi::api::sequences::PyList_GetItem(list, index as isize)
                    };
                    assert!(!item.is_null());
                    unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(item) }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(physical_values(2), [1, 2]);

        crate::molt_list_append(list_bits, MoltObject::from_int(3).bits());
        assert_eq!(physical_values(3), [1, 2, 3]);

        crate::molt_list_insert(
            list_bits,
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(9).bits(),
        );
        assert_eq!(physical_values(4), [1, 9, 2, 3]);

        crate::molt_store_index(
            list_bits,
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(8).bits(),
        );
        assert_eq!(physical_values(4), [1, 9, 8, 3]);

        crate::molt_list_reverse(list_bits);
        assert_eq!(physical_values(4), [3, 8, 9, 1]);

        let popped = crate::molt_list_pop(list_bits, MoltObject::none().bits());
        assert_eq!(MoltObject::from_bits(popped).as_int(), Some(1));
        with_gil(|_py| dec_ref_bits(&_py, popped));
        assert_eq!(physical_values(3), [3, 8, 9]);

        crate::molt_list_clear(list_bits);
        assert!(physical_values(0).is_empty());
        assert!(unsafe { (*physical).ob_item.is_null() });

        let heap_bits = with_gil(|_py| {
            let ptr = alloc_string(&_py, b"heap edge");
            assert!(!ptr.is_null());
            MoltObject::from_ptr(ptr).bits()
        });
        crate::molt_list_append(list_bits, heap_bits);
        with_gil(|_py| dec_ref_bits(&_py, heap_bits));
        let list_ptr = MoltObject::from_bits(list_bits).as_ptr().unwrap();
        assert_eq!(
            unsafe { crate::object::seq_access::tracked_heap_edge_count(list_ptr) },
            Some(1)
        );
        assert_ne!(
            unsafe { (*header_from_obj_ptr(list_ptr)).load_metadata_flags() }
                & crate::object::HEADER_FLAG_CONTAINS_REFS,
            0
        );
        crate::molt_store_index(
            list_bits,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(42).bits(),
        );
        assert_eq!(
            unsafe { crate::object::seq_access::tracked_heap_edge_count(list_ptr) },
            Some(0)
        );
        assert_eq!(
            unsafe { (*header_from_obj_ptr(list_ptr)).load_metadata_flags() }
                & crate::object::HEADER_FLAG_CONTAINS_REFS,
            0
        );
        assert_eq!(physical_values(1), [42]);
        crate::molt_list_clear(list_bits);

        release_bridge_pyobj(list);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn pylist_append_preserves_exact_c_origin_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        let list_bits = unsafe { hook_alloc_list() };
        assert_ne!(list_bits, 0);
        let list = bridge_pyobj_from_bits(list_bits);
        let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        assert!(!item.is_null());

        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PyList_Append(list, item) },
            0
        );
        assert_eq!(
            unsafe { molt_cpython_abi::api::sequences::PyList_GetItem(list, 0) },
            item,
            "PyList_Append must retain the originating object, not rematerialize an equal scalar"
        );

        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(item) };
        crate::molt_list_clear(list_bits);
        release_bridge_pyobj(list);
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn projected_sequence_family_preserves_non_small_c_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        use molt_cpython_abi::api::abstract_sequence::{
            _PyList_Extend, PySequence_Concat, PySequence_Contains, PySequence_Count,
            PySequence_Fast, PySequence_Fast_ITEMS, PySequence_InPlaceConcat,
            PySequence_InPlaceRepeat, PySequence_Index, PySequence_List, PySequence_Repeat,
            PySequence_Tuple,
        };
        use molt_cpython_abi::api::sequences::{
            PyList_Append, PyList_AsTuple, PyList_GetItem, PyList_GetSlice, PyList_Insert,
            PyList_New, PyList_Reverse, PyList_SetSlice, PyList_Size, PyList_Sort, PyTuple_GetItem,
            PyTuple_GetSlice, PyTuple_New, PyTuple_SetItem,
        };

        let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2000) };
        assert!(!first.is_null() && !second.is_null());

        let assert_list = |list: *mut PyObject, expected: &[*mut PyObject]| {
            assert_eq!(unsafe { PyList_Size(list) }, expected.len() as isize);
            for (index, &pointer) in expected.iter().enumerate() {
                assert_eq!(
                    unsafe { PyList_GetItem(list, index as isize) },
                    pointer,
                    "physical identity drifted at list index {index}"
                );
            }
        };

        let list = unsafe { PyList_New(0) };
        assert!(!list.is_null());
        assert_eq!(unsafe { PyList_Append(list, first) }, 0);
        assert_eq!(unsafe { PyList_Insert(list, 0, second) }, 0);
        assert_list(list, &[second, first]);
        assert_eq!(unsafe { PyList_Reverse(list) }, 0);
        assert_list(list, &[first, second]);
        assert_eq!(unsafe { PyList_Sort(list) }, 0);
        assert_list(list, &[first, second]);

        let slice = unsafe { PyList_GetSlice(list, 0, 2) };
        assert!(!slice.is_null());
        assert_list(slice, &[first, second]);

        let concat = unsafe { PySequence_Concat(list, slice) };
        assert!(!concat.is_null());
        assert_list(concat, &[first, second, first, second]);
        let repeated = unsafe { PySequence_Repeat(list, 2) };
        assert!(!repeated.is_null());
        assert_list(repeated, &[first, second, first, second]);
        let copied = unsafe { PySequence_List(list) };
        assert!(!copied.is_null());
        assert_list(copied, &[first, second]);

        let tuple = unsafe { PyList_AsTuple(list) };
        assert!(!tuple.is_null());
        assert_eq!(unsafe { PyTuple_GetItem(tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(tuple, 1) }, second);
        let sequence_tuple = unsafe { PySequence_Tuple(list) };
        assert!(!sequence_tuple.is_null());
        assert_eq!(unsafe { PyTuple_GetItem(sequence_tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(sequence_tuple, 1) }, second);

        let empty_tuple = unsafe { PyTuple_New(0) };
        let empty_tuple_again = unsafe { PyTuple_New(0) };
        assert_eq!(empty_tuple, empty_tuple_again);
        assert!(molt_cpython_abi::abi_types::is_immortal_refcnt(unsafe {
            (*empty_tuple).ob_refcnt
        }));
        let empty_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(empty_tuple)
            .expect("empty tuple is runtime-backed from birth")
            .bits();
        assert_eq!(
            unsafe { molt_cpython_abi::hooks::hooks_or_stubs().classify_heap(empty_bits) },
            molt_cpython_abi::abi_types::MoltTypeTag::Tuple as u8
        );

        let c_tuple = unsafe { PyTuple_New(2) };
        assert!(!c_tuple.is_null());
        let c_tuple_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(c_tuple)
            .expect("C-created exact tuple has canonical runtime identity")
            .bits();
        assert_eq!(
            unsafe { molt_cpython_abi::hooks::hooks_or_stubs().tuple_len(c_tuple_bits) },
            2
        );
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(first);
            assert_eq!(PyTuple_SetItem(c_tuple, 0, first), 0);
            molt_cpython_abi::api::refcount::Py_INCREF(second);
            assert_eq!(PyTuple_SetItem(c_tuple, 1, second), 0);
        }
        assert_eq!(unsafe { PyTuple_GetItem(c_tuple, 0) }, first);
        assert_eq!(unsafe { PyTuple_GetItem(c_tuple, 1) }, second);

        let full_slice = unsafe { PyTuple_GetSlice(c_tuple, 0, 2) };
        assert_eq!(full_slice, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(full_slice) };
        let repeated_once = unsafe { PySequence_Repeat(c_tuple, 1) };
        assert_eq!(repeated_once, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(repeated_once) };
        let repeated_zero = unsafe { PySequence_Repeat(c_tuple, 0) };
        assert_eq!(repeated_zero, empty_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(repeated_zero) };
        let concat_empty = unsafe { PySequence_Concat(empty_tuple, c_tuple) };
        assert_eq!(concat_empty, c_tuple);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(concat_empty) };

        let fast_tuple = unsafe { PySequence_Fast(c_tuple, c"expected iterable".as_ptr()) };
        assert_eq!(fast_tuple, c_tuple);
        let fast_tuple_items = unsafe { PySequence_Fast_ITEMS(fast_tuple) };
        assert_eq!(unsafe { *fast_tuple_items }, first);
        assert_eq!(unsafe { *fast_tuple_items.add(1) }, second);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(fast_tuple) };

        let tuple_iterator = unsafe { molt_cpython_abi::api::object::PySeqIter_New(c_tuple) };
        assert!(!tuple_iterator.is_null());
        let fast_from_iterator =
            unsafe { PySequence_Fast(tuple_iterator, c"expected iterable".as_ptr()) };
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tuple_iterator) };
        assert_ne!(
            unsafe { molt_cpython_abi::api::sequences::PyList_CheckExact(fast_from_iterator) },
            0
        );
        assert_list(fast_from_iterator, &[first, second]);

        let equal_first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        assert!(!equal_first.is_null());
        assert_ne!(equal_first, first);
        let nested_left = unsafe { PyTuple_New(1) };
        let nested_right = unsafe { PyTuple_New(1) };
        let nested_list = unsafe { PyList_New(0) };
        assert!(!nested_left.is_null() && !nested_right.is_null() && !nested_list.is_null());
        unsafe {
            molt_cpython_abi::api::refcount::Py_INCREF(first);
            assert_eq!(PyTuple_SetItem(nested_left, 0, first), 0);
            molt_cpython_abi::api::refcount::Py_INCREF(equal_first);
            assert_eq!(PyTuple_SetItem(nested_right, 0, equal_first), 0);
        }
        assert_eq!(unsafe { PyList_Append(nested_list, nested_left) }, 0);
        assert_eq!(unsafe { PySequence_Contains(nested_list, nested_right) }, 1);
        assert_eq!(unsafe { PySequence_Count(nested_list, nested_right) }, 1);
        assert_eq!(unsafe { PySequence_Index(nested_list, nested_right) }, 0);

        let fast = unsafe { PySequence_Fast(list, c"expected iterable".as_ptr()) };
        assert_eq!(
            fast, list,
            "exact list must take PySequence_Fast's NewRef path"
        );
        let fast_items = unsafe { PySequence_Fast_ITEMS(fast) };
        assert!(!fast_items.is_null());
        assert_eq!(unsafe { *fast_items }, first);
        assert_eq!(unsafe { *fast_items.add(1) }, second);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(fast) };

        let extended = unsafe { PyList_New(0) };
        assert!(!extended.is_null());
        let none = unsafe { _PyList_Extend(extended, list) };
        assert!(!none.is_null());
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(none) };
        assert_list(extended, &[first, second]);
        assert_eq!(unsafe { PyList_SetSlice(extended, 0, 1, slice) }, 0);
        assert_list(extended, &[first, second, second]);

        let inplace = unsafe { PySequence_InPlaceConcat(extended, list) };
        assert_eq!(inplace, extended);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inplace) };
        assert_list(extended, &[first, second, second, first, second]);
        let inplace = unsafe { PySequence_InPlaceRepeat(extended, 2) };
        assert_eq!(inplace, extended);
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inplace) };
        assert_list(
            extended,
            &[
                first, second, second, first, second, first, second, second, first, second,
            ],
        );

        unsafe {
            for pointer in [
                extended,
                nested_list,
                nested_right,
                nested_left,
                equal_first,
                fast_from_iterator,
                c_tuple,
                empty_tuple_again,
                empty_tuple,
                sequence_tuple,
                tuple,
                copied,
                repeated,
                concat,
                slice,
                list,
                first,
                second,
            ] {
                molt_cpython_abi::api::refcount::Py_DECREF(pointer);
            }
        }
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn list_read_and_exact_publication_share_one_cpython_authority() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let _ = crate::molt_exception_clear();

        use molt_cpython_abi::abi_types::{
            PyList_Type, PyListObject, PyObject, PyTypeObject, PyVarObject,
        };
        use molt_cpython_abi::api::abstract_sequence::{
            PySequence_Concat, PySequence_List, PySequence_Repeat,
        };
        use molt_cpython_abi::api::sequences::{
            PyList_Append, PyList_Check, PyList_CheckExact, PyList_GET_ITEM, PyList_GET_SIZE,
            PyList_GetItem, PyList_GetSlice, PyList_New, PyList_Size,
        };

        let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
        let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2000) };
        let list = unsafe { PyList_New(0) };
        assert!(!first.is_null() && !second.is_null() && !list.is_null());
        assert_eq!(unsafe { PyList_Append(list, first) }, 0);
        assert_eq!(unsafe { PyList_Append(list, second) }, 0);

        let assert_exact_list = |value: *mut PyObject, expected: &[*mut PyObject]| {
            assert!(!value.is_null());
            assert_ne!(unsafe { PyList_CheckExact(value) }, 0);
            assert_eq!(unsafe { PyList_Size(value) }, expected.len() as isize);
            assert_eq!(unsafe { PyList_GET_SIZE(value) }, expected.len() as isize);
            for (index, &item) in expected.iter().enumerate() {
                assert_eq!(unsafe { PyList_GetItem(value, index as isize) }, item);
                assert_eq!(unsafe { PyList_GET_ITEM(value, index as isize) }, item);
            }
        };

        let baseline_first = unsafe { (*first).ob_refcnt };
        let baseline_second = unsafe { (*second).ob_refcnt };
        let full = unsafe { PyList_GetSlice(list, 0, isize::MAX) };
        let tail = unsafe { PyList_GetSlice(list, 1, isize::MAX) };
        let negative = unsafe { PyList_GetSlice(list, -5, -1) };
        let reversed_bounds = unsafe { PyList_GetSlice(list, 1, 0) };
        assert_exact_list(full, &[first, second]);
        assert_exact_list(tail, &[second]);
        assert_exact_list(negative, &[]);
        assert_exact_list(reversed_bounds, &[]);
        assert_ne!(full, list, "a full list C slice must be a fresh base list");

        let repeated_once = unsafe { PySequence_Repeat(list, 1) };
        let repeated_zero = unsafe { PySequence_Repeat(list, 0) };
        let repeated_negative = unsafe { PySequence_Repeat(list, -7) };
        assert_exact_list(repeated_once, &[first, second]);
        assert_exact_list(repeated_zero, &[]);
        assert_exact_list(repeated_negative, &[]);
        assert_ne!(
            repeated_once, list,
            "list repetition by one must not reuse the source"
        );

        let copied = unsafe { PySequence_List(list) };
        assert_exact_list(copied, &[first, second]);
        assert_ne!(copied, list, "PySequence_List always returns a fresh list");

        let mut subtype: PyTypeObject = unsafe { std::mem::zeroed() };
        subtype.tp_base = &raw mut PyList_Type;
        let mut subclass_items = [first, second];
        let mut subclass = PyListObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut subtype,
                },
                ob_size: 2,
            },
            ob_item: subclass_items.as_mut_ptr(),
            allocated: 2,
        };
        let subclass = (&raw mut subclass).cast::<PyObject>();
        assert_ne!(unsafe { PyList_Check(subclass) }, 0);
        assert_eq!(unsafe { PyList_Size(subclass) }, 2);
        assert_eq!(unsafe { PyList_GET_SIZE(subclass) }, 2);
        assert_eq!(unsafe { PyList_GetItem(subclass, 0) }, first);
        assert_eq!(unsafe { PyList_GET_ITEM(subclass, 1) }, second);

        let subclass_slice = unsafe { PyList_GetSlice(subclass, 0, 2) };
        let subclass_concat = unsafe { PySequence_Concat(list, subclass) };
        assert_exact_list(subclass_slice, &[first, second]);
        assert_exact_list(subclass_concat, &[first, second, first, second]);

        let mut overflow_subclass = PyListObject {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut subtype,
                },
                ob_size: isize::MAX,
            },
            ob_item: subclass_items.as_mut_ptr(),
            allocated: isize::MAX,
        };
        let overflow_subclass = (&raw mut overflow_subclass).cast::<PyObject>();
        assert!(unsafe { PySequence_Concat(list, overflow_subclass) }.is_null());
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast::<PyObject>(),
                )
            },
            1
        );
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let _ = crate::molt_exception_clear();

        unsafe {
            for value in [
                subclass_concat,
                subclass_slice,
                copied,
                repeated_negative,
                repeated_zero,
                repeated_once,
                reversed_bounds,
                negative,
                tail,
                full,
            ] {
                molt_cpython_abi::api::refcount::Py_DECREF(value);
            }
        }
        assert_eq!(unsafe { (*first).ob_refcnt }, baseline_first);
        assert_eq!(unsafe { (*second).ob_refcnt }, baseline_second);
        unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(list);
            molt_cpython_abi::api::refcount::Py_DECREF(first);
            molt_cpython_abi::api::refcount::Py_DECREF(second);
        }
        let _ = crate::molt_exception_clear();
    }

    #[test]
    fn integer_byte_and_bit_hooks_are_arbitrary_width_and_partial_fill_correct() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();

        let mut source = [0u8; 17];
        source[0] = 0xf0;
        source[7] = 0x12;
        source[16] = 0x01; // bit length 129
        let bits = unsafe { hook_int_from_bytes(source.as_ptr(), source.len(), 1, 0) };
        assert_ne!(bits, 0);
        let mut num_bits = 0usize;
        assert_eq!(unsafe { hook_int_num_bits(bits, &raw mut num_bits) }, 0);
        assert_eq!(num_bits, 129);

        let mut full = [0u8; 17];
        assert_eq!(
            unsafe { hook_int_to_bytes(bits, full.as_mut_ptr(), full.len(), 1, 0) },
            crate::builtins::numbers::INT_BYTES_OK
        );
        assert_eq!(full, source);

        let mut short = [0xaa; 8];
        assert_eq!(
            unsafe { hook_int_to_bytes(bits, short.as_mut_ptr(), short.len(), 1, 0) },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(short, source[..8]);

        let big_endian_source = [0x12, 0x34, 0x56];
        let big_endian = unsafe {
            hook_int_from_bytes(big_endian_source.as_ptr(), big_endian_source.len(), 0, 0)
        };
        let mut big_endian_short = [0xaa; 2];
        assert_eq!(
            unsafe {
                hook_int_to_bytes(
                    big_endian,
                    big_endian_short.as_mut_ptr(),
                    big_endian_short.len(),
                    0,
                    0,
                )
            },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(
            big_endian_short,
            [0x34, 0x56],
            "big-endian overflow writes the low bytes, not the MSB prefix"
        );

        let minus_129 = [0xff, 0x7f];
        let negative = unsafe { hook_int_from_bytes(minus_129.as_ptr(), 2, 0, 1) };
        let mut one = [0u8; 1];
        assert_eq!(
            unsafe { hook_int_to_bytes(negative, one.as_mut_ptr(), 1, 1, 1) },
            crate::builtins::numbers::INT_BYTES_OVERFLOW
        );
        assert_eq!(one, [0x7f]);
        assert_eq!(
            unsafe { hook_int_to_bytes(negative, one.as_mut_ptr(), 1, 1, 0) },
            crate::builtins::numbers::INT_BYTES_NEGATIVE_UNSIGNED
        );

        with_gil(|_py| {
            dec_ref_bits(&_py, bits);
            dec_ref_bits(&_py, big_endian);
            dec_ref_bits(&_py, negative);
        });
    }

    #[test]
    fn integer_byte_hook_refuses_float_and_accepts_bool_without_conversion() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        let mut output = [0xa5; 4];
        assert_eq!(
            unsafe {
                hook_int_to_bytes(
                    MoltObject::from_float(3.0).bits(),
                    output.as_mut_ptr(),
                    4,
                    1,
                    1,
                )
            },
            INT_BYTES_INVALID
        );
        assert_eq!(output, [0xa5; 4]);
        assert_eq!(
            unsafe {
                hook_int_to_bytes(
                    MoltObject::from_bool(true).bits(),
                    output.as_mut_ptr(),
                    4,
                    0,
                    0,
                )
            },
            crate::builtins::numbers::INT_BYTES_OK
        );
        assert_eq!(output, [0, 0, 0, 1]);
        assert_eq!(
            unsafe { hook_int_to_bytes(MoltObject::from_int(-1).bits(), ptr::null_mut(), 0, 0, 1) },
            crate::builtins::numbers::INT_BYTES_OK
        );
        assert_eq!(
            unsafe {
                hook_int_to_bytes(
                    MoltObject::from_int(1).bits(),
                    output.as_mut_ptr(),
                    usize::MAX,
                    1,
                    0,
                )
            },
            INT_BYTES_INVALID
        );
    }

    #[test]
    fn hook_tuple_set_checked_index_and_tag_guard() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        register_cpython_hooks();
        with_gil(|_py| unsafe {
            let ptr = alloc_tuple_uninitialized(&_py, 4);
            assert!(!ptr.is_null());
            assert_eq!(object_type_id(ptr), TYPE_ID_TUPLE);
            let bits = MoltObject::from_ptr(ptr).bits();

            hook_tuple_set(
                bits,
                usize::MAX,
                MoltObject::from_int(7).bits(),
                ptr::null_mut(),
            );
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );

            hook_tuple_set(
                bits,
                1_000_004,
                MoltObject::from_int(7).bits(),
                ptr::null_mut(),
            );
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );

            use molt_cpython_abi::api::refcount::OwnedPyObject;
            let zero_pointer =
                OwnedPyObject::from_owned(molt_cpython_abi::api::numbers::PyFloat_FromDouble(0.0));
            let value_pointer =
                OwnedPyObject::from_owned(molt_cpython_abi::api::numbers::PyLong_FromLong(42));
            assert!(!zero_pointer.as_ptr().is_null() && !value_pointer.as_ptr().is_null());
            let val = MoltObject::from_int(42).bits();
            for index in [0, 1, 3] {
                assert!(matches!(
                    hook_tuple_item(bits, index).decode(),
                    DecodedHandleResult::Missing
                ));
                assert_eq!(crate::c_api::PyTuple_GetItem(bits, index as isize), 0);
                assert!(!crate::exception_pending(&_py));
                assert!(matches!(
                    hook_tuple_set(bits, index, 0, zero_pointer.as_ptr()).decode(),
                    DecodedHandleResult::Missing
                ));
                assert_eq!(borrowed_bits(hook_tuple_item(bits, index)), Some(0));
                assert_eq!(crate::c_api::PyTuple_GetItem(bits, index as isize), 0);
                assert!(!crate::exception_pending(&_py));
            }
            let none = MoltObject::none().bits();
            let none_pointer = OwnedPyObject::from_owned(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(none),
            );
            assert!(!none_pointer.as_ptr().is_null());
            assert!(matches!(
                hook_tuple_set(bits, 3, none, none_pointer.as_ptr()).decode(),
                DecodedHandleResult::Ok(0)
            ));
            assert_eq!(borrowed_bits(hook_tuple_item(bits, 3)), Some(none));
            assert_eq!(crate::c_api::PyTuple_GetItem(bits, 3), none);
            assert!(!crate::exception_pending(&_py));

            match hook_tuple_set(bits, 2, val, value_pointer.as_ptr()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Missing => {}
                _ => panic!("tuple store failed"),
            }
            assert_eq!(
                crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.len()),
                Some(4)
            );
            assert_eq!(borrowed_bits(hook_tuple_item(bits, 2)), Some(val));
            assert_eq!(crate::c_api::PyTuple_GetItem(bits, 2), val);
            assert!(!crate::exception_pending(&_py));

            let heap_ptr = alloc_string(&_py, b"owned");
            assert!(!heap_ptr.is_null());
            let heap_bits = MoltObject::from_ptr(heap_ptr).bits();
            let heap_pointer = OwnedPyObject::from_owned(
                molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(heap_bits),
            );
            assert!(!heap_pointer.as_ptr().is_null());
            inc_ref_bits(&_py, heap_bits);
            let old = match hook_tuple_set(bits, 2, heap_bits, heap_pointer.as_ptr()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(old) => old,
                _ => panic!("tuple heap store failed"),
            };
            dec_ref_bits(&_py, old);
            let borrowed_refs = (*header_from_obj_ptr(heap_ptr)).ref_count_snapshot();
            assert_eq!(borrowed_bits(hook_tuple_item(bits, 2)), Some(heap_bits));
            assert_eq!(crate::c_api::PyTuple_GetItem(bits, 2), heap_bits);
            assert_eq!(
                (*header_from_obj_ptr(heap_ptr)).ref_count_snapshot(),
                borrowed_refs,
                "tuple construction reads must not acquire an owned reference"
            );
            assert_eq!(
                crate::object::seq_access::tracked_heap_edge_count(ptr),
                Some(1)
            );
            assert_ne!(
                (*header_from_obj_ptr(ptr)).load_metadata_flags()
                    & crate::object::HEADER_FLAG_CONTAINS_REFS,
                0
            );
            let old = match hook_tuple_set(bits, 2, val, value_pointer.as_ptr()).decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(old) => old,
                _ => panic!("tuple primitive replacement failed"),
            };
            dec_ref_bits(&_py, old);
            assert_eq!(
                crate::object::seq_access::tracked_heap_edge_count(ptr),
                Some(0)
            );
            assert_eq!(
                (*header_from_obj_ptr(ptr)).load_metadata_flags()
                    & crate::object::HEADER_FLAG_CONTAINS_REFS,
                0
            );
            dec_ref_bits(&_py, heap_bits);

            let list_bits = hook_alloc_list();
            hook_tuple_set(list_bits, 0, val, ptr::null_mut());
            dec_ref_bits(&_py, list_bits);
            dec_ref_bits(&_py, bits);
        });
    }
}
