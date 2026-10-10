//! C-API callbacks forward to the native Context owner without Python imports.
use super::*;
use crate::builtins::contextvars as owner;
use molt_cpython_abi::hooks::OwnedHandleResult;
pub(super) unsafe extern "C" fn type_admit(pointer: usize) -> i32 {
    with_gil(|py| {
        if !admit_process_cpython_state(&py) {
            return -1;
        }
        let bits = if pointer == (&raw mut molt_cpython_abi::abi_types::PyContext_Type).addr() {
            owner::context_class(&py)
        } else if pointer == (&raw mut molt_cpython_abi::abi_types::PyContextVar_Type).addr() {
            owner::variable_class(&py)
        } else if pointer == (&raw mut molt_cpython_abi::abi_types::PyContextToken_Type).addr() {
            owner::token_class(&py)
        } else {
            crate::raise_exception::<u64>(&py, "SystemError", "unknown Context static shell");
            return -1;
        };
        if bits == 0 || crate::exception_pending(&py) {
            -1
        } else {
            0
        }
    })
}
fn result(bits: Option<u64>) -> OwnedHandleResult {
    bits.map_or_else(OwnedHandleResult::error, OwnedHandleResult::ok)
}
pub(super) unsafe extern "C" fn new() -> OwnedHandleResult {
    with_gil(|py| result(owner::new_context(&py)))
}
pub(super) unsafe extern "C" fn copy(ctx: u64) -> OwnedHandleResult {
    with_gil(|py| result(owner::copy_context(&py, ctx)))
}
pub(super) unsafe extern "C" fn copy_current() -> OwnedHandleResult {
    with_gil(|py| result(owner::copy_current(&py)))
}
pub(super) unsafe extern "C" fn enter(ctx: u64) -> i32 {
    with_gil(|py| if owner::enter(&py, ctx) { 0 } else { -1 })
}
pub(super) unsafe extern "C" fn exit(ctx: u64) -> i32 {
    with_gil(|py| if owner::exit(&py, ctx) { 0 } else { -1 })
}
pub(super) unsafe extern "C" fn var_new(
    name: u64,
    default: u64,
    has_default: i32,
) -> OwnedHandleResult {
    with_gil(|py| {
        result(owner::new_variable(
            &py,
            name,
            (has_default != 0).then_some(default),
        ))
    })
}
pub(super) unsafe extern "C" fn var_get(
    var: u64,
    default: u64,
    has_default: i32,
) -> OwnedHandleResult {
    with_gil(
        |py| match owner::get_variable(&py, var, (has_default != 0).then_some(default)) {
            Some(Some(bits)) => OwnedHandleResult::ok(bits),
            Some(None) => OwnedHandleResult::missing(),
            None => OwnedHandleResult::error(),
        },
    )
}
pub(super) unsafe extern "C" fn var_set(var: u64, value: u64) -> OwnedHandleResult {
    with_gil(|py| result(owner::set_variable(&py, var, value)))
}
pub(super) unsafe extern "C" fn var_reset(var: u64, token: u64) -> i32 {
    with_gil(|py| {
        if owner::reset_variable(&py, var, token) {
            0
        } else {
            -1
        }
    })
}
