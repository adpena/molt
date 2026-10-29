//! The outer program-exit boundary, shared by the native main stub, the JIT
//! runner and every WASM host.
//!
//! CPython reports an exception that escapes the main module with
//! `PyErr_Print`: `_Py_HandleSystemExit` turns a `SystemExit` into the exit
//! status, and any other exception sets `sys.last_exc` (and the legacy
//! `last_*` names), raises the `sys.excepthook` audit event and calls
//! `sys.excepthook`. A failing hook is reported as "Error in sys.excepthook"
//! followed by the original exception. Hosts call `molt_main_exit_status` once
//! the entry returns and then leave through the runtime's exit, so the report
//! runs before teardown and its atexit phase, as in `Py_RunMain`.

use super::unraisable::sys_attr_bits;
use super::*;
use crate::BuiltinExc;
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{
    attr_name_bits_from_bytes, call_callable1, call_callable3, molt_get_attr_name,
    molt_set_attr_name,
};

/// The exit status of a program whose main body has returned: 0 when nothing
/// is pending; otherwise the pending exception is taken, reported as CPython's
/// `PyErr_Print` reports it, and its status returned.
#[unsafe(no_mangle)]
pub extern "C" fn molt_main_exit_status() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let pending = exception_pending(_py);
        if runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_MAIN_EXCEPTION) {
            eprintln!("molt main finish pending={}", i32::from(pending));
        }
        if !pending {
            return 0;
        }
        let exc_bits = super::exception_state_abi::exception_last_pending_bits(_py);
        clear_exception(_py);
        let status = report_uncaught(_py, exc_bits);
        dec_ref_bits(_py, exc_bits);
        status as u32 as u64
    })
}

fn report_uncaught(py: &PyToken<'_>, exc_bits: u64) -> i32 {
    let Some(exc_ptr) = obj_from_bits(exc_bits).as_ptr() else {
        eprintln!("RuntimeError: invalid uncaught exception payload");
        return 1;
    };
    if !exception_is_instance(py, exc_bits) {
        eprintln!("RuntimeError: uncaught payload is not an exception");
        return 1;
    }
    if exception_matches_kind(py, exc_bits, crate::BuiltinExc::SystemExit) {
        return super::system_exit_code(py, exc_ptr);
    }
    // CPython `run_eval_code_obj`: only an exact KeyboardInterrupt makes the
    // process exit by SIGINT once finalization completes.
    let keyboard_interrupt = exception_class_bits(py, BuiltinExc::KeyboardInterrupt);
    if keyboard_interrupt != 0
        && exception_class(py, exc_bits).is_some_and(|class| class.bits() == keyboard_interrupt)
    {
        crate::builtins::signal_ext::signal_note_unhandled_keyboard_interrupt(py);
    }
    if let Some(path) = crate::runtime_state(py)
        .policy()
        .exception_log_path
        .as_ref()
    {
        let _ = std::fs::write(
            path,
            format_exception_with_traceback(py, exc_ptr).as_bytes(),
        );
    }

    let type_bits = crate::type_of_bits(py, exc_bits);
    let trace_bits = unsafe { exception_trace_bits(exc_ptr) };
    let tb_bits = if exception_field_is_missing(trace_bits) {
        MoltObject::none().bits()
    } else {
        trace_bits
    };
    set_sys_attr(py, b"last_exc", exc_bits);
    set_sys_attr(py, b"last_type", type_bits);
    set_sys_attr(py, b"last_value", exc_bits);
    set_sys_attr(py, b"last_traceback", tb_bits);

    let hook_bits = sys_attr_bits(py, b"excepthook");
    let audited = crate::builtins::sys_ext::audit_event(
        py,
        "sys.excepthook",
        &[hook_bits, type_bits, exc_bits, tb_bits],
    );
    if !audited {
        let runtime_error = exception_class_bits(py, BuiltinExc::RuntimeError);
        if runtime_error != 0 && pending_exception_matches_type(py, runtime_error) {
            clear_exception(py);
            release(py, hook_bits);
            return 1;
        }
        let audit_exc = super::exception_state_abi::exception_last_pending_bits(py);
        clear_exception(py);
        report_captured_unraisable(
            py,
            MoltObject::none().bits(),
            audit_exc,
            Some("Exception ignored in audit hook"),
        );
        release(py, audit_exc);
    }
    if obj_from_bits(hook_bits).is_none() {
        write_sys_stderr(py, "sys.excepthook is missing\n");
        display_exception(py, exc_bits);
        return 1;
    }
    let result = unsafe { call_callable3(py, hook_bits, type_bits, exc_bits, tb_bits) };
    release(py, hook_bits);
    release(py, result);
    if exception_pending(py) {
        let hook_exc = super::exception_state_abi::exception_last_pending_bits(py);
        clear_exception(py);
        if exception_matches_kind(py, hook_exc, crate::BuiltinExc::SystemExit) {
            let status = obj_from_bits(hook_exc)
                .as_ptr()
                .map_or(1, |ptr| super::system_exit_code(py, ptr));
            release(py, hook_exc);
            return status;
        }
        write_sys_stderr(py, "Error in sys.excepthook:\n");
        display_exception(py, hook_exc);
        write_sys_stderr(py, "\nOriginal exception was:\n");
        display_exception(py, exc_bits);
        release(py, hook_exc);
    }
    1
}

fn release(py: &PyToken<'_>, bits: u64) {
    if !obj_from_bits(bits).is_none() {
        dec_ref_bits(py, bits);
    }
}

/// Set one `sys` attribute; a failure is cleared, as `_PySys_SetAttr` failures
/// are in `PyErr_Print`.
fn set_sys_attr(py: &PyToken<'_>, name: &[u8], value_bits: u64) {
    let Some(sys_bits) = crate::builtins::modules::interpreter_sys_module(py) else {
        return;
    };
    let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
        return;
    };
    let result = molt_set_attr_name(sys_bits, name_bits, value_bits);
    dec_ref_bits(py, name_bits);
    release(py, result);
    if exception_pending(py) {
        clear_exception(py);
    }
}

/// `PySys_WriteStderr`: write through `sys.stderr`, or to the process stderr
/// when it is missing, None, or its `write` fails.
pub(super) fn write_sys_stderr(py: &PyToken<'_>, text: &str) {
    let stream_bits = sys_attr_bits(py, b"stderr");
    if !obj_from_bits(stream_bits).is_none() {
        let written = write_stream(py, stream_bits, text);
        dec_ref_bits(py, stream_bits);
        if written {
            return;
        }
    }
    eprint!("{text}");
}

fn write_stream(py: &PyToken<'_>, stream_bits: u64, text: &str) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(py, b"write") else {
        return false;
    };
    let write_bits = molt_get_attr_name(stream_bits, name_bits);
    dec_ref_bits(py, name_bits);
    if exception_pending(py) {
        clear_exception(py);
        release(py, write_bits);
        return false;
    }
    let text_ptr = alloc_string(py, text.as_bytes());
    if text_ptr.is_null() {
        release(py, write_bits);
        return false;
    }
    let text_bits = MoltObject::from_ptr(text_ptr).bits();
    let result = unsafe { call_callable1(py, write_bits, text_bits) };
    dec_ref_bits(py, text_bits);
    release(py, write_bits);
    release(py, result);
    if exception_pending(py) {
        clear_exception(py);
        return false;
    }
    true
}

/// `PyErr_DisplayException`: the default display, which `sys.__excepthook__`
/// owns; the formatted traceback goes to the process stderr only when that
/// hook is missing or fails.
fn display_exception(py: &PyToken<'_>, exc_bits: u64) {
    let hook_bits = sys_attr_bits(py, b"__excepthook__");
    if !obj_from_bits(hook_bits).is_none() {
        let type_bits = crate::type_of_bits(py, exc_bits);
        let tb_bits = obj_from_bits(exc_bits)
            .as_ptr()
            .map(|ptr| unsafe { exception_trace_bits(ptr) })
            .filter(|bits| !exception_field_is_missing(*bits))
            .unwrap_or_else(|| MoltObject::none().bits());
        let result = unsafe { call_callable3(py, hook_bits, type_bits, exc_bits, tb_bits) };
        dec_ref_bits(py, hook_bits);
        release(py, result);
        if !exception_pending(py) {
            return;
        }
        clear_exception(py);
    }
    if let Some(ptr) = obj_from_bits(exc_bits).as_ptr() {
        eprintln!("{}", format_exception_with_traceback(py, ptr));
    }
}
