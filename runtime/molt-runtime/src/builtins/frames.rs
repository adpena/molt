use crate::PyToken;
use crate::builtins::exceptions::raise_exception;
use crate::{
    FRAME_STACK, MoltHeader, TRACEBACK_BUILD_COUNT, TRACEBACK_BUILD_FRAMES, TYPE_ID_CODE,
    TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_FUNCTION, TYPE_ID_MODULE, TYPE_ID_TRACEBACK_PAYLOAD,
    TYPE_ID_TUPLE, TYPE_ID_TYPE, alloc_dict_with_pairs, alloc_instance_for_class_no_pool,
    alloc_object, builtin_classes, code_filename_bits, code_firstlineno, code_linetable_bits,
    code_name_bits, dec_ref_bits, dict_get_in_place, dict_order, function_globals_bits,
    function_globals_override_enabled, inc_ref_bits, instance_dict_bits, instance_set_dict_bits,
    intern_runtime_static_name, intern_static_name, module_dict_bits, obj_from_bits,
    object_mark_has_ptrs, object_type_id, profile_enabled, runtime_state, seq_vec_ref,
    string_obj_to_owned, to_i64,
};
use molt_obj_model::MoltObject;
use std::sync::atomic::Ordering as AtomicOrdering;

#[derive(Clone, Copy)]
pub(crate) struct FrameEntry {
    pub(crate) code_bits: u64,
    pub(crate) line: i64,
    /// 0-based column offset for traceback caret annotations.
    /// -1 means "not available" (fall back to inference).
    pub(crate) col_offset: i64,
    /// 0-based end column offset for traceback caret annotations.
    /// -1 means "not available".
    pub(crate) end_col_offset: i64,
    /// Optional dict snapshot for `locals()` / `frame.f_locals`.
    ///
    /// This is set by compiler-emitted ops (`frame_locals_set`) and is owned by
    /// the frame stack entry (we INCREF on set and DECREF on pop/replacement).
    pub(crate) locals_bits: u64,
    /// Optional globals dict for function frames.
    ///
    /// Function objects own their `__globals__` slot; frame entries retain it so
    /// runtime global lookups and `globals()` observe the active function
    /// namespace even when the same code object is re-bound by `types.FunctionType`.
    pub(crate) globals_bits: u64,
}

const TRACEBACK_PAYLOAD_CODE_OFFSET: usize = 0;
const TRACEBACK_PAYLOAD_LINE_OFFSET: usize = std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_COL_OFFSET: usize = 2 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_END_COL_OFFSET: usize = 3 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_NEXT_OFFSET: usize = 4 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_SIZE: usize =
    std::mem::size_of::<MoltHeader>() + 5 * std::mem::size_of::<u64>();

pub(crate) unsafe fn traceback_payload_code_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_CODE_OFFSET) as *const u64) }
}

pub(crate) unsafe fn traceback_payload_line(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_LINE_OFFSET) as *const i64) }
}

pub(crate) unsafe fn traceback_payload_col(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_COL_OFFSET) as *const i64) }
}

pub(crate) unsafe fn traceback_payload_end_col(ptr: *mut u8) -> i64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_END_COL_OFFSET) as *const i64) }
}

pub(crate) unsafe fn traceback_payload_next_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_NEXT_OFFSET) as *const u64) }
}

pub(crate) fn traceback_payload_is_lazy(bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe { object_type_id(ptr) == TYPE_ID_TRACEBACK_PAYLOAD }
}

// --- Frame stack and traceback helpers ---

fn frame_stack_push_entry(code_bits: u64, globals_bits: u64) {
    let line = if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_CODE {
                code_firstlineno(ptr)
            } else {
                0
            }
        }
    } else {
        0
    };
    FRAME_STACK.with(|stack| {
        stack.borrow_mut().push(FrameEntry {
            code_bits,
            line,
            col_offset: -1,
            end_col_offset: -1,
            locals_bits: 0,
            globals_bits,
        });
    });
}

pub(crate) fn frame_stack_push(_py: &PyToken<'_>, code_bits: u64) {
    crate::gil_assert();
    if code_bits != 0 {
        inc_ref_bits(_py, code_bits);
    }
    frame_stack_push_entry(code_bits, 0);
}

pub(crate) fn frame_stack_push_function(_py: &PyToken<'_>, code_bits: u64, func_ptr: *mut u8) {
    crate::gil_assert();
    if code_bits != 0 {
        inc_ref_bits(_py, code_bits);
    }
    let globals_bits = unsafe {
        if !func_ptr.is_null()
            && object_type_id(func_ptr) == TYPE_ID_FUNCTION
            && function_globals_override_enabled(func_ptr)
        {
            function_globals_bits(func_ptr)
        } else {
            0
        }
    };
    if globals_bits != 0 && !obj_from_bits(globals_bits).is_none() {
        inc_ref_bits(_py, globals_bits);
        frame_stack_push_entry(code_bits, globals_bits);
    } else {
        frame_stack_push_entry(code_bits, 0);
    }
}

/// Push a frame entry for a code object reference already owned by the caller.
///
/// The frame stack takes ownership and releases it in `frame_stack_pop`.
/// Use this for runtime registries that acquire a strong reference as part of
/// lookup; calling `frame_stack_push` there would create a second transient
/// ownership protocol at every call site.
pub(crate) fn frame_stack_push_owned(_py: &PyToken<'_>, code_bits: u64) {
    crate::gil_assert();
    frame_stack_push_entry(code_bits, 0);
}

pub(crate) fn frame_stack_active_globals_bits() -> u64 {
    FRAME_STACK.with(|stack| {
        stack
            .borrow()
            .last()
            .map(|entry| entry.globals_bits)
            .unwrap_or(0)
    })
}

pub(crate) fn frame_stack_set_line(line: i64) {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().last_mut() {
            entry.line = line;
            entry.col_offset = -1;
            entry.end_col_offset = -1;
        }
    });
}

pub(crate) fn frame_stack_set_line_col(line: i64, col_offset: i64, end_col_offset: i64) {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().last_mut() {
            entry.line = line;
            entry.col_offset = col_offset;
            entry.end_col_offset = end_col_offset;
        }
    });
}

pub(crate) fn frame_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
    let entry = FRAME_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some(entry) = entry {
        if entry.code_bits != 0 {
            dec_ref_bits(_py, entry.code_bits);
        }
        if entry.locals_bits != 0 && !obj_from_bits(entry.locals_bits).is_none() {
            dec_ref_bits(_py, entry.locals_bits);
        }
        if entry.globals_bits != 0 && !obj_from_bits(entry.globals_bits).is_none() {
            dec_ref_bits(_py, entry.globals_bits);
        }
    }
}

/// Return (filename, lineno, function_name, col_offset, end_col_offset) from
/// the top frame, if available.  col_offset/end_col_offset are -1 when unknown.
pub(crate) fn frame_stack_top_info(_py: &PyToken<'_>) -> Option<(String, i64, String, i64, i64)> {
    FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        let entry = stack.last()?;
        if entry.code_bits == 0 {
            return None;
        }
        let ptr = obj_from_bits(entry.code_bits).as_ptr()?;
        unsafe {
            if object_type_id(ptr) != TYPE_ID_CODE {
                return None;
            }
            let filename_bits = code_filename_bits(ptr);
            let filename = string_obj_to_owned(obj_from_bits(filename_bits))
                .unwrap_or_else(|| "<unknown>".to_string());
            let name_bits = code_name_bits(ptr);
            let name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<module>".to_string());
            Some((
                filename,
                entry.line,
                name,
                entry.col_offset,
                entry.end_col_offset,
            ))
        }
    })
}

pub(crate) fn frame_stack_set_locals_dict(_py: &PyToken<'_>, dict_bits: u64) {
    crate::gil_assert();
    let prev = FRAME_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let entry = stack.last_mut()?;
        // Replace and manage refcounts. 0 means "unset".
        let prev = entry.locals_bits;
        entry.locals_bits = 0;
        if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
            inc_ref_bits(_py, dict_bits);
            entry.locals_bits = dict_bits;
        }
        Some(prev)
    });
    if let Some(prev) = prev
        && prev != 0
        && !obj_from_bits(prev).is_none()
    {
        dec_ref_bits(_py, prev);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_set(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug = std::env::var("MOLT_DEBUG_LOCALS").as_deref() == Ok("1");
        if debug {
            let (depth, top_locals, top_code) = FRAME_STACK.with(|stack| {
                let stack = stack.borrow();
                let depth = stack.len();
                let (locals, code) = stack
                    .last()
                    .map(|e| (e.locals_bits, e.code_bits))
                    .unwrap_or((0, 0));
                (depth, locals, code)
            });
            eprintln!(
                "molt debug locals frame_locals_set depth={} prev_locals=0x{:016x} code=0x{:016x} new=0x{:016x}",
                depth, top_locals, top_code, dict_bits
            );
        }
        frame_stack_set_locals_dict(_py, dict_bits);
        MoltObject::from_bool(true).bits()
    })
}

#[derive(Clone, Copy)]
struct FrameField {
    bits: u64,
    owned: bool,
}

/// Push a frame entry onto the frame stack.  Called by native-backend
/// module chunk functions at entry to populate traceback file/line info.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_push(code_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        frame_stack_push(_py, code_bits);
        MoltObject::none().bits()
    })
}

/// Push a frame entry from filename/name string bits and a line number.
/// Allocates a temporary code object internally.  Preferred for module
/// chunks where no pre-built code object exists.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_push_info(filename_bits: u64, name_bits: u64, lineno: i64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        use crate::object::builders::alloc_code_obj;
        let code_ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            lineno,
            0, // linetable (None — not needed for traceback)
            0, // varnames (None — not needed for traceback)
            0, // names (None — not needed for traceback)
            0, // argcount
            0, // posonlyargcount
            0, // kwonlyargcount
        );
        let code_bits = MoltObject::from_ptr(code_ptr).bits();
        frame_stack_push_owned(_py, code_bits);
        MoltObject::none().bits()
    })
}

/// Update the current line number on the top frame stack entry.
/// Called by `line` ops in module chunk functions for accurate tracebacks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_set_line(line: i64) -> u64 {
    frame_stack_set_line(line);
    0
}

/// Update line and column offsets on the top frame stack entry.
/// Called by `line` ops that carry column offset info for caret annotations.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_set_line_col(line: i64, col_offset: i64, end_col_offset: i64) -> u64 {
    frame_stack_set_line_col(line, col_offset, end_col_offset);
    0
}

/// Update only column offsets on the top frame entry (line unchanged).
/// Called before potentially-raising ops that carry expression-level col info.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_set_col(col_offset: i64, end_col_offset: i64) -> u64 {
    FRAME_STACK.with(|stack| {
        if let Some(entry) = stack.borrow_mut().last_mut() {
            entry.col_offset = col_offset;
            entry.end_col_offset = end_col_offset;
        }
    });
    0
}

/// Pop a frame entry from the frame stack.  Called at module chunk exit.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_pop() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        frame_stack_pop(_py);
        MoltObject::none().bits()
    })
}

unsafe fn alloc_empty_dict_field(_py: &PyToken<'_>) -> Option<FrameField> {
    let ptr = alloc_dict_with_pairs(_py, &[]);
    if ptr.is_null() {
        None
    } else {
        Some(FrameField {
            bits: MoltObject::from_ptr(ptr).bits(),
            owned: true,
        })
    }
}

unsafe fn frame_line_from_entry(entry: FrameEntry) -> Option<i64> {
    unsafe {
        if entry.code_bits == 0 {
            return None;
        }
        let code_ptr = obj_from_bits(entry.code_bits).as_ptr()?;
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            return None;
        }
        let mut line = entry.line;
        if line <= 0 {
            line = code_firstlineno(code_ptr);
        }
        Some(line)
    }
}

unsafe fn code_is_module(code_bits: u64) -> bool {
    unsafe {
        let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
            return false;
        };
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            return false;
        }
        let name_bits = code_name_bits(code_ptr);
        string_obj_to_owned(obj_from_bits(name_bits)).is_some_and(|name| name == "<module>")
    }
}

unsafe fn frame_globals_field_for_code(_py: &PyToken<'_>, code_bits: u64) -> Option<FrameField> {
    unsafe {
        let mut filename: Option<String> = None;
        if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr()
            && object_type_id(code_ptr) == TYPE_ID_CODE
        {
            let filename_bits = code_filename_bits(code_ptr);
            filename = string_obj_to_owned(obj_from_bits(filename_bits));
        }
        let (module_bits, main_bits) = {
            let cache = runtime_state(_py).module_cache.lock().unwrap();
            (
                cache.values().copied().collect::<Vec<u64>>(),
                cache.get("__main__").copied(),
            )
        };
        if let Some(filename) = filename {
            let file_name_bits = intern_runtime_static_name(_py, b"__file__");
            for module_bits in &module_bits {
                let Some(module_ptr) = obj_from_bits(*module_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(module_ptr) != TYPE_ID_MODULE {
                    continue;
                }
                let dict_bits = module_dict_bits(module_ptr);
                let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                let Some(file_bits) = dict_get_in_place(_py, dict_ptr, file_name_bits) else {
                    continue;
                };
                if string_obj_to_owned(obj_from_bits(file_bits))
                    .is_some_and(|value| value == filename)
                {
                    return Some(FrameField {
                        bits: dict_bits,
                        owned: false,
                    });
                }
            }
        }
        if let Some(main_bits) = main_bits
            && let Some(module_ptr) = obj_from_bits(main_bits).as_ptr()
            && object_type_id(module_ptr) == TYPE_ID_MODULE
        {
            let dict_bits = module_dict_bits(module_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                return Some(FrameField {
                    bits: dict_bits,
                    owned: false,
                });
            }
        }
        alloc_empty_dict_field(_py)
    }
}

unsafe fn frame_locals_field_for_code(
    _py: &PyToken<'_>,
    code_bits: u64,
    globals: FrameField,
) -> Option<FrameField> {
    unsafe {
        if code_is_module(code_bits) {
            return Some(FrameField {
                bits: globals.bits,
                owned: false,
            });
        }
        alloc_empty_dict_field(_py)
    }
}

unsafe fn alloc_frame_obj(
    _py: &PyToken<'_>,
    code_bits: u64,
    line: i64,
    back_bits: u64,
) -> Option<u64> {
    unsafe {
        let builtins = builtin_classes(_py);
        let class_obj = obj_from_bits(builtins.frame);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let frame_bits = alloc_instance_for_class_no_pool(_py, class_ptr);
        let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
        let f_code_bits =
            intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
        let f_lineno_bits =
            intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
        let f_lasti_bits =
            intern_static_name(_py, &runtime_state(_py).interned.f_lasti_name, b"f_lasti");
        let f_back_bits =
            intern_static_name(_py, &runtime_state(_py).interned.f_back_name, b"f_back");
        let f_globals_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.f_globals_name,
            b"f_globals",
        );
        let f_locals_bits =
            intern_static_name(_py, &runtime_state(_py).interned.f_locals_name, b"f_locals");
        let globals = frame_globals_field_for_code(_py, code_bits)?;
        let locals = frame_locals_field_for_code(_py, code_bits, globals)?;
        let line_bits = MoltObject::from_int(line).bits();
        let lasti_bits = MoltObject::from_int(-1).bits();
        let dict_ptr = alloc_dict_with_pairs(
            _py,
            &[
                f_code_bits,
                code_bits,
                f_lineno_bits,
                line_bits,
                f_lasti_bits,
                lasti_bits,
                f_back_bits,
                back_bits,
                f_globals_bits,
                globals.bits,
                f_locals_bits,
                locals.bits,
            ],
        );
        if globals.owned {
            dec_ref_bits(_py, globals.bits);
        }
        if locals.owned && locals.bits != globals.bits {
            dec_ref_bits(_py, locals.bits);
        }
        if dict_ptr.is_null() {
            dec_ref_bits(_py, frame_bits);
            return None;
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(_py, frame_ptr, dict_bits);
        object_mark_has_ptrs(_py, frame_ptr);
        Some(frame_bits)
    }
}

unsafe fn alloc_traceback_obj(
    _py: &PyToken<'_>,
    frame_bits: u64,
    line: i64,
    next_bits: u64,
) -> Option<u64> {
    unsafe {
        fn compute_tb_lasti(_py: &PyToken<'_>, frame_bits: u64, line: i64) -> i64 {
            let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() else {
                return -1;
            };
            unsafe {
                let dict_bits = instance_dict_bits(frame_ptr);
                let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                    return -1;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    return -1;
                }
                let f_code_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
                let Some(code_bits) = dict_get_in_place(_py, dict_ptr, f_code_bits) else {
                    return -1;
                };
                let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
                    return -1;
                };
                if object_type_id(code_ptr) != TYPE_ID_CODE {
                    return -1;
                }
                let linetable_bits = code_linetable_bits(code_ptr);
                let Some(linetable_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                    return -1;
                };
                if object_type_id(linetable_ptr) != TYPE_ID_TUPLE {
                    return -1;
                }
                let mut best: Option<(usize, i64)> = None;
                for (idx, entry_bits) in seq_vec_ref(linetable_ptr).iter().copied().enumerate() {
                    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
                        continue;
                    };
                    if object_type_id(entry_ptr) != TYPE_ID_TUPLE {
                        continue;
                    }
                    let parts = seq_vec_ref(entry_ptr);
                    if parts.len() < 4 {
                        continue;
                    }
                    let Some(start_line) = to_i64(obj_from_bits(parts[0])) else {
                        continue;
                    };
                    if start_line != line {
                        continue;
                    }
                    let start_col = to_i64(obj_from_bits(parts[2])).unwrap_or(-1);
                    let end_col = to_i64(obj_from_bits(parts[3])).unwrap_or(start_col);
                    let span = if start_col >= 0 && end_col >= start_col {
                        end_col - start_col
                    } else {
                        -1
                    };
                    match best {
                        Some((_, best_span)) if span <= best_span => {}
                        _ => best = Some((idx, span)),
                    }
                }
                if let Some((idx, _)) = best {
                    return (idx as i64) * 2;
                }
                -1
            }
        }

        let builtins = builtin_classes(_py);
        let class_obj = obj_from_bits(builtins.traceback);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let tb_bits = alloc_instance_for_class_no_pool(_py, class_ptr);
        let tb_ptr = obj_from_bits(tb_bits).as_ptr()?;
        let tb_frame_bits =
            intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
        let tb_lineno_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.tb_lineno_name,
            b"tb_lineno",
        );
        let tb_next_bits =
            intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
        let tb_lasti_bits =
            intern_static_name(_py, &runtime_state(_py).interned.tb_lasti_name, b"tb_lasti");
        let line_bits = MoltObject::from_int(line).bits();
        let lasti_bits = MoltObject::from_int(compute_tb_lasti(_py, frame_bits, line)).bits();
        let dict_ptr = alloc_dict_with_pairs(
            _py,
            &[
                tb_frame_bits,
                frame_bits,
                tb_lineno_bits,
                line_bits,
                tb_next_bits,
                next_bits,
                tb_lasti_bits,
                lasti_bits,
            ],
        );
        if dict_ptr.is_null() {
            dec_ref_bits(_py, tb_bits);
            return None;
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(_py, tb_ptr, dict_bits);
        object_mark_has_ptrs(_py, tb_ptr);
        Some(tb_bits)
    }
}

unsafe fn alloc_traceback_payload_obj(
    _py: &PyToken<'_>,
    entry: FrameEntry,
    next_bits: u64,
) -> Option<u64> {
    unsafe {
        if entry.code_bits == 0 {
            return None;
        }
        let line = frame_line_from_entry(entry)?;
        let ptr = alloc_object(_py, TRACEBACK_PAYLOAD_SIZE, TYPE_ID_TRACEBACK_PAYLOAD);
        if ptr.is_null() {
            return None;
        }
        *(ptr.add(TRACEBACK_PAYLOAD_CODE_OFFSET) as *mut u64) = entry.code_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_LINE_OFFSET) as *mut i64) = line;
        *(ptr.add(TRACEBACK_PAYLOAD_COL_OFFSET) as *mut i64) = entry.col_offset;
        *(ptr.add(TRACEBACK_PAYLOAD_END_COL_OFFSET) as *mut i64) = entry.end_col_offset;
        *(ptr.add(TRACEBACK_PAYLOAD_NEXT_OFFSET) as *mut u64) = next_bits;
        inc_ref_bits(_py, entry.code_bits);
        inc_ref_bits(_py, next_bits);
        object_mark_has_ptrs(_py, ptr);
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

unsafe fn build_frame_chain(_py: &PyToken<'_>, entries: &[FrameEntry]) -> Option<Vec<(u64, i64)>> {
    unsafe {
        let mut out: Vec<(u64, i64)> = Vec::with_capacity(entries.len());
        let mut back_bits = MoltObject::none().bits();
        for entry in entries {
            let Some(line) = frame_line_from_entry(*entry) else {
                continue;
            };
            let frame_bits = match alloc_frame_obj(_py, entry.code_bits, line, back_bits) {
                Some(bits) => bits,
                None => {
                    for (bits, _) in out {
                        dec_ref_bits(_py, bits);
                    }
                    return None;
                }
            };
            back_bits = frame_bits;
            out.push((frame_bits, line));
        }
        Some(out)
    }
}

pub(crate) fn frame_stack_trace_payload_bits(
    _py: &PyToken<'_>,
    handler_frame_index: Option<usize>,
    include_caller_frame: bool,
) -> Option<u64> {
    FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        if stack.is_empty() {
            return None;
        }
        let start = handler_frame_index
            .map(|idx| {
                if include_caller_frame {
                    idx.saturating_sub(1)
                } else {
                    idx
                }
            })
            .unwrap_or(0)
            .min(stack.len());
        let active = stack[start..].to_vec();
        if active.is_empty() {
            return None;
        }
        let mut next_bits = MoltObject::none().bits();
        let mut built_any = false;
        for entry in active.iter().rev().copied() {
            if unsafe { frame_line_from_entry(entry) }.is_none() {
                continue;
            }
            unsafe {
                let Some(payload_bits) = alloc_traceback_payload_obj(_py, entry, next_bits) else {
                    if !obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(_py, next_bits);
                    }
                    return None;
                };
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(_py, next_bits);
                }
                next_bits = payload_bits;
                built_any = true;
            }
        }
        if built_any && !obj_from_bits(next_bits).is_none() {
            Some(next_bits)
        } else {
            None
        }
    })
}

pub(crate) fn traceback_payload_to_traceback_bits(_py: &PyToken<'_>, payload_bits: u64) -> u64 {
    let mut payload_entries: Vec<(u64, i64)> = Vec::new();
    let mut current_bits = payload_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 1024 {
            break;
        }
        let Some(ptr) = obj_from_bits(current_bits).as_ptr() else {
            break;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TRACEBACK_PAYLOAD {
                break;
            }
            payload_entries.push((
                traceback_payload_code_bits(ptr),
                traceback_payload_line(ptr),
            ));
            current_bits = traceback_payload_next_bits(ptr);
        }
        depth += 1;
    }
    if payload_entries.is_empty() {
        return MoltObject::none().bits();
    }
    let mut frames: Vec<(u64, i64)> = Vec::with_capacity(payload_entries.len());
    let mut back_bits = MoltObject::none().bits();
    unsafe {
        for (code_bits, line) in payload_entries.iter().copied() {
            let Some(frame_bits) = alloc_frame_obj(_py, code_bits, line, back_bits) else {
                for (bits, _) in frames {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };
            back_bits = frame_bits;
            frames.push((frame_bits, line));
        }
        let mut next_bits = MoltObject::none().bits();
        let mut built_any = false;
        let mut frames_built: u64 = 0;
        for (frame_bits, line) in frames.iter().rev().copied() {
            let Some(tb_bits) = alloc_traceback_obj(_py, frame_bits, line, next_bits) else {
                if !obj_from_bits(next_bits).is_none() {
                    dec_ref_bits(_py, next_bits);
                }
                for (bits, _) in frames.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };
            if !obj_from_bits(next_bits).is_none() {
                dec_ref_bits(_py, next_bits);
            }
            next_bits = tb_bits;
            built_any = true;
            frames_built += 1;
        }
        for (bits, _) in frames.iter().copied() {
            dec_ref_bits(_py, bits);
        }
        if !built_any || obj_from_bits(next_bits).is_none() {
            if !obj_from_bits(next_bits).is_none() {
                dec_ref_bits(_py, next_bits);
            }
            return MoltObject::none().bits();
        }
        if profile_enabled(_py) {
            TRACEBACK_BUILD_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
            TRACEBACK_BUILD_FRAMES.fetch_add(frames_built, AtomicOrdering::Relaxed);
        }
        next_bits
    }
}

pub(crate) fn exception_materialize_traceback_bits(_py: &PyToken<'_>, exc_ptr: *mut u8) -> u64 {
    crate::gil_assert();
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return MoltObject::none().bits();
        }
        let trace_slot = exc_ptr.add(5 * std::mem::size_of::<u64>()) as *mut u64;
        let trace_bits = *trace_slot;
        if !traceback_payload_is_lazy(trace_bits) {
            return trace_bits;
        }
        let materialized_bits = traceback_payload_to_traceback_bits(_py, trace_bits);
        if obj_from_bits(materialized_bits).is_none() {
            return MoltObject::none().bits();
        }
        *trace_slot = materialized_bits;
        dec_ref_bits(_py, trace_bits);
        materialized_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getframe(depth_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let depth_val = obj_from_bits(depth_bits);
        let Some(depth) = to_i64(depth_val) else {
            return raise_exception::<u64>(_py, "TypeError", "depth must be an integer");
        };
        if depth < 0 {
            return raise_exception::<u64>(_py, "ValueError", "depth must be >= 0");
        }
        let depth = depth as usize;
        let entries = FRAME_STACK.with(|stack| {
            let stack = stack.borrow();
            if depth >= stack.len() {
                None
            } else {
                Some(stack[..=stack.len() - 1 - depth].to_vec())
            }
        });
        let Some(entries) = entries else {
            return MoltObject::none().bits();
        };
        unsafe {
            if let Some(frames) = build_frame_chain(_py, &entries) {
                if let Some((frame_bits, _)) = frames.last().copied() {
                    inc_ref_bits(_py, frame_bits);
                    for (bits, _) in frames {
                        dec_ref_bits(_py, bits);
                    }
                    return frame_bits;
                }
                for (bits, _) in frames {
                    dec_ref_bits(_py, bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

fn empty_dict_bits(_py: &PyToken<'_>) -> u64 {
    let ptr = alloc_dict_with_pairs(_py, &[]);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

unsafe fn code_is_molt_builtin(code_bits: u64) -> bool {
    unsafe {
        if code_bits == 0 {
            return false;
        }
        let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() else {
            return false;
        };
        if object_type_id(code_ptr) != TYPE_ID_CODE {
            return false;
        }
        let filename_bits = code_filename_bits(code_ptr);
        string_obj_to_owned(obj_from_bits(filename_bits))
            .is_some_and(|name| name == "<molt-builtin>")
    }
}

fn top_user_frame_entry() -> Option<FrameEntry> {
    FRAME_STACK.with(|stack| {
        let stack = stack.borrow();
        for entry in stack.iter().rev() {
            unsafe {
                if !code_is_molt_builtin(entry.code_bits) {
                    return Some(*entry);
                }
            }
        }
        None
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_locals_builtin() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let debug = std::env::var("MOLT_DEBUG_LOCALS").as_deref() == Ok("1");
        let entry = top_user_frame_entry();
        if let Some(entry) = entry {
            let bits = entry.locals_bits;
            if bits != 0 && !obj_from_bits(bits).is_none() {
                unsafe {
                    // CPython 3.12+: optimized-function `locals()` returns a snapshot dict at
                    // the call point; module-scope `locals()` stays an alias of globals.
                    if code_is_module(entry.code_bits) {
                        inc_ref_bits(_py, bits);
                        return bits;
                    }
                    if let Some(locals_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(locals_ptr) == TYPE_ID_DICT
                    {
                        let pairs = dict_order(locals_ptr).clone();
                        let out_ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
                // Defensive fallback for non-dict locals payloads.
                inc_ref_bits(_py, bits);
                return bits;
            }
        }
        if debug {
            let (depth, top_locals, top_code) = FRAME_STACK.with(|stack| {
                let stack = stack.borrow();
                let depth = stack.len();
                let (locals, code) = stack
                    .last()
                    .map(|e| (e.locals_bits, e.code_bits))
                    .unwrap_or((0, 0));
                (depth, locals, code)
            });
            eprintln!(
                "molt debug locals locals_builtin fallback depth={} locals=0x{:016x} code=0x{:016x}",
                depth, top_locals, top_code
            );
        }
        // Fallback: for module frames, CPython uses f_locals == f_globals.
        if let Some(entry) = entry {
            unsafe {
                if let Some(field) = frame_globals_field_for_code(_py, entry.code_bits) {
                    let bits = field.bits;
                    if !field.owned && !obj_from_bits(bits).is_none() {
                        inc_ref_bits(_py, bits);
                    }
                    return bits;
                }
            }
        }
        empty_dict_bits(_py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_globals_builtin() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let entry = top_user_frame_entry();
        let Some(entry) = entry else {
            return empty_dict_bits(_py);
        };
        unsafe {
            // Module-scope globals() must resolve to the executing module namespace.
            // When compiler instrumentation has pinned module locals on the frame,
            // prefer that dict directly (module locals == module globals in CPython).
            if code_is_module(entry.code_bits) {
                let bits = entry.locals_bits;
                if bits != 0 && !obj_from_bits(bits).is_none() {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
            }
            if let Some(field) = frame_globals_field_for_code(_py, entry.code_bits) {
                let bits = field.bits;
                if !field.owned && !obj_from_bits(bits).is_none() {
                    inc_ref_bits(_py, bits);
                }
                return bits;
            }
        }
        empty_dict_bits(_py)
    })
}

#[cfg(test)]
mod tests {
    use super::{frame_stack_pop, frame_stack_push, frame_stack_push_owned};
    use crate::object::builders::alloc_code_obj;
    use crate::object::header_from_obj_ptr;
    use crate::{alloc_string, dec_ref_bits, inc_ref_bits};
    use molt_obj_model::MoltObject;
    use std::sync::atomic::Ordering;

    unsafe fn ref_count(ptr: *mut u8) -> u32 {
        unsafe {
            (*header_from_obj_ptr(ptr))
                .ref_count
                .load(Ordering::Relaxed)
        }
    }

    fn alloc_test_code(_py: &crate::PyToken<'_>) -> (*mut u8, u64) {
        let filename_ptr = alloc_string(_py, b"<frame-test>");
        let name_ptr = alloc_string(_py, b"frame_test");
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let code_ptr = alloc_code_obj(_py, filename_bits, name_bits, 7, 0, 0, 0, 0, 0, 0);
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, name_bits);
        (code_ptr, MoltObject::from_ptr(code_ptr).bits())
    }

    #[test]
    fn frame_stack_push_borrowed_balances_refcount_on_pop() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let (code_ptr, code_bits) = alloc_test_code(_py);
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);

            frame_stack_push(_py, code_bits);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);
            frame_stack_pop(_py);
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);

            dec_ref_bits(_py, code_bits);
        });
    }

    #[test]
    fn frame_stack_push_owned_takes_existing_reference() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry_nopanic!(_py, {
            let (code_ptr, code_bits) = alloc_test_code(_py);
            inc_ref_bits(_py, code_bits);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);

            frame_stack_push_owned(_py, code_bits);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);
            frame_stack_pop(_py);
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);

            dec_ref_bits(_py, code_bits);
        });
    }
}
