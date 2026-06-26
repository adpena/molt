use super::*;

pub(super) fn exception_last_public_bits(_py: &PyToken<'_>) -> u64 {
    // Fast path: if neither the global nor task pending flag is set,
    // there is no live exception — return None immediately.  This
    // keeps `exception_last` in sync with the inline `check_exception`
    // flag byte that the Cranelift backend uses.
    let state = runtime_state(_py);
    let task_pending = state
        .task_last_exception_pending
        .load(AtomicOrdering::Relaxed);
    let global_pending = state.last_exception_pending.load(AtomicOrdering::Acquire);
    if !task_pending && !global_pending {
        if let Some(bits) = exception_context_active_bits() {
            inc_ref_bits(_py, bits);
            return bits;
        }
        return MoltObject::none().bits();
    }
    let debug_flow = debug_exception_flow();
    if let Some(task_key) = current_task_key() {
        let ptr = {
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            match guard.get(&task_key).copied() {
                Some(ptr) if exception_slot_is_valid(ptr) => Some(ptr),
                Some(_) => {
                    guard.remove(&task_key);
                    if guard.is_empty() {
                        runtime_state(_py)
                            .task_last_exception_pending
                            .store(false, AtomicOrdering::Relaxed);
                    }
                    None
                }
                None => None,
            }
        };
        if let Some(ptr) = ptr {
            let bits = MoltObject::from_ptr(ptr.0).bits();
            if exception_handler_active() {
                let active_bits = exception_context_active_bits();
                if let Some(active_bits) = active_bits {
                    inc_ref_bits(_py, active_bits);
                    clear_exception(_py);
                    return active_bits;
                }
                exception_context_set(_py, bits);
                runtime_state(_py)
                    .task_last_exception_pending
                    .store(false, AtomicOrdering::Relaxed);
            }
            if debug_flow {
                let kind_bits = unsafe { exception_kind_bits(ptr.0) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let rc = unsafe {
                    let header = header_from_obj_ptr(ptr.0);
                    (*header).ref_count.load(AtomicOrdering::Acquire)
                };
                eprintln!(
                    "molt exc last task=0x{:x} kind={} ptr=0x{:x} rc={}",
                    task_key.0 as usize, kind, ptr.0 as usize, rc
                );
            }
            inc_ref_bits(_py, bits);
            return bits;
        }
    }
    let ptr = global_last_exception_pending_slot(_py);
    if let Some(ptr) = ptr {
        let bits = MoltObject::from_ptr(ptr.0).bits();
        if exception_handler_active() {
            let active_bits = exception_context_active_bits();
            if let Some(active_bits) = active_bits {
                inc_ref_bits(_py, active_bits);
                clear_exception(_py);
                return active_bits;
            }
            exception_context_set(_py, bits);
            state
                .last_exception_pending
                .store(false, AtomicOrdering::Release);
        }
        if debug_flow {
            let kind_bits = unsafe { exception_kind_bits(ptr.0) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let rc = unsafe {
                let header = header_from_obj_ptr(ptr.0);
                (*header).ref_count.load(AtomicOrdering::Acquire)
            };
            eprintln!(
                "molt exc last task=0x0 kind={} ptr=0x{:x} rc={}",
                kind, ptr.0 as usize, rc
            );
        }
        inc_ref_bits(_py, bits);
        return bits;
    }
    if debug_flow {
        eprintln!("molt exc last task=0x0 kind=none");
    }
    MoltObject::none().bits()
}

pub(super) fn exception_last_pending_bits(_py: &PyToken<'_>) -> u64 {
    let state = runtime_state(_py);
    let task_pending = state
        .task_last_exception_pending
        .load(AtomicOrdering::Relaxed);
    let global_pending = state.last_exception_pending.load(AtomicOrdering::Acquire);
    if !task_pending && !global_pending {
        if debug_exception_flow() {
            eprintln!("molt exc last_pending task=0x0 kind=none");
        }
        return MoltObject::none().bits();
    }

    let debug_flow = debug_exception_flow();
    if let Some(task_key) = current_task_key()
        && task_pending
    {
        let ptr = {
            let mut guard = task_last_exceptions(_py).lock().unwrap();
            match guard.get(&task_key).copied() {
                Some(ptr) if exception_slot_is_valid(ptr) => Some(ptr),
                Some(_) => {
                    guard.remove(&task_key);
                    if guard.is_empty() {
                        state
                            .task_last_exception_pending
                            .store(false, AtomicOrdering::Relaxed);
                    }
                    None
                }
                None => None,
            }
        };
        if let Some(ptr) = ptr {
            let bits = MoltObject::from_ptr(ptr.0).bits();
            if debug_flow {
                let kind_bits = unsafe { exception_kind_bits(ptr.0) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!(
                    "molt exc last_pending task=0x{:x} kind={} ptr=0x{:x}",
                    task_key.0 as usize, kind, ptr.0 as usize
                );
            }
            inc_ref_bits(_py, bits);
            return bits;
        }
    }

    if let Some(ptr) = global_last_exception_pending_slot(_py) {
        let bits = MoltObject::from_ptr(ptr.0).bits();
        if debug_flow {
            let kind_bits = unsafe { exception_kind_bits(ptr.0) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            eprintln!(
                "molt exc last_pending task=0x0 kind={} ptr=0x{:x}",
                kind, ptr.0 as usize
            );
        }
        inc_ref_bits(_py, bits);
        return bits;
    }

    if debug_flow {
        eprintln!("molt exc last_pending task=0x0 kind=none");
    }
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_last() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { exception_last_public_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_last_pending() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { exception_last_pending_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_active() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(bits) = exception_context_active_bits() {
            if debug_exception_flow() {
                let kind = obj_from_bits(bits)
                    .as_ptr()
                    .map(|ptr| unsafe { exception_kind_bits(ptr) })
                    .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!("molt exc active kind={} bits=0x{:x}", kind, bits);
            }
            inc_ref_bits(_py, bits);
            return bits;
        }
        if debug_exception_flow() {
            eprintln!(
                "molt exc active kind=none bits=0x{:x}",
                MoltObject::none().bits()
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_current() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(bits) = exception_context_active_bits() {
            if debug_exception_flow() {
                let kind = obj_from_bits(bits)
                    .as_ptr()
                    .map(|ptr| unsafe { exception_kind_bits(ptr) })
                    .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!(
                    "molt exc current source=active kind={} bits=0x{:x}",
                    kind, bits
                );
            }
            inc_ref_bits(_py, bits);
            return bits;
        }
        let bits = exception_last_public_bits(_py);
        if debug_exception_flow() {
            let kind = obj_from_bits(bits)
                .as_ptr()
                .map(|ptr| unsafe { exception_kind_bits(ptr) })
                .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
                .unwrap_or_else(|| type_name(_py, obj_from_bits(bits)).into_owned());
            eprintln!(
                "molt exc current source=last kind={} bits=0x{:x}",
                kind, bits
            );
        }
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_resolve_captured(captured_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let captured = obj_from_bits(captured_bits);
        if let Some(ptr) = captured.as_ptr()
            && unsafe { object_type_id(ptr) == TYPE_ID_EXCEPTION }
        {
            if debug_exception_flow() {
                let kind_bits = unsafe { exception_kind_bits(ptr) };
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!(
                    "molt exc resolve source=captured kind={} bits=0x{:x}",
                    kind, captured_bits
                );
            }
            inc_ref_bits(_py, captured_bits);
            return captured_bits;
        }
        if let Some(bits) = exception_context_active_bits() {
            if debug_exception_flow() {
                let kind = obj_from_bits(bits)
                    .as_ptr()
                    .map(|ptr| unsafe { exception_kind_bits(ptr) })
                    .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
                    .unwrap_or_else(|| "<unknown>".to_string());
                eprintln!(
                    "molt exc resolve source=active kind={} bits=0x{:x}",
                    kind, bits
                );
            }
            inc_ref_bits(_py, bits);
            return bits;
        }
        let bits = exception_last_public_bits(_py);
        if debug_exception_flow() {
            let kind = obj_from_bits(bits)
                .as_ptr()
                .map(|ptr| unsafe { exception_kind_bits(ptr) })
                .and_then(|kind_bits| string_obj_to_owned(obj_from_bits(kind_bits)))
                .unwrap_or_else(|| type_name(_py, obj_from_bits(bits)).into_owned());
            eprintln!(
                "molt exc resolve source=last kind={} bits=0x{:x}",
                kind, bits
            );
        }
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_enter_handler(captured_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bits = {
            let captured = obj_from_bits(captured_bits);
            if let Some(ptr) = captured.as_ptr()
                && unsafe { object_type_id(ptr) == TYPE_ID_EXCEPTION }
            {
                inc_ref_bits(_py, captured_bits);
                captured_bits
            } else if let Some(active_bits) = exception_context_active_bits() {
                inc_ref_bits(_py, active_bits);
                active_bits
            } else {
                exception_last_public_bits(_py)
            }
        };
        clear_exception(_py);
        exception_context_set(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_clear() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug_clear = debug_exception_clear();
        let reason = exception_clear_reason_take();
        let cleared_bits = if debug_clear && exception_pending(_py) {
            molt_exception_last()
        } else {
            MoltObject::none().bits()
        };
        if debug_clear
            && !obj_from_bits(cleared_bits).is_none()
            && let Some(ptr) = maybe_ptr_from_bits(cleared_bits)
        {
            let kind_bits = unsafe { exception_kind_bits(ptr) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            let reason_str = reason.unwrap_or("<unset>");
            eprintln!(
                "molt exc clear task=0x{:x} kind={} reason={}",
                task, kind, reason_str
            );
            if reason_str == "<unset>" {
                eprintln!("molt exc clear backtrace (reason unset):");
                eprintln!("{:?}", Backtrace::force_capture());
            }
            let frame = FRAME_STACK.with(|stack| stack.borrow().last().copied());
            if let Some(frame) = frame
                && let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits)
            {
                let (name_bits, file_bits) =
                    unsafe { (code_name_bits(code_ptr), code_filename_bits(code_ptr)) };
                let name = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let file = string_obj_to_owned(obj_from_bits(file_bits))
                    .unwrap_or_else(|| "<unknown>".to_string());
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, file_bits);
                eprintln!(
                    "molt exc clear frame name={} file={} line={}",
                    name, file, frame.line
                );
            }
            if kind == "GeneratorExit" {
                let task_ptr = current_task_ptr();
                if !task_ptr.is_null() {
                    let (poll_fn, type_id, class_name) = unsafe {
                        let _header = header_from_obj_ptr(task_ptr);
                        let poll_fn = crate::object::object_poll_fn(task_ptr);
                        let type_id = object_type_id(task_ptr);
                        let class_name = class_name_for_error(object_class_bits(task_ptr));
                        (poll_fn, type_id, class_name)
                    };
                    eprintln!(
                        "molt exc clear ctx task=0x{:x} poll=0x{:x} type_id={} class={}",
                        task_ptr as usize, poll_fn, type_id, class_name
                    );
                } else {
                    eprintln!("molt exc clear ctx task=none");
                }
                eprintln!("molt exc clear backtrace (GeneratorExit):");
                eprintln!("{:?}", Backtrace::force_capture());
            }
        }
        clear_exception(_py);
        if debug_clear && !obj_from_bits(cleared_bits).is_none() {
            dec_ref_bits(_py, cleared_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending_fast() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { if exception_pending(_py) { 1 } else { 0 } })
}

/// Returns a pointer to the `last_exception_pending` AtomicBool byte.
/// The native Cranelift backend uses this to inline the exception check
/// as a single byte load + branch, avoiding the full function call
/// overhead of `molt_exception_pending_fast` on the happy path.
///
/// Returns null if the runtime is not initialized.
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {
    let Some(state) = crate::state::runtime_state::runtime_state_for_gil() else {
        return 0;
    };
    state.last_exception_pending.as_ptr() as u64
}

/// Returns a pointer to the `task_last_exception_pending` AtomicBool byte,
/// or null (0) if no async task is active.
#[unsafe(no_mangle)]
pub extern "C" fn molt_task_exception_pending_flag_ptr() -> u64 {
    let Some(state) = crate::state::runtime_state::runtime_state_for_gil() else {
        return 0;
    };
    if current_task_key().is_none() {
        return 0;
    }
    state.task_last_exception_pending.as_ptr() as u64
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_enter() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let prev = exception_stack_baseline_get();
        let depth = exception_stack_depth();
        exception_stack_baseline_set(depth);
        int_bits_from_i64(_py, prev as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_exit(prev_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // The prev_bits value comes from exception_stack_enter and should
        // be a NaN-boxed non-negative int.  However, when the exception
        // handler is reached through SSA paths where the variable was
        // never defined (e.g., check_exception brif to an exception label
        // that joins multiple paths), the value may be None (the default
        // Cranelift Variable value for undefined paths).  In that case,
        // reset to 0 rather than raising a TypeError that prevents stdlib
        // module init from completing.
        let prev = match to_i64(obj_from_bits(prev_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => 0,
        };
        exception_stack_baseline_set(prev);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_depth() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        int_bits_from_i64(_py, exception_stack_depth() as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_set_depth(depth_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Same as exception_stack_exit: SSA paths through exception
        // handlers may pass None for undefined variables. Default to 0.
        let depth = match to_i64(obj_from_bits(depth_bits)) {
            Some(val) if val >= 0 => val as usize,
            _ => 0,
        };
        exception_stack_set_depth(_py, depth);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_push() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        exception_stack_push();
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pop() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        exception_stack_pop(_py);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_stack_clear() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        exception_stack_set_depth(_py, 0);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_raise(exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let exc_obj = obj_from_bits(exc_bits);
        if exc_obj.is_none() || exc_bits == 0 {
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        }
        let Some(ptr) = exc_obj.as_ptr() else {
            let payload_type = type_name(_py, exc_obj).into_owned();
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if exception_handler_active() && payload_type == "object" {
                // Internal safeguard: control-flow bookkeeping can transiently surface
                // non-pointer garbage payloads at handler boundaries. Do not convert that
                // into a user-visible TypeError; let handler unwinding continue.
                return MoltObject::none().bits();
            }
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exceptions must derive from BaseException",
            );
        };
        let mut exc_ptr = ptr;
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_EXCEPTION => {}
                TYPE_ID_TYPE => {
                    let class_bits = MoltObject::from_ptr(ptr).bits();
                    if !issubclass_bits(class_bits, builtin_classes(_py).base_exception) {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "exceptions must derive from BaseException",
                        );
                    }
                    let inst_bits = call_class_init_with_args(_py, ptr, &[]);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(inst_ptr) != TYPE_ID_EXCEPTION {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "exceptions must derive from BaseException",
                        );
                    }
                    exc_ptr = inst_ptr;
                }
                _ => {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "exceptions must derive from BaseException",
                    );
                }
            }
        }
        if debug_exception_flow() || debug_exception_raise() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let task = current_task_key().map(|slot| slot.0 as usize).unwrap_or(0);
            let depth = exception_stack_depth();
            eprintln!(
                "molt exc raise task=0x{:x} kind={} handler_active={} depth={} task_raise_active={}",
                task,
                kind,
                exception_handler_active(),
                depth,
                task_raise_active()
            );
        }
        record_exception(_py, exc_ptr);
        if exception_handler_active() {
            exception_context_set(_py, MoltObject::from_ptr(exc_ptr).bits());
        }
        if !exception_handler_active() && !generator_raise_active() && !task_raise_active() {
            let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
            if string_obj_to_owned(obj_from_bits(kind_bits)).as_deref() == Some("SystemExit") {
                handle_system_exit(_py, exc_ptr);
            }
            context_stack_unwind(_py, MoltObject::from_ptr(exc_ptr).bits());
            let formatted = format_exception_with_traceback(_py, exc_ptr);
            eprintln!("{}", formatted);
            if let Ok(path) = std::env::var("MOLT_EXCEPTION_LOG_PATH") {
                let _ = std::fs::write(path, formatted.as_bytes());
            }
            std::process::exit(1);
        }
        MoltObject::none().bits()
    })
}
