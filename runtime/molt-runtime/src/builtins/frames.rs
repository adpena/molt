use crate::PyToken;
use crate::builtins::exceptions::raise_exception;
use crate::{
    FRAME_STACK, MoltHeader, TRACEBACK_BUILD_COUNT, TRACEBACK_BUILD_FRAMES, TYPE_ID_CODE,
    TYPE_ID_DICT, TYPE_ID_EXCEPTION, TYPE_ID_MODULE, TYPE_ID_TRACEBACK_PAYLOAD, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, alloc_dict_with_pairs, alloc_instance_for_class, alloc_object, builtin_classes,
    code_filename_bits, code_firstlineno, code_linetable_bits, code_name_bits, dec_ref_bits,
    dict_get_in_place, exception_pending, inc_ref_bits, instance_dict_bits, instance_set_dict_bits,
    intern_static_name, module_dict_bits, obj_from_bits, object_mark_has_ptrs, object_type_id,
    profile_enabled, runtime_state, string_obj_to_owned, to_i64,
};
use molt_obj_model::MoltObject;
use std::sync::atomic::Ordering as AtomicOrdering;

mod namespace;
#[cfg(test)]
mod namespace_tests;
pub(crate) use namespace::{
    CodeNamespace, CompiledCodeSlot, FrameInvocationGuard, acquire_pending_invocation_context,
    compiled_slot_for_code, globals_namespace_storage_bits, globals_namespace_storage_ptr,
    take_invocation_namespace, take_pending_namespaces_for_teardown,
};

/// The executing Python argument-zero slot, independent of ABI-only parameters.
#[derive(Clone, Copy, Default)]
pub(crate) enum PythonArgumentZero {
    #[default]
    NoArgument,
    Value(u64),
    Cell(u64),
}

impl PythonArgumentZero {
    fn retained_bits(self) -> Option<u64> {
        match self {
            Self::NoArgument => None,
            Self::Value(bits) | Self::Cell(bits) => Some(bits),
        }
    }
}

/// Live lexical context; cells are the compiler's canonical one-item list cells.
#[derive(Clone, Copy, Default)]
pub(crate) struct PythonFrameContext {
    pub(crate) argument_zero: PythonArgumentZero,
    pub(crate) class_cell_bits: Option<u64>,
}

impl PythonFrameContext {
    fn retain(self, py: &PyToken<'_>) {
        for bits in self
            .argument_zero
            .retained_bits()
            .into_iter()
            .chain(self.class_cell_bits)
        {
            inc_ref_bits(py, bits);
        }
    }

    fn release(self, py: &PyToken<'_>) {
        for bits in self
            .argument_zero
            .retained_bits()
            .into_iter()
            .chain(self.class_cell_bits)
        {
            dec_ref_bits(py, bits);
        }
    }
}

/// A retained snapshot survives arbitrary reentry during receiver validation.
pub(crate) struct PythonFrameContextSnapshot<'a, 'py> {
    py: &'a PyToken<'py>,
    pub(crate) context: PythonFrameContext,
}

impl Drop for PythonFrameContextSnapshot<'_, '_> {
    fn drop(&mut self) {
        self.context.release(self.py);
    }
}

pub(crate) fn frame_python_context_snapshot<'a, 'py>(
    py: &'a PyToken<'py>,
) -> PythonFrameContextSnapshot<'a, 'py> {
    let context = FRAME_STACK.with(|stack| {
        let context = stack
            .borrow()
            .last()
            .map(|entry| entry.python_context)
            .unwrap_or_default();
        context.retain(py);
        context
    });
    PythonFrameContextSnapshot { py, context }
}

pub(crate) fn frame_stack_set_python_context(
    py: &PyToken<'_>,
    context: PythonFrameContext,
) -> bool {
    // Retain before publication. A replaced value can run finalizers on release.
    context.retain(py);
    let previous = FRAME_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let entry = stack.last_mut()?;
        Some(std::mem::replace(&mut entry.python_context, context))
    });
    if let Some(previous) = previous {
        previous.release(py);
        true
    } else {
        context.release(py);
        false
    }
}

fn frame_context_cell_is_valid(bits: u64) -> bool {
    crate::object::cells::cell_ptr_from_bits(bits).is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_context_set(
    argument_zero_bits: u64,
    argument_kind_bits: u64,
    class_cell_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let argument_zero = match to_i64(obj_from_bits(argument_kind_bits)) {
            Some(0) => PythonArgumentZero::NoArgument,
            Some(1) => PythonArgumentZero::Value(argument_zero_bits),
            Some(2) if frame_context_cell_is_valid(argument_zero_bits) => {
                PythonArgumentZero::Cell(argument_zero_bits)
            }
            _ => {
                return raise_exception::<_>(
                    py,
                    "SystemError",
                    "invalid executing-frame argument-zero transport",
                );
            }
        };
        let class_cell_bits = if obj_from_bits(class_cell_bits).is_none() {
            None
        } else if frame_context_cell_is_valid(class_cell_bits) {
            Some(class_cell_bits)
        } else {
            return raise_exception::<_>(
                py,
                "SystemError",
                "invalid executing-frame class-cell transport",
            );
        };
        if !frame_stack_set_python_context(
            py,
            PythonFrameContext {
                argument_zero,
                class_cell_bits,
            },
        ) {
            return raise_exception::<_>(
                py,
                "SystemError",
                "executing-frame context published without a Python frame",
            );
        }
        MoltObject::none().bits()
    })
}

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
    /// Effective `f_builtins` selected when the frame is created. This retained
    /// object is not a live re-read of globals["__builtins__"]; modules are
    /// normalized to their dictionaries, while every other supplied value is
    /// preserved for normal lookup error semantics.
    pub(crate) builtins_bits: u64,
    pub(crate) python_context: PythonFrameContext,
}

const TRACEBACK_PAYLOAD_CODE_OFFSET: usize = 0;
const TRACEBACK_PAYLOAD_LINE_OFFSET: usize = std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_COL_OFFSET: usize = 2 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_END_COL_OFFSET: usize = 3 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_NEXT_OFFSET: usize = 4 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_GLOBALS_OFFSET: usize = 5 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_LOCALS_OFFSET: usize = 6 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_BUILTINS_OFFSET: usize = 7 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_SIZE: usize =
    std::mem::size_of::<MoltHeader>() + 8 * std::mem::size_of::<u64>();

pub(crate) const TRACEBACK_PAYLOAD_OWNED_SLOTS: [usize; 5] = [0, 4, 5, 6, 7];

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

unsafe fn traceback_payload_frame_entry(ptr: *mut u8) -> FrameEntry {
    unsafe {
        FrameEntry {
            code_bits: traceback_payload_code_bits(ptr),
            line: traceback_payload_line(ptr),
            col_offset: traceback_payload_col(ptr),
            end_col_offset: traceback_payload_end_col(ptr),
            globals_bits: *(ptr.add(TRACEBACK_PAYLOAD_GLOBALS_OFFSET) as *const u64),
            locals_bits: *(ptr.add(TRACEBACK_PAYLOAD_LOCALS_OFFSET) as *const u64),
            builtins_bits: *(ptr.add(TRACEBACK_PAYLOAD_BUILTINS_OFFSET) as *const u64),
            python_context: PythonFrameContext::default(),
        }
    }
}

pub(crate) fn traceback_payload_is_lazy(bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe { object_type_id(ptr) == TYPE_ID_TRACEBACK_PAYLOAD }
}

// --- Frame stack and traceback helpers ---

fn normalize_builtins_value(bits: u64) -> u64 {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return bits;
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_MODULE {
        return bits;
    }
    let dict_bits = unsafe { module_dict_bits(ptr) };
    obj_from_bits(dict_bits)
        .as_ptr()
        .is_some_and(|dict_ptr| unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT)
        .then_some(dict_bits)
        .unwrap_or(bits)
}

pub(crate) fn frame_effective_builtins_bits(_py: &PyToken<'_>, globals_bits: u64) -> u64 {
    let globals_ptr = globals_namespace_storage_ptr(_py, globals_bits);
    if let Some(globals_ptr) = globals_ptr {
        let key_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.dunder_builtins_name,
            b"__builtins__",
        );
        if let Some(value_bits) = unsafe { dict_get_in_place(_py, globals_ptr, key_bits) } {
            return normalize_builtins_value(value_bits);
        }
    }
    if exception_pending(_py) {
        return 0;
    }
    if let Some(bits) =
        FRAME_STACK.with(|stack| stack.borrow().last().map(|entry| entry.builtins_bits))
    {
        return bits;
    }
    let cached = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        cache.lock().unwrap().get("builtins").copied()
    };
    if let Some(bits) = cached {
        return normalize_builtins_value(bits);
    }
    // A compiled application admits its builtins body in the module registry.
    // Materialize that canonical namespace before capture, never substitute a
    // later cache value into an already executing frame. The builtins body
    // publishes its dictionary before entering its own frame, closing cycles.
    if !crate::exception_pending(_py)
        && crate::builtins::module_table::module_execution_target_has_body("builtins") == Some(true)
    {
        let id = crate::builtins::module_table::module_id_of("builtins")
            .expect("admitted builtins registry row");
        let module = crate::builtins::module_table::module_ensure(_py, id);
        let bits = normalize_builtins_value(module);
        // The module table owns this published namespace after ensure returns.
        dec_ref_bits(_py, module);
        return bits;
    }
    0
}

/// Transfer an explicitly owned code/namespace triple to the sole Python frame.
pub(crate) fn frame_stack_push_owned(
    _py: &PyToken<'_>,
    code_bits: u64,
    globals_bits: u64,
    builtins_bits: u64,
) {
    crate::gil_assert();
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
            builtins_bits,
            python_context: PythonFrameContext::default(),
        });
    });
}

#[cfg(test)]
pub(crate) fn frame_stack_push(_py: &PyToken<'_>, code_bits: u64) {
    crate::gil_assert();
    if code_bits != 0 {
        inc_ref_bits(_py, code_bits);
    }
    frame_stack_push_owned(_py, code_bits, 0, 0);
}

pub(crate) fn frame_stack_active_code_bits() -> u64 {
    FRAME_STACK.with(|stack| stack.borrow().last().map_or(0, |entry| entry.code_bits))
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

pub(crate) fn frame_stack_active_builtins_bits() -> u64 {
    FRAME_STACK.with(|stack| {
        stack
            .borrow()
            .last()
            .map(|entry| entry.builtins_bits)
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

impl FrameEntry {
    fn retain(self, py: &PyToken<'_>) {
        for bits in [
            self.code_bits,
            self.locals_bits,
            self.globals_bits,
            self.builtins_bits,
        ] {
            inc_ref_bits(py, bits);
        }
        self.python_context.retain(py);
    }

    /// One edge-release authority for normal return and thread teardown.
    pub(crate) fn release(self, py: &PyToken<'_>) {
        for bits in [
            self.code_bits,
            self.locals_bits,
            self.globals_bits,
            self.builtins_bits,
        ] {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                dec_ref_bits(py, bits);
            }
        }
        self.python_context.release(py);
    }
}

/// Own snapshot edges while allocations or finalizers can reenter the runtime.
struct FrameStackSnapshot<'a, 'py> {
    py: &'a PyToken<'py>,
    entries: Vec<FrameEntry>,
}

impl<'a, 'py> FrameStackSnapshot<'a, 'py> {
    fn retained(py: &'a PyToken<'py>, entries: Vec<FrameEntry>) -> Self {
        for entry in &entries {
            entry.retain(py);
        }
        Self { py, entries }
    }

    fn capture(py: &'a PyToken<'py>, select: impl FnOnce(&[FrameEntry]) -> &[FrameEntry]) -> Self {
        let entries = FRAME_STACK.with(|stack| {
            let stack = stack.borrow();
            select(&stack).to_vec()
        });
        Self::retained(py, entries)
    }
}

impl Drop for FrameStackSnapshot<'_, '_> {
    fn drop(&mut self) {
        for entry in self.entries.drain(..) {
            entry.release(self.py);
        }
    }
}

pub(crate) fn frame_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
    let entry = FRAME_STACK.with(|stack| stack.borrow_mut().pop());
    if let Some(entry) = entry {
        entry.release(_py);
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

fn frame_globals_field(_py: &PyToken<'_>, entry: FrameEntry) -> Option<FrameField> {
    if globals_namespace_storage_bits(_py, entry.globals_bits).is_some() {
        return Some(FrameField {
            bits: entry.globals_bits,
            owned: false,
        });
    }
    if exception_pending(_py) {
        return None;
    }
    raise_exception::<u64>(
        _py,
        "SystemError",
        "Python frame has no bound globals namespace",
    );
    None
}

unsafe fn frame_locals_field(
    _py: &PyToken<'_>,
    entry: FrameEntry,
    globals: FrameField,
) -> Option<FrameField> {
    unsafe {
        if entry.locals_bits != 0 && !obj_from_bits(entry.locals_bits).is_none() {
            return Some(FrameField {
                bits: entry.locals_bits,
                owned: false,
            });
        }
        if code_is_module(entry.code_bits) {
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
    entry: FrameEntry,
    line: i64,
    back_bits: u64,
    lasti: i64,
) -> Option<u64> {
    unsafe {
        let names = &runtime_state(_py).interned;
        let name = |slot, bytes| {
            let bits = intern_static_name(_py, slot, bytes);
            (bits != 0).then_some(bits)
        };
        // Stop at the first failed intern before another allocation can replace
        // its exception or an unpublished frame acquires cleanup obligations.
        let f_code_bits = name(&names.f_code_name, b"f_code")?;
        let f_lineno_bits = name(&names.f_lineno_name, b"f_lineno")?;
        let f_lasti_bits = name(&names.f_lasti_name, b"f_lasti")?;
        let f_back_bits = name(&names.f_back_name, b"f_back")?;
        let f_globals_bits = name(&names.f_globals_name, b"f_globals")?;
        let f_locals_bits = name(&names.f_locals_name, b"f_locals")?;
        let f_builtins_bits = name(&names.f_builtins_name, b"f_builtins")?;
        let builtins = builtin_classes(_py);
        let class_obj = obj_from_bits(builtins.frame);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let frame_bits = alloc_instance_for_class(_py, class_ptr);
        let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
        let Some(globals) = frame_globals_field(_py, entry) else {
            dec_ref_bits(_py, frame_bits);
            return None;
        };
        let Some(locals) = frame_locals_field(_py, entry, globals) else {
            if globals.owned {
                dec_ref_bits(_py, globals.bits);
            }
            dec_ref_bits(_py, frame_bits);
            return None;
        };
        let line_bits = MoltObject::from_int(line).bits();
        let lasti_bits = MoltObject::from_int(lasti).bits();
        let dict_ptr = alloc_dict_with_pairs(
            _py,
            &[
                f_code_bits,
                entry.code_bits,
                f_lineno_bits,
                line_bits,
                f_lasti_bits,
                lasti_bits,
                f_back_bits,
                back_bits,
                f_globals_bits,
                globals.bits,
                f_builtins_bits,
                entry.builtins_bits,
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

/// Materialize every suspended view through the same frame authority as live
/// stacks and tracebacks. Runtime-native tasks have no Python frame to invent.
pub(crate) unsafe fn suspended_frame_bits(py: &PyToken<'_>, ptr: *mut u8, lasti: i64) -> u64 {
    unsafe {
        let [globals_bits, builtins_bits, code_bits] =
            crate::object::aux_header::object_frame_context_bits(ptr);
        if code_bits == 0 {
            return MoltObject::none().bits();
        }
        let mut entry = FrameEntry {
            code_bits,
            globals_bits,
            builtins_bits,
            locals_bits: 0,
            line: 0,
            col_offset: -1,
            end_col_offset: -1,
            python_context: PythonFrameContext::default(),
        };
        let Some(line) = frame_line_from_entry(entry) else {
            return raise_exception::<u64>(py, "SystemError", "suspended frame has invalid code");
        };
        if object_type_id(ptr) == crate::TYPE_ID_GENERATOR {
            entry.locals_bits = crate::async_rt::generators::generator_locals_dict(py, ptr);
            if crate::exception_pending(py) || obj_from_bits(entry.locals_bits).is_none() {
                dec_ref_bits(py, entry.locals_bits);
                return MoltObject::none().bits();
            }
        }
        let frame = alloc_frame_obj(py, entry, line, MoltObject::none().bits(), lasti);
        dec_ref_bits(py, entry.locals_bits);
        frame.unwrap_or_else(|| MoltObject::none().bits())
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
                let Some(linetable) = crate::object::seq_access::pin_tuple(_py, linetable_ptr)
                else {
                    return -1;
                };
                let mut best: Option<(usize, i64)> = None;
                for (idx, entry_bits) in linetable.iter().copied().enumerate() {
                    let Some(entry_ptr) = obj_from_bits(entry_bits).as_ptr() else {
                        continue;
                    };
                    if object_type_id(entry_ptr) != TYPE_ID_TUPLE {
                        continue;
                    }
                    let Some(parts) =
                        crate::object::seq_access::with_immutable_tuple_slice(entry_ptr, |parts| {
                            (parts.len() >= 4).then(|| [parts[0], parts[1], parts[2], parts[3]])
                        })
                        .flatten()
                    else {
                        continue;
                    };
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
        let tb_bits = alloc_instance_for_class(_py, class_ptr);
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
        *(ptr.add(TRACEBACK_PAYLOAD_GLOBALS_OFFSET) as *mut u64) = entry.globals_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_LOCALS_OFFSET) as *mut u64) = entry.locals_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_BUILTINS_OFFSET) as *mut u64) = entry.builtins_bits;
        for slot in TRACEBACK_PAYLOAD_OWNED_SLOTS {
            inc_ref_bits(
                _py,
                *(ptr.add(slot * std::mem::size_of::<u64>()) as *const u64),
            );
        }
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
            let frame_bits = match alloc_frame_obj(_py, *entry, line, back_bits, -1) {
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
    let snapshot = FrameStackSnapshot::capture(_py, |stack| {
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
        &stack[start..]
    });
    let active = &snapshot.entries;
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
}

pub(crate) fn traceback_payload_to_traceback_bits(_py: &PyToken<'_>, payload_bits: u64) -> u64 {
    let mut payload_entries: Vec<FrameEntry> = Vec::new();
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
            payload_entries.push(traceback_payload_frame_entry(ptr));
            current_bits = traceback_payload_next_bits(ptr);
        }
        depth += 1;
    }
    if payload_entries.is_empty() {
        return MoltObject::none().bits();
    }
    let snapshot = FrameStackSnapshot::retained(_py, payload_entries);
    unsafe {
        let Some(frames) = build_frame_chain(_py, &snapshot.entries) else {
            return MoltObject::none().bits();
        };
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
        let trace_bits = crate::exception_trace_bits(exc_ptr);
        if !traceback_payload_is_lazy(trace_bits) {
            return trace_bits;
        }
        let materialized_bits = traceback_payload_to_traceback_bits(_py, trace_bits);
        if obj_from_bits(materialized_bits).is_none() {
            return MoltObject::none().bits();
        }
        crate::builtins::exceptions::exception_publish_field_slot(
            _py,
            exc_ptr,
            crate::builtins::exceptions::ExceptionFieldSlot::Traceback,
            materialized_bits,
        );
        dec_ref_bits(_py, materialized_bits);
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
        let snapshot = FrameStackSnapshot::capture(_py, |stack| {
            if depth >= stack.len() {
                &[]
            } else {
                &stack[..=stack.len() - 1 - depth]
            }
        });
        unsafe {
            if let Some(frames) = build_frame_chain(_py, &snapshot.entries) {
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

fn top_user_frame_entry() -> Option<FrameEntry> {
    // Runtime builtins no longer manufacture Python frames at dispatch.
    FRAME_STACK.with(|stack| stack.borrow().last().copied())
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
                    // PEP 667 makes optimized-function snapshots independent
                    // starting in 3.13. Earlier targets reuse the frame cache;
                    // module locals remain the namespace on every target.
                    if code_is_module(entry.code_bits)
                        || !crate::object::ops_sys::runtime_target_at_least(_py, 3, 13)
                    {
                        inc_ref_bits(_py, bits);
                        return bits;
                    }
                    if let Some(locals_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(locals_ptr) == TYPE_ID_DICT
                    {
                        return crate::molt_dict_copy(bits);
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
            if let Some(field) = frame_globals_field(_py, entry) {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_globals_builtin() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let entry = top_user_frame_entry();
        let Some(entry) = entry else {
            return empty_dict_bits(_py);
        };
        if let Some(field) = frame_globals_field(_py, entry) {
            let bits = field.bits;
            if !field.owned && !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        empty_dict_bits(_py)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        frame_stack_active_builtins_bits, frame_stack_pop, frame_stack_push, frame_stack_push_owned,
    };
    use crate::object::builders::{alloc_code_obj, alloc_tuple};
    use crate::object::header_from_obj_ptr;
    use crate::{
        alloc_dict_with_pairs, alloc_string, dec_ref_bits, dict_set_in_place, inc_ref_bits,
    };
    use molt_obj_model::MoltObject;

    unsafe fn ref_count(ptr: *mut u8) -> u32 {
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    pub(super) fn alloc_test_code(_py: &crate::PyToken<'_>) -> (*mut u8, u64) {
        let filename_ptr = alloc_string(_py, b"<frame-test>");
        let name_ptr = alloc_string(_py, b"frame_test");
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let empty_tuple_ptr = alloc_tuple(_py, &[]);
        let empty_tuple_bits = MoltObject::from_ptr(empty_tuple_ptr).bits();
        let code_ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            7,
            MoltObject::none().bits(),
            empty_tuple_bits,
            empty_tuple_bits,
            0,
            0,
            0,
        );
        dec_ref_bits(_py, empty_tuple_bits);
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, name_bits);
        (code_ptr, MoltObject::from_ptr(code_ptr).bits())
    }

    #[test]
    fn python_context_edges_survive_snapshot_replacement_and_pop() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let argument = crate::object::cells::alloc_cell(py, MoltObject::from_int(1).bits());
            let class_cell =
                crate::object::cells::alloc_cell(py, crate::builtin_classes(py).object);
            assert!(!argument.is_null() && !class_cell.is_null());
            let argument_bits = MoltObject::from_ptr(argument).bits();
            let class_bits = MoltObject::from_ptr(class_cell).bits();
            frame_stack_push(py, 0);
            assert!(super::frame_stack_set_python_context(
                py,
                super::PythonFrameContext {
                    argument_zero: super::PythonArgumentZero::Cell(argument_bits),
                    class_cell_bits: Some(class_bits),
                }
            ));
            assert_eq!(unsafe { ref_count(argument) }, 2);
            assert_eq!(unsafe { ref_count(class_cell) }, 2);
            let snapshot = super::frame_python_context_snapshot(py);
            assert_eq!(unsafe { ref_count(argument) }, 3);
            assert!(super::frame_stack_set_python_context(
                py,
                super::PythonFrameContext::default()
            ));
            assert_eq!(unsafe { ref_count(argument) }, 2);
            frame_stack_pop(py);
            assert_eq!(unsafe { ref_count(argument) }, 2);
            drop(snapshot);
            assert_eq!(unsafe { ref_count(argument) }, 1);
            assert_eq!(unsafe { ref_count(class_cell) }, 1);
            dec_ref_bits(py, argument_bits);
            dec_ref_bits(py, class_bits);
        });
    }

    #[test]
    fn python_context_publication_preserves_pending_unwind() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            frame_stack_push(py, 0);
            let none = MoltObject::none().bits();
            let _ = crate::raise_exception::<u64>(py, "ValueError", "frame unwind");
            let before = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::exception_pending(py));
            let result = super::molt_frame_context_set(none, MoltObject::from_int(1).bits(), none);
            assert!(crate::obj_from_bits(result).is_none());
            assert!(crate::exception_pending(py));
            let after = crate::builtins::exceptions::molt_exception_last_pending();
            assert_eq!(before, after);
            crate::molt_exception_clear();
            frame_stack_pop(py);
            dec_ref_bits(py, before);
            dec_ref_bits(py, after);
            dec_ref_bits(py, result);
        });
    }

    #[test]
    fn frame_stack_push_borrowed_balances_refcount_on_pop() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let (code_ptr, code_bits) = alloc_test_code(_py);
            inc_ref_bits(_py, code_bits);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);

            frame_stack_push_owned(_py, code_bits, 0, 0);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);
            frame_stack_pop(_py);
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);

            dec_ref_bits(_py, code_bits);
        });
    }

    #[test]
    fn frame_builtins_is_captured_once_not_reread_from_globals() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let globals_ptr = alloc_dict_with_pairs(_py, &[]);
            let first_ptr = alloc_dict_with_pairs(_py, &[]);
            let second_ptr = alloc_dict_with_pairs(_py, &[]);
            let key_ptr = alloc_string(_py, b"__builtins__");
            let globals_bits = MoltObject::from_ptr(globals_ptr).bits();
            let first_bits = MoltObject::from_ptr(first_ptr).bits();
            let second_bits = MoltObject::from_ptr(second_ptr).bits();
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            unsafe { dict_set_in_place(_py, globals_ptr, key_bits, first_bits) };
            inc_ref_bits(_py, globals_bits);
            inc_ref_bits(_py, first_bits);
            frame_stack_push_owned(_py, 0, globals_bits, first_bits);
            assert_eq!(frame_stack_active_builtins_bits(), first_bits);

            unsafe { dict_set_in_place(_py, globals_ptr, key_bits, second_bits) };
            assert_eq!(
                frame_stack_active_builtins_bits(),
                first_bits,
                "PyEval_GetBuiltins follows stored f_builtins, not a later globals mutation"
            );
            assert_eq!(
                super::frame_effective_builtins_bits(_py, globals_bits),
                second_bits,
                "a new function captures the current explicit globals entry"
            );
            let unconfigured = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[])).bits();
            assert_eq!(
                super::frame_effective_builtins_bits(_py, unconfigured),
                first_bits,
                "absent __builtins__ inherits the active captured builtins"
            );
            dec_ref_bits(_py, unconfigured);

            frame_stack_pop(_py);
            dec_ref_bits(_py, globals_bits);
            dec_ref_bits(_py, first_bits);
            dec_ref_bits(_py, second_bits);
            dec_ref_bits(_py, key_bits);
        });
    }
}
