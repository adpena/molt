use crate::BuiltinExc;
use crate::PyToken;
use crate::builtins::exceptions::raise_exception;
use crate::builtins::numbers::{index_bigint_from_obj, index_i64_integral_bits};
use crate::runtime_flags::{self, RuntimeFlag};
use crate::{
    MoltHeader, TRACEBACK_BUILD_COUNT, TRACEBACK_BUILD_FRAMES, TYPE_ID_CODE, TYPE_ID_DICT,
    TYPE_ID_EXCEPTION, TYPE_ID_FRAME_BINDINGS, TYPE_ID_MODULE, TYPE_ID_TRACEBACK_PAYLOAD,
    TYPE_ID_TYPE, alloc_dict_with_pairs, alloc_instance_for_class, alloc_object, builtin_classes,
    code_filename_bits, code_firstlineno, code_name_bits, dec_ref_bits, dict_get_in_place,
    exception_pending, inc_ref_bits, instance_set_dict_bits, module_dict_bits, obj_from_bits,
    object_mark_has_ptrs, object_type_id, profile_enabled, string_obj_to_owned, to_i64,
};
use locations::LocationId;
use molt_obj_model::MoltObject;
use num_traits::ToPrimitive;
use std::sync::atomic::Ordering as AtomicOrdering;

mod activation;
#[cfg(test)]
mod activation_tests;
mod bindings;
mod locals_proxy;
pub(crate) mod locations;
mod namespace;
#[cfg(test)]
mod namespace_tests;
pub(crate) mod records;
pub(crate) use activation::{
    ActivationFrameScope, activation_detach_frame_bindings, activation_exit_bindings,
    activation_locals_bits, empty_locals_bits,
};
#[cfg(test)]
pub(crate) use activation::{
    molt_frame_cell_publish_attached_body, molt_frame_locals_begin_attached_body,
};
pub(crate) use bindings::{
    FramePlan, FrameRelease, frame_argument_zero, frame_bindings_detach, frame_bindings_visit,
};
pub(crate) use locals_proxy::{
    frame_locals_proxy_detach, frame_locals_proxy_visit, frame_object_detach, frame_object_visit,
    molt_frame_clear, molt_frame_f_lasti_get, molt_frame_f_lineno_get, molt_frame_f_locals_get,
    molt_frame_locals_proxy_contains, molt_frame_locals_proxy_copy,
    molt_frame_locals_proxy_delitem, molt_frame_locals_proxy_eq, molt_frame_locals_proxy_get,
    molt_frame_locals_proxy_getitem, molt_frame_locals_proxy_items, molt_frame_locals_proxy_iter,
    molt_frame_locals_proxy_keys, molt_frame_locals_proxy_len, molt_frame_locals_proxy_pop,
    molt_frame_locals_proxy_repr, molt_frame_locals_proxy_setdefault,
    molt_frame_locals_proxy_setitem, molt_frame_locals_proxy_update,
    molt_frame_locals_proxy_values,
};
pub(crate) use namespace::{
    CodeNamespace, CompiledCodeSlot, FrameInvocationGuard, acquire_pending_invocation_context,
    compiled_slot_for_code, globals_namespace_storage_bits, globals_namespace_storage_ptr,
    take_invocation_namespace,
};
pub(crate) use records::{clear_frame_home_tls_storage, touch_frame_home_tls_lifetime};

/// The executing Python argument-zero slot, independent of ABI-only parameters.
#[derive(Clone, Copy, Default)]
pub(crate) enum PythonArgumentZero {
    #[default]
    NoArgument,
    Value(u64),
    Cell(u64),
    /// The first code slot of the executing synchronous frame's homes, read
    /// when `super()` asks ([`frame_argument_zero`]). The home is the only
    /// owner: rebinding, `del` and proxy writes need no republication.
    Home,
}

impl PythonArgumentZero {
    fn retained_bits(self) -> Option<u64> {
        match self {
            Self::NoArgument | Self::Home => None,
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
    let context = records::top()
        .map(records::FrameRecord::python_context)
        .unwrap_or_default();
    context.retain(py);
    PythonFrameContextSnapshot { py, context }
}

pub(crate) fn frame_stack_set_python_context(
    py: &PyToken<'_>,
    context: PythonFrameContext,
) -> bool {
    // Retain before publication. A replaced value can run finalizers on release.
    context.retain(py);
    // Only a frame the runtime linked carries a published context: a
    // synchronous frame's is read from its code and homes.
    let previous = records::top().and_then(|record| {
        record.update_owned(|entry| std::mem::replace(&mut entry.python_context, context))
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

pub(crate) fn molt_frame_context_set_attached_body(
    _py: &crate::PyToken<'_>,
    argument_zero_bits: u64,
    argument_kind_bits: u64,
    class_cell_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let argument_zero = match to_i64(obj_from_bits(argument_kind_bits)) {
            Some(0) => PythonArgumentZero::NoArgument,
            Some(1) => PythonArgumentZero::Value(argument_zero_bits),
            Some(2) if frame_context_cell_is_valid(argument_zero_bits) => {
                PythonArgumentZero::Cell(argument_zero_bits)
            }
            Some(3) if obj_from_bits(argument_zero_bits).is_none() => PythonArgumentZero::Home,
            _ => {
                return raise_exception::<_>(
                    py,
                    BuiltinExc::SystemError,
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
                BuiltinExc::SystemError,
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
                BuiltinExc::SystemError,
                "executing-frame context published without a Python frame",
            );
        }
        MoltObject::none().bits()
    })
}

#[derive(Clone, Copy)]
pub(crate) struct FrameEntry {
    pub(crate) code_bits: u64,
    /// A traceback's frozen site. Live entries use their arena location word.
    pub(crate) location_snapshot: Option<LocationId>,
    /// The namespace mapping of a module or class body (their distinct
    /// publication entries): its
    /// `f_locals` and `locals()`. Owned by the entry. An optimized frame's
    /// bindings are `bindings`, never a dict here.
    pub(crate) locals_bits: u64,
    /// Owned stateful payload; zero for ordinary frames and frozen snapshots.
    pub(crate) activation_bits: u64,
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
    /// An optimized synchronous activation's homes, with the payload its frame
    /// objects share (owned by the entry); a snapshot copy's or a traceback
    /// frame's captured payload (owned by the copy). A stateful activation's
    /// payload belongs to its task.
    pub(crate) bindings: bindings::FrameBindings,
}

const TRACEBACK_PAYLOAD_CODE_OFFSET: usize = 0;
const TRACEBACK_PAYLOAD_LINE_OFFSET: usize = std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_COL_OFFSET: usize = 2 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_END_COL_OFFSET: usize = 3 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_NEXT_OFFSET: usize = 4 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_GLOBALS_OFFSET: usize = 5 * std::mem::size_of::<u64>();
/// The frame's binding source, owned: an optimized frame's `FRAME_BINDINGS`
/// payload, which the exiting frame's bindings move into, or a module body's
/// namespace mapping (module or class).
const TRACEBACK_PAYLOAD_SOURCE_OFFSET: usize = 6 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_BUILTINS_OFFSET: usize = 7 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_LOCATION_OFFSET: usize = 8 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_LOCATION_OWNER_OFFSET: usize = 9 * std::mem::size_of::<u64>();
const TRACEBACK_PAYLOAD_SIZE: usize =
    std::mem::size_of::<MoltHeader>() + 10 * std::mem::size_of::<u64>();

pub(crate) const TRACEBACK_PAYLOAD_OWNED_SLOTS: [usize; 6] = [0, 4, 5, 6, 7, 9];

pub(crate) unsafe fn traceback_payload_code_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_CODE_OFFSET) as *const u64) }
}

/// A line or column a raise has not decoded. A raise records the code and the
/// location id; the position is decoded only when an observer reads it, as
/// CPython computes `tb_lineno` from `tb_lasti`. Decoding the varint location
/// table on every raise was 24% of `bench_exception_heavy` (2026-11-14).
const TRACEBACK_PAYLOAD_UNRESOLVED: i64 = i64::MIN;

/// The recorded position as (line, column, end column): the payload's own
/// values when a writer stored them, else decoded from its code and location.
/// An absent location is the code's first line; a location the table cannot
/// decode has line 0 (as `FrameEntry::source_line`) and no columns.
unsafe fn traceback_payload_position(ptr: *mut u8) -> (i64, i64, i64) {
    unsafe {
        let stored = (
            *(ptr.add(TRACEBACK_PAYLOAD_LINE_OFFSET) as *const i64),
            *(ptr.add(TRACEBACK_PAYLOAD_COL_OFFSET) as *const i64),
            *(ptr.add(TRACEBACK_PAYLOAD_END_COL_OFFSET) as *const i64),
        );
        if stored.0 != TRACEBACK_PAYLOAD_UNRESOLVED {
            return stored;
        }
        let Some(code) = obj_from_bits(traceback_payload_code_bits(ptr)).as_ptr() else {
            return (0, -1, -1);
        };
        if object_type_id(code) != TYPE_ID_CODE {
            return (0, -1, -1);
        }
        let location =
            LocationId::from_word(*(ptr.add(TRACEBACK_PAYLOAD_LOCATION_OFFSET) as *const u64));
        if location == LocationId::ABSENT {
            return (code_firstlineno(code), -1, -1);
        }
        locations::code_location(code, location).map_or((0, -1, -1), |site| {
            (site.line, site.column, site.end_column)
        })
    }
}

pub(crate) unsafe fn traceback_payload_line(ptr: *mut u8) -> i64 {
    unsafe { traceback_payload_position(ptr).0 }
}

pub(crate) unsafe fn traceback_payload_col(ptr: *mut u8) -> i64 {
    unsafe { traceback_payload_position(ptr).1 }
}

pub(crate) unsafe fn traceback_payload_end_col(ptr: *mut u8) -> i64 {
    unsafe { traceback_payload_position(ptr).2 }
}

pub(crate) unsafe fn traceback_payload_next_bits(ptr: *mut u8) -> u64 {
    unsafe { *(ptr.add(TRACEBACK_PAYLOAD_NEXT_OFFSET) as *const u64) }
}

/// Whether a frame entry records a code object, the frames a traceback keeps.
fn entry_has_code(entry: FrameEntry) -> bool {
    obj_from_bits(entry.code_bits)
        .as_ptr()
        .is_some_and(|code| unsafe { object_type_id(code) } == TYPE_ID_CODE)
}

/// The frame a traceback payload recorded, its binding source split back into
/// a bindings payload or a namespace mapping.
unsafe fn traceback_payload_frame_entry(ptr: *mut u8) -> FrameEntry {
    unsafe {
        let source = *(ptr.add(TRACEBACK_PAYLOAD_SOURCE_OFFSET) as *const u64);
        let payload = obj_from_bits(source)
            .as_ptr()
            .is_some_and(|source| object_type_id(source) == TYPE_ID_FRAME_BINDINGS);
        FrameEntry {
            code_bits: traceback_payload_code_bits(ptr),
            location_snapshot: Some(LocationId::from_word(
                *(ptr.add(TRACEBACK_PAYLOAD_LOCATION_OFFSET) as *const u64),
            )),
            globals_bits: *(ptr.add(TRACEBACK_PAYLOAD_GLOBALS_OFFSET) as *const u64),
            locals_bits: if payload { 0 } else { source },
            builtins_bits: *(ptr.add(TRACEBACK_PAYLOAD_BUILTINS_OFFSET) as *const u64),
            activation_bits: 0,
            python_context: PythonFrameContext::default(),
            bindings: bindings::FrameBindings::captured(
                *(ptr.add(TRACEBACK_PAYLOAD_LOCATION_OWNER_OFFSET) as *const u64),
            ),
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
    if obj_from_bits(dict_bits)
        .as_ptr()
        .is_some_and(|dict_ptr| unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT)
    {
        dict_bits
    } else {
        bits
    }
}

pub(crate) fn frame_effective_builtins_bits(
    _py: &PyToken<'_>,
    globals_bits: u64,
) -> Result<u64, ()> {
    let globals_ptr = globals_namespace_storage_ptr(_py, globals_bits)?;
    if let Some(globals_ptr) = globals_ptr {
        let key_bits = crate::NameId::DunderBuiltins.bits(_py);
        if key_bits == 0 {
            return Err(());
        }
        if let Some(value_bits) = unsafe { dict_get_in_place(_py, globals_ptr, key_bits) }? {
            return Ok(normalize_builtins_value(value_bits));
        }
    }
    if let Some(bits) = frame_stack_active_builtins() {
        return Ok(bits);
    }
    if let Some(bits) = crate::builtins::module_table::interpreter_builtins_dictionary(_py) {
        return Ok(bits);
    }
    // A compiled application admits its builtins body in the module registry.
    // Materialize that canonical namespace before capture, never substitute a
    // later cache value into an already executing frame. The builtins body
    // publishes its dictionary before entering its own frame, closing cycles.
    if crate::builtins::module_table::module_execution_target_has_body("builtins") == Some(true)
        && !crate::builtins::module_table::module_initialization_awaits_publication(_py, "builtins")
    {
        let id = crate::builtins::module_table::module_id_of("builtins")
            .expect("admitted builtins registry row");
        let module = crate::builtins::module_table::module_ensure(_py, id);
        let failed = exception_pending(_py);
        // Only the interpreter role guarantees a default borrowed lifetime.
        // A public replacement can bypass initialization, and publication
        // callbacks may remove the module's last dictionary entry.
        let dictionary = crate::builtins::module_table::interpreter_builtins_dictionary(_py);
        dec_ref_bits(_py, module);
        if failed || exception_pending(_py) {
            return Err(());
        }
        return dictionary.ok_or_else(|| {
            crate::raise_exception::<()>(
                _py,
                BuiltinExc::SystemError,
                "canonical builtins initializer did not publish the interpreter dictionary",
            );
        });
    }
    Ok(0)
}

/// The executing frame's facts, borrowed from its record.
pub(crate) fn frame_stack_top_entry() -> Option<FrameEntry> {
    records::top().map(records::FrameRecord::entry)
}

/// Transfer an explicitly owned code/namespace triple to the sole Python
/// frame: link its record as the executing frame, with no binding homes (a
/// runtime namespace context). A thread whose frame arena cannot hold one
/// more record cannot run Python at all, as a thread that cannot grow
/// CPython's data stack cannot.
pub(crate) fn frame_stack_push_owned(
    _py: &PyToken<'_>,
    code_bits: u64,
    globals_bits: u64,
    builtins_bits: u64,
    activation_bits: u64,
) {
    crate::gil_assert();
    records::push(0, |base| FrameEntry {
        code_bits,
        location_snapshot: None,
        locals_bits: 0,
        globals_bits,
        builtins_bits,
        activation_bits,
        python_context: PythonFrameContext::default(),
        bindings: bindings::FrameBindings::over(FramePlan::default(), base),
    })
    .expect("frame arena cannot hold a frame record");
}

/// Link the frame of an activation with the homes its `plan` calls for, in
/// one arena transaction; the edges transfer as in
/// [`frame_stack_push_owned`]. `false`: the arena could not hold the homes.
/// The frame is linked without them, so a `MemoryError` traceback keeps it,
/// and its exit still pops it.
pub(crate) fn frame_stack_enter(
    py: &PyToken<'_>,
    code_bits: u64,
    globals_bits: u64,
    builtins_bits: u64,
    activation_bits: u64,
    plan: FramePlan,
) -> bool {
    crate::gil_assert();
    let entry = |bindings| FrameEntry {
        code_bits,
        location_snapshot: None,
        locals_bits: 0,
        globals_bits,
        builtins_bits,
        activation_bits,
        python_context: PythonFrameContext::default(),
        bindings,
    };
    let linked = records::push(plan.homes as usize, |base| {
        let mut bindings = bindings::FrameBindings::over(plan, base);
        if let Some(task) = obj_from_bits(activation_bits).as_ptr() {
            bindings.set_location(LocationId::from_word(
                crate::object::aux_header::object_frame_location(task),
            ));
            let payload = crate::object::aux_header::object_frame_bindings_bits(task);
            if let Some(ptr) = obj_from_bits(payload).as_ptr() {
                inc_ref_bits(py, payload);
                bindings.payload_bits = payload;
                bindings::lend_location(ptr, bindings.location_address());
            }
        }
        entry(bindings)
    });
    if linked.is_some() {
        return true;
    }
    frame_stack_push_owned(py, code_bits, globals_bits, builtins_bits, activation_bits);
    false
}

/// A synchronous Python body implemented by a runtime plan. It has the same
/// homes, live locals, namespace and exit custody as a compiled Python entry.
/// No code-slot registry or secondary locals dictionary is introduced.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeBodyState {
    Binding,
    Executing,
}

pub(crate) struct PythonFunctionFrame<'a, 'py> {
    py: &'a PyToken<'py>,
    homes: *mut u64,
    body_state: RuntimeBodyState,
    code: u64,
    record: Option<records::FrameRecord>,
}

impl<'a, 'py> PythonFunctionFrame<'a, 'py> {
    /// On success adopts each argument. On failure adopts none. `cells` are
    /// borrowed lexical cells; their frame owners are retained separately.
    pub(crate) fn adopt(
        py: &'a PyToken<'py>,
        context: [u64; 3],
        plan: FramePlan,
        arguments: &[u64],
        cells: &[u64],
    ) -> Result<Self, ()> {
        use molt_codegen_abi::{FRAME_HOME_CELL, FRAME_HOME_PLAIN, FRAME_HOME_WORDS};
        let [globals, builtins, code] = context;
        if !plan.optimized || plan.homes as usize != arguments.len() + cells.len() {
            raise_exception::<()>(
                py,
                BuiltinExc::SystemError,
                "runtime body disagrees with Python frame layout",
            );
            return Err(());
        }
        for bits in context {
            inc_ref_bits(py, bits);
        }
        let entered = frame_stack_enter(py, code, globals, builtins, 0, plan);
        let record = records::top();
        let mut frame = Self {
            py,
            homes: std::ptr::null_mut(),
            body_state: RuntimeBodyState::Binding,
            code,
            record,
        };
        if !entered {
            crate::record_memory_error_without_allocation(py);
            return Err(());
        }
        let record = record.expect("an entered frame is linked");
        frame.homes = record.base() as *mut u64;
        // No fallible work or callbacks remain before the argument transfer.
        for (slot, &value) in arguments.iter().enumerate() {
            unsafe {
                frame
                    .homes
                    .add(slot * FRAME_HOME_WORDS)
                    .write(FRAME_HOME_PLAIN as u64);
                frame.homes.add(slot * FRAME_HOME_WORDS + 1).write(value);
            }
        }
        for (index, &cell) in cells.iter().enumerate() {
            inc_ref_bits(py, cell);
            let slot = arguments.len() + index;
            unsafe {
                frame
                    .homes
                    .add(slot * FRAME_HOME_WORDS)
                    .write(FRAME_HOME_CELL as u64);
                frame.homes.add(slot * FRAME_HOME_WORDS + 1).write(cell);
            }
        }
        record.bindings().set_location(LocationId::from_word(0));
        frame.body_state = RuntimeBodyState::Executing;
        Ok(frame)
    }

    /// A fresh owned read observes PEP 667 writes made by an earlier callback.
    pub(crate) fn argument(&self, slot: usize) -> Result<u64, ()> {
        let home = unsafe { self.homes.add(slot * molt_codegen_abi::FRAME_HOME_WORDS) };
        let value = bindings::molt_frame_home_load_attached_body(self.py, home as usize as u64);
        if exception_pending(self.py) {
            return Err(());
        }
        if crate::is_missing_bits(self.py, value) {
            let code = obj_from_bits(self.code).as_ptr().unwrap();
            let names = unsafe { crate::object::layout::code_varnames_bits(code) };
            let name = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(names).as_ptr().unwrap(),
                    |items| items[slot],
                )
            }
            .unwrap();
            let name = string_obj_to_owned(obj_from_bits(name)).unwrap();
            raise_exception::<()>(
                self.py,
                BuiltinExc::UnboundLocalError,
                &format!(
                    "cannot access local variable '{name}' where it is not associated with a value"
                ),
            );
            return Err(());
        }
        inc_ref_bits(self.py, value);
        Ok(value)
    }
}

impl Drop for PythonFunctionFrame<'_, '_> {
    fn drop(&mut self) {
        assert!(
            records::top() == self.record,
            "runtime Python frame custody is not LIFO"
        );
        if self.body_state == RuntimeBodyState::Executing && exception_pending(self.py) {
            crate::builtins::exceptions::molt_exception_frame_origin_attached_body(
                self.py,
                self.homes as u64,
            );
        }
        frame_stack_pop(self.py);
    }
}

/// What a compiled function keeps across its body: the base its frame entry
/// returned, which its exit passes back. Runtime tests that stand in for
/// compiled code keep those bases here.
#[cfg(test)]
pub(crate) mod compiled_entry {
    use crate::{MoltObject, PyToken};
    use std::cell::RefCell;

    thread_local! {
        static BASES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    }

    /// Enter the frame of code slot `slot`, as a compiled entry does.
    pub(crate) fn enter(py: &PyToken<'_>, slot: u64) -> u64 {
        let base = crate::molt_frame_record_enter_attached_body(py, slot);
        BASES.with(|bases| bases.borrow_mut().push(base));
        MoltObject::none().bits()
    }

    /// Leave the frame of the matching [`enter`], if it linked one.
    pub(crate) fn exit(py: &PyToken<'_>) -> u64 {
        let base = BASES.with(|bases| bases.borrow_mut().pop()).unwrap_or(0);
        if base != 0 {
            crate::molt_frame_record_exit_attached_body(py, base);
        }
        MoltObject::none().bits()
    }

    /// The executing frame's homes base, as compiled code holds it.
    pub(crate) fn homes() -> u64 {
        super::records::top().map_or(0, |record| record.base() as u64)
    }
}

#[cfg(test)]
pub(crate) fn frame_stack_push(_py: &PyToken<'_>, code_bits: u64) {
    crate::gil_assert();
    if code_bits != 0 {
        inc_ref_bits(_py, code_bits);
    }
    frame_stack_push_owned(_py, code_bits, 0, 0, 0);
}

pub(crate) fn frame_stack_active_activation_bits() -> u64 {
    records::top().map_or(0, records::FrameRecord::activation_bits)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn frame_stack_active_code_bits() -> u64 {
    frame_stack_top_entry().map_or(0, |entry| entry.code_bits)
}

pub(crate) fn frame_stack_active_globals_bits() -> u64 {
    records::top().map_or(0, |record| record.namespaces().0)
}

/// A present frame owns its captured namespace, including unavailable (zero)
/// bootstrap state. Only absence of a frame permits interpreter defaults.
pub(crate) fn frame_stack_active_builtins() -> Option<u64> {
    records::top().map(|record| record.namespaces().1)
}

pub(crate) fn frame_stack_active_builtins_bits() -> u64 {
    frame_stack_active_builtins().unwrap_or(0)
}

/// The active frame's globals and builtins, the pair `molt_module_get_global`
/// consults, read in one frame-stack access. `None` without a frame.
pub(crate) fn frame_stack_active_namespaces() -> Option<(u64, u64)> {
    records::top().map(records::FrameRecord::namespaces)
}

impl FrameEntry {
    pub(crate) fn is_class_body(self) -> bool {
        Self::code_is_class_body(self.code_bits)
    }

    fn code_is_class_body(code_bits: u64) -> bool {
        obj_from_bits(code_bits).as_ptr().is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_CODE
                && crate::object::layout::code_scope_kind(ptr)
                    == crate::object::layout::CodeScopeKind::ClassBody
        })
    }
    pub(crate) fn source_position(self) -> Option<locations::CodeLocation> {
        let code = obj_from_bits(self.code_bits).as_ptr()?;
        if unsafe { object_type_id(code) } != TYPE_ID_CODE {
            return None;
        }
        let id = self
            .location_snapshot
            .unwrap_or_else(|| self.bindings.location());
        unsafe { locations::code_location(code, id) }
    }

    pub(crate) fn source_line(self) -> i64 {
        unsafe { frame_line_from_entry(self) }.unwrap_or(0)
    }
    fn retain(self, py: &PyToken<'_>) {
        for bits in [
            self.code_bits,
            self.locals_bits,
            self.globals_bits,
            self.builtins_bits,
            self.activation_bits,
            self.bindings.payload_bits,
        ] {
            inc_ref_bits(py, bits);
        }
        self.python_context.retain(py);
    }

    /// One edge-release authority for frame exits and snapshot copies.
    pub(crate) fn release(self, py: &PyToken<'_>) {
        for bits in [
            self.code_bits,
            self.locals_bits,
            self.globals_bits,
            self.builtins_bits,
            self.activation_bits,
            self.bindings.payload_bits,
        ] {
            if bits != 0 && !obj_from_bits(bits).is_none() {
                dec_ref_bits(py, bits);
            }
        }
        self.python_context.release(py);
    }

    /// The exit of an entry no longer on the stack, after a return, an unwind
    /// or runtime teardown: every finalizer its releases run sees the caller as
    /// the executing frame. The bindings leave the homes first (a frame object
    /// that shares them takes them over, as CPython's `take_ownership`), the
    /// homes go back once those releases finish, then the entry's own edges
    /// are released.
    pub(crate) fn exit(self, py: &PyToken<'_>) {
        if let Some(task) = obj_from_bits(self.activation_bits).as_ptr() {
            crate::object::aux_header::object_set_frame_location(
                task,
                self.bindings.location().word(),
            );
            if let Some(ptr) = obj_from_bits(self.bindings.payload_bits).as_ptr() {
                bindings::freeze_location(ptr);
            }
        } else {
            bindings::frame_bindings_exit(py, &self.bindings);
        }
        self.release(py);
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

    /// Snapshot `records` (outermost first), each optimized frame's bindings
    /// payload attached first, so every frame object or traceback built from
    /// the snapshot shares its activation's bindings instead of copying them.
    /// Attaching allocates and can run finalizers; those run above the
    /// selected records and cannot pop one. `Err`: a payload could not be
    /// attached and nothing was captured.
    fn capture(
        py: &'a PyToken<'py>,
        records: &[records::FrameRecord],
    ) -> Result<Self, bindings::ObserveError> {
        let mut payloads = Vec::with_capacity(records.len());
        for &record in records {
            payloads.push(bindings::frame_record_observe(py, record)?);
        }
        let entries = records
            .iter()
            .zip(payloads)
            // A copy never owns homes: it shares the payload it retains.
            .map(|(record, payload)| record.entry_with(bindings::FrameBindings::captured(payload)))
            .collect();
        Ok(Self::retained(py, entries))
    }
}

impl Drop for FrameStackSnapshot<'_, '_> {
    fn drop(&mut self) {
        for entry in self.entries.drain(..) {
            entry.release(self.py);
        }
    }
}

/// Unlink the executing frame, then run its exit ([`FrameEntry::exit`]).
pub(crate) fn frame_stack_pop(_py: &PyToken<'_>) {
    crate::gil_assert();
    if let Some(unlinked) = records::unlink() {
        // The record's storage stays reserved until the exit's releases end.
        unlinked.exit(_py);
    }
}

/// Return (filename, lineno, function_name, col_offset, end_col_offset) from
/// the top frame, if available.  col_offset/end_col_offset are -1 when unknown.
pub(crate) fn frame_stack_top_info(_py: &PyToken<'_>) -> Option<(String, i64, String, i64, i64)> {
    {
        let entry = frame_stack_top_entry()?;
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
                entry.source_line(),
                name,
                entry.source_position().map_or(-1, |site| site.column),
                entry.source_position().map_or(-1, |site| site.end_column),
            ))
        }
    }
}

/// One executing Python frame as a location query reads it: the code object,
/// the globals namespace and the current line (`f_lineno`).
#[derive(Clone, Copy)]
pub(crate) struct PythonFrameSite {
    pub(crate) code_bits: u64,
    pub(crate) globals_bits: u64,
    pub(crate) line: i64,
}

/// Choose one executing Python frame and retain its code and globals for the
/// caller, who releases both. `choose` sees the Python-visible frames newest
/// first; runtime namespace contexts without a code object are not Python
/// calls and are skipped, as [`frame_at_depth`] skips them. No frame object is
/// materialized. `choose` walks the live record chain: it may read objects,
/// but it must not allocate Python objects or run Python code.
pub(crate) fn python_frame_site(
    py: &PyToken<'_>,
    choose: impl FnOnce(&mut dyn Iterator<Item = PythonFrameSite>) -> Option<PythonFrameSite>,
) -> Option<PythonFrameSite> {
    crate::gil_assert();
    let mut frames = records::innermost_first().filter_map(|record| {
        let entry = record.entry();
        let line = unsafe { frame_line_from_entry(entry) }?;
        Some(PythonFrameSite {
            code_bits: entry.code_bits,
            globals_bits: entry.globals_bits,
            line,
        })
    });
    let site = choose(&mut frames)?;
    inc_ref_bits(py, site.code_bits);
    inc_ref_bits(py, site.globals_bits);
    Some(site)
}

pub(crate) fn frame_stack_set_locals_dict(_py: &PyToken<'_>, dict_bits: u64) {
    crate::gil_assert();
    // Only a module or class body, which the runtime links, binds a namespace.
    let prev = records::top().and_then(|record| {
        record.update_owned(|entry| {
            // Replace and manage refcounts. 0 means "unset".
            let prev = entry.locals_bits;
            entry.locals_bits = 0;
            if dict_bits != 0 && !obj_from_bits(dict_bits).is_none() {
                inc_ref_bits(_py, dict_bits);
                entry.locals_bits = dict_bits;
            }
            prev
        })
    });
    if let Some(prev) = prev
        && prev != 0
        && !obj_from_bits(prev).is_none()
    {
        dec_ref_bits(_py, prev);
    }
}

pub(crate) fn molt_frame_locals_set_attached_body(_py: &crate::PyToken<'_>, dict_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        // This operation publishes an executable module's namespace, not an
        // optimized function's locals. CPython prepares __builtins__ once at
        // execution entry; neither global reads nor FunctionType may fill it.
        let dict_ptr = match globals_namespace_storage_ptr(_py, dict_bits) {
            Ok(Some(ptr)) => ptr,
            Ok(None) => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::SystemError,
                    "module locals is not a dictionary",
                );
            }
            Err(()) => return MoltObject::none().bits(),
        };
        let builtins_bits = frame_stack_active_builtins_bits();
        // CPython initializes its native builtins without executing Python
        // module code. Our source facade must preserve that public dictionary:
        // adding a self key would make a deleted __builtins__ name resolve via
        // builtin fallback in every other module. Only its exact in-flight
        // ModuleTable owner has this bootstrap privilege.
        let builtin_bootstrap = dict_bits == builtins_bits
            && crate::builtins::module_table::module_execution_owns_initializing_globals(
                _py, "builtins", dict_bits,
            );
        if !builtin_bootstrap {
            let key = crate::NameId::DunderBuiltins.bits(_py);
            if key == 0 {
                return MoltObject::none().bits();
            }
            match unsafe { dict_get_in_place(_py, dict_ptr, key) } {
                Ok(Some(_)) => {} // Preserve even an explicit Python None.
                Ok(None) => {
                    if builtins_bits == 0 {
                        return raise_exception::<_>(
                            _py,
                            BuiltinExc::SystemError,
                            "module execution has no captured builtins namespace",
                        );
                    }
                    // No frame-record access spans dictionary equality or
                    // allocation, which may enter Python before publication.
                    inc_ref_bits(_py, builtins_bits);
                    let inserted =
                        unsafe { crate::dict_set_in_place(_py, dict_ptr, key, builtins_bits) };
                    dec_ref_bits(_py, builtins_bits);
                    if !inserted {
                        return MoltObject::none().bits();
                    }
                }
                Err(()) => return MoltObject::none().bits(),
            }
        }
        let debug = runtime_flags::enabled(RuntimeFlag::MOLT_DEBUG_LOCALS);
        if debug {
            let depth = records::depth();
            let (top_locals, top_code) = frame_stack_top_entry()
                .map(|e| (e.locals_bits, e.code_bits))
                .unwrap_or((0, 0));
            eprintln!(
                "molt debug locals frame_locals_set depth={} prev_locals=0x{:016x} code=0x{:016x} new=0x{:016x}",
                depth, top_locals, top_code, dict_bits
            );
        }
        frame_stack_set_locals_dict(_py, dict_bits);
        MoltObject::from_bool(true).bits()
    })
}

/// The class namespace is already prepared. Preserve its identity, including
/// custom mappings; module __builtins__ initialization has no place here.
pub(crate) fn molt_frame_class_locals_set_attached_body(
    _py: &crate::PyToken<'_>,
    mapping_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let class_frame = frame_stack_top_entry()
            .is_some_and(|entry| entry.is_class_body() && !entry.bindings.has_python_bindings());
        if !class_frame || obj_from_bits(mapping_bits).as_ptr().is_none() {
            return raise_exception::<_>(
                py,
                BuiltinExc::SystemError,
                "class namespace publication requires a class frame and mapping",
            );
        }
        frame_stack_set_locals_dict(py, mapping_bits);
        MoltObject::none().bits()
    })
}

pub(crate) fn molt_class_body_builtins_attached_body(
    _py: &crate::PyToken<'_>,
    globals_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        match frame_effective_builtins_bits(py, globals_bits) {
            Ok(bits) => {
                inc_ref_bits(py, bits);
                bits
            }
            Err(()) => MoltObject::none().bits(),
        }
    })
}

pub(crate) fn molt_code_set_class_body_attached_body(
    _py: &crate::PyToken<'_>,
    code_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(ptr) = obj_from_bits(code_bits).as_ptr().filter(|ptr| unsafe {
            object_type_id(*ptr) == TYPE_ID_CODE
                && crate::object::layout::code_frame_slot_id(*ptr).is_none()
                && crate::object::layout::code_scope_kind(*ptr)
                    == crate::object::layout::CodeScopeKind::Function
        }) else {
            return raise_exception::<_>(
                py,
                BuiltinExc::SystemError,
                "class body code must be marked before its compiled slot is published",
            );
        };
        unsafe {
            crate::object::layout::code_set_scope_kind(
                ptr,
                crate::object::layout::CodeScopeKind::ClassBody,
            );
        }
        MoltObject::none().bits()
    })
}

/// Inline class regions retain the enclosing function's homes. Their source
/// sites belong to the class frame, never the enclosing homes' location word.
pub(crate) fn molt_class_frame_set_location_attached_body(
    _py: &crate::PyToken<'_>,
    index_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(index) =
            to_i64(obj_from_bits(index_bits)).and_then(|value| u64::try_from(value).ok())
        else {
            return raise_exception::<_>(
                py,
                BuiltinExc::SystemError,
                "invalid class location index",
            );
        };
        frame_stack_top_entry()
            .expect("executing class frame")
            .bindings
            .set_location(LocationId::from_word(index));
        MoltObject::none().bits()
    })
}

pub(super) const UNMATERIALIZED_CLASS_LOCALS: &str =
    "class namespace reflection requires a materialized class body mapping (spec 0216)";

#[derive(Clone, Copy)]
struct FrameField {
    bits: u64,
    owned: bool,
}

/// What holds a frame's bindings for its frame objects and tracebacks, borrowed
/// from the entry: an optimized frame's payload, else a module body's
/// namespace mapping, else zero.
fn frame_binding_source(entry: &FrameEntry) -> u64 {
    if entry.bindings.payload_bits != 0 && entry.bindings.has_python_bindings() {
        entry.bindings.payload_bits
    } else if entry.locals_bits == 0 && entry.is_class_body() {
        // A distinct typed source state for an elided class mapping. Frozen
        // traceback frames keep this state instead of borrowing another scope.
        MoltObject::none().bits()
    } else {
        entry.locals_bits
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
        let location = entry
            .location_snapshot
            .unwrap_or_else(|| entry.bindings.location());
        if location == LocationId::ABSENT {
            return Some(code_firstlineno(code_ptr));
        }
        locations::code_location(code_ptr, location).map(|location| location.line)
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
        crate::object::layout::code_is_module(code_ptr)
    }
}

fn frame_globals_field(_py: &PyToken<'_>, entry: FrameEntry) -> Option<FrameField> {
    let globals = match globals_namespace_storage_bits(_py, entry.globals_bits) {
        Ok(storage) => storage,
        Err(()) => return None,
    };
    if globals.is_some() {
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
        BuiltinExc::SystemError,
        "Python frame has no bound globals namespace",
    );
    None
}

unsafe fn alloc_frame_obj(_py: &PyToken<'_>, entry: FrameEntry, back_bits: u64) -> Option<u64> {
    unsafe {
        let name = |name: crate::NameId| {
            let bits = name.bits(_py);
            (bits != 0).then_some(bits)
        };
        // Stop at the first failed intern before another allocation can replace
        // its exception or an unpublished frame acquires cleanup obligations.
        let f_code_bits = name(crate::NameId::FCode)?;
        let f_back_bits = name(crate::NameId::FBack)?;
        let f_globals_bits = name(crate::NameId::FGlobals)?;
        let f_builtins_bits = name(crate::NameId::FBuiltins)?;
        let builtins = builtin_classes(_py);
        let class_obj = obj_from_bits(builtins.frame);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        // `f_locals` and `clear()` read the typed source this frame records.
        if !crate::builtins::types::frame_class_ready(_py) {
            return None;
        }
        let frame_bits = alloc_instance_for_class(_py, class_ptr);
        let frame_ptr = obj_from_bits(frame_bits).as_ptr()?;
        if !locals_proxy::frame_object_set_location_owner(
            _py,
            frame_ptr,
            entry.bindings.payload_bits,
        ) {
            dec_ref_bits(_py, frame_bits);
            return None;
        }
        let Some(globals) = frame_globals_field(_py, entry) else {
            dec_ref_bits(_py, frame_bits);
            return None;
        };
        // `f_locals` reads this source afresh at every access (the frame
        // class's descriptor): an optimized frame's bindings payload, or a
        // module body's namespace. No locals dict is copied here.
        let source = match frame_binding_source(&entry) {
            0 if code_is_module(entry.code_bits) => globals.bits,
            source => source,
        };
        if source != 0 && !locals_proxy::frame_object_set_source(_py, frame_ptr, source) {
            dec_ref_bits(_py, frame_bits);
            return None;
        }
        let dict_ptr = alloc_dict_with_pairs(
            _py,
            &[
                f_code_bits,
                entry.code_bits,
                f_back_bits,
                back_bits,
                f_globals_bits,
                globals.bits,
                f_builtins_bits,
                entry.builtins_bits,
            ],
        );
        if globals.owned {
            dec_ref_bits(_py, globals.bits);
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
/// stacks and tracebacks. Runtime-native tasks have no Python frame to invent,
/// and a finished activation has none left. The frame object shares the
/// activation's bindings payload, which reads the task payload while the
/// activation lives and takes its bindings over when it finishes.
pub(crate) unsafe fn suspended_frame_bits(py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let [globals_bits, builtins_bits, code_bits] =
            crate::object::aux_header::object_frame_context_bits(ptr);
        if code_bits == 0 {
            return MoltObject::none().bits();
        }
        let payload = match bindings::frame_bindings_attach_activation(py, ptr, false) {
            Ok(0) => return MoltObject::none().bits(),
            Ok(payload) => payload,
            Err(error) => {
                error.raise(py);
                return MoltObject::none().bits();
            }
        };
        let entry = FrameEntry {
            code_bits,
            globals_bits,
            builtins_bits,
            locals_bits: 0,
            location_snapshot: None,
            activation_bits: 0,
            python_context: PythonFrameContext::default(),
            bindings: bindings::FrameBindings::captured(payload),
        };
        if frame_line_from_entry(entry).is_none() {
            return raise_exception::<u64>(
                py,
                BuiltinExc::SystemError,
                "suspended frame has invalid code",
            );
        }
        alloc_frame_obj(py, entry, MoltObject::none().bits())
            .unwrap_or_else(|| MoltObject::none().bits())
    }
}

unsafe fn alloc_traceback_obj(
    _py: &PyToken<'_>,
    frame_bits: u64,
    line: i64,
    next_bits: u64,
    lasti: i64,
) -> Option<u64> {
    unsafe {
        let builtins = builtin_classes(_py);
        let class_obj = obj_from_bits(builtins.traceback);
        let class_ptr = class_obj.as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let tb_bits = alloc_instance_for_class(_py, class_ptr);
        let tb_ptr = obj_from_bits(tb_bits).as_ptr()?;
        let tb_frame_bits = crate::NameId::TbFrame.bits(_py);
        let tb_lineno_bits = crate::NameId::TbLineno.bits(_py);
        let tb_next_bits = crate::NameId::TbNext.bits(_py);
        let tb_lasti_bits = crate::NameId::TbLasti.bits(_py);
        let line_bits = MoltObject::from_int(line).bits();
        let lasti_bits = MoltObject::from_int(lasti).bits();
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
        if !entry_has_code(entry) {
            return None;
        }
        let ptr = alloc_object(_py, TRACEBACK_PAYLOAD_SIZE, TYPE_ID_TRACEBACK_PAYLOAD);
        if ptr.is_null() {
            return None;
        }
        *(ptr.add(TRACEBACK_PAYLOAD_CODE_OFFSET) as *mut u64) = entry.code_bits;
        // The position is decoded when read (`traceback_payload_position`).
        for offset in [
            TRACEBACK_PAYLOAD_LINE_OFFSET,
            TRACEBACK_PAYLOAD_COL_OFFSET,
            TRACEBACK_PAYLOAD_END_COL_OFFSET,
        ] {
            *(ptr.add(offset) as *mut i64) = TRACEBACK_PAYLOAD_UNRESOLVED;
        }
        *(ptr.add(TRACEBACK_PAYLOAD_NEXT_OFFSET) as *mut u64) = next_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_GLOBALS_OFFSET) as *mut u64) = entry.globals_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_SOURCE_OFFSET) as *mut u64) = frame_binding_source(&entry);
        *(ptr.add(TRACEBACK_PAYLOAD_BUILTINS_OFFSET) as *mut u64) = entry.builtins_bits;
        *(ptr.add(TRACEBACK_PAYLOAD_LOCATION_OFFSET) as *mut u64) = entry
            .location_snapshot
            .unwrap_or_else(|| entry.bindings.location())
            .word();
        *(ptr.add(TRACEBACK_PAYLOAD_LOCATION_OWNER_OFFSET) as *mut u64) =
            entry.bindings.payload_bits;
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

unsafe fn build_frame_chain(
    _py: &PyToken<'_>,
    entries: &[FrameEntry],
) -> Option<Vec<(u64, i64, LocationId)>> {
    unsafe {
        let mut out: Vec<(u64, i64, LocationId)> = Vec::with_capacity(entries.len());
        let mut back_bits = MoltObject::none().bits();
        for entry in entries {
            let Some(line) = frame_line_from_entry(*entry) else {
                continue;
            };
            let frame_bits = match alloc_frame_obj(_py, *entry, back_bits) {
                Some(bits) => bits,
                None => {
                    for (bits, _, _) in out {
                        dec_ref_bits(_py, bits);
                    }
                    return None;
                }
            };
            back_bits = frame_bits;
            out.push((
                frame_bits,
                line,
                entry
                    .location_snapshot
                    .unwrap_or_else(|| entry.bindings.location()),
            ));
        }
        Some(out)
    }
}

/// Select by Python-visible frames, not by raw runtime stack entries. Runtime
/// namespace contexts with no code object are not additional Python calls.
/// `None` means the requested frame is absent; allocation/callback failures
/// retain their pending exception and return `Err` instead.
pub(crate) fn frame_at_depth(_py: &PyToken<'_>, depth: usize) -> Result<Option<u64>, ()> {
    // Select first, so only the selected frame and its `f_back` chain attach
    // bindings payloads; frames above it stay unobserved.
    let mut chain: Vec<records::FrameRecord> = records::innermost_first()
        .skip_while({
            let mut visible = 0;
            move |record| {
                if unsafe { frame_line_from_entry(record.entry()) }.is_none() {
                    return true;
                }
                visible += 1;
                visible <= depth
            }
        })
        .collect();
    if chain.is_empty() {
        return Ok(None);
    }
    // Outermost first, the selected frame last.
    chain.reverse();
    let snapshot = match FrameStackSnapshot::capture(_py, &chain) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            error.raise(_py);
            return Err(());
        }
    };
    let frames = unsafe { build_frame_chain(_py, &snapshot.entries) };
    let Some(frames) = frames else {
        if !exception_pending(_py) {
            raise_exception::<()>(_py, BuiltinExc::MemoryError, "frame materialization failed");
        }
        return Err(());
    };
    if exception_pending(_py) {
        for (bits, _, _) in frames {
            dec_ref_bits(_py, bits);
        }
        return Err(());
    }
    let Some((selected_bits, _, _)) = frames.last().copied() else {
        raise_exception::<()>(
            _py,
            BuiltinExc::SystemError,
            "selected Python frame was not materialized",
        );
        return Err(());
    };
    inc_ref_bits(_py, selected_bits);
    for (bits, _, _) in frames {
        dec_ref_bits(_py, bits);
    }
    Ok(Some(selected_bits))
}

#[cfg(test)]
pub(crate) fn frame_stack_trace_payload_bits(py: &PyToken<'_>) -> Option<u64> {
    frame_stack_trace_payload_prepend_bits(py, MoltObject::none().bits())
}

/// Capture exactly this activation's Origin event, ahead of the current tail.
pub(crate) fn frame_stack_trace_payload_prepend_bits(
    _py: &PyToken<'_>,
    tail_bits: u64,
) -> Option<u64> {
    // Each recorded frame shares its activation's bindings payload, as a
    // CPython traceback holds the frame object. A payload that cannot be
    // attached (allocation failure, malformed code layout) abandons the whole
    // capture exactly as a failed traceback allocation does: the raise keeps
    // its own exception and any traceback it already owns, and no second error
    // is recorded through this path. Observers of such a frame diagnose it.
    // The executing frame alone: an Origin capture never walks the chain.
    let snapshot = FrameStackSnapshot::capture(_py, records::top().as_slice()).ok()?;
    let active = &snapshot.entries;
    if active.is_empty() {
        return None;
    }
    let mut next_bits = tail_bits;
    inc_ref_bits(_py, next_bits);
    let mut built_any = false;
    for entry in active.iter().rev().copied() {
        if !entry_has_code(entry) {
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
        dec_ref_bits(_py, next_bits);
        None
    }
}

/// Return an owned traceback, or `Some(None)` when the payload has no Python
/// frames. Failure returns `None` with its own error selected; an incoming
/// error is restored only after successful materialization.
pub(crate) fn traceback_payload_to_traceback_bits(
    _py: &PyToken<'_>,
    payload_bits: u64,
) -> Option<u64> {
    let mut payload_entries: Vec<FrameEntry> = Vec::new();
    let mut current_bits = payload_bits;
    while !obj_from_bits(current_bits).is_none() {
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
    }
    if payload_entries.is_empty() {
        return Some(MoltObject::none().bits());
    }
    let mut result = None;
    crate::builtins::exceptions::with_saved_raised_exception(_py, || {
        result = (|| {
            let snapshot = FrameStackSnapshot::retained(_py, payload_entries);
            inc_ref_bits(_py, current_bits);
            unsafe {
                let Some(frames) = build_frame_chain(_py, &snapshot.entries) else {
                    dec_ref_bits(_py, current_bits);
                    return None;
                };
                let mut next_bits = current_bits;
                let mut built_any = false;
                let mut frames_built: u64 = 0;
                for (frame_bits, line, location) in frames.iter().rev().copied() {
                    let Some(tb_bits) =
                        alloc_traceback_obj(_py, frame_bits, line, next_bits, location.lasti()?)
                    else {
                        if !obj_from_bits(next_bits).is_none() {
                            dec_ref_bits(_py, next_bits);
                        }
                        for (bits, _, _) in frames.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return None;
                    };
                    if !obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(_py, next_bits);
                    }
                    next_bits = tb_bits;
                    built_any = true;
                    frames_built += 1;
                }
                for (bits, _, _) in frames.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
                if !built_any || obj_from_bits(next_bits).is_none() {
                    if !obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(_py, next_bits);
                    }
                    return Some(MoltObject::none().bits());
                }
                if profile_enabled(_py) {
                    TRACEBACK_BUILD_COUNT.fetch_add(1, AtomicOrdering::Relaxed);
                    TRACEBACK_BUILD_FRAMES.fetch_add(frames_built, AtomicOrdering::Relaxed);
                }
                Some(next_bits)
            }
        })();
        if result.is_none() && !exception_pending(_py) {
            raise_exception::<()>(
                _py,
                BuiltinExc::MemoryError,
                "traceback materialization failed",
            );
        }
        result.is_some()
    });
    result
}

/// Return the borrowed published traceback. Absence and failure are distinct,
/// including while the caller is observing an already raised exception.
pub(crate) fn exception_materialize_traceback_bits(
    _py: &PyToken<'_>,
    exc_ptr: *mut u8,
) -> Option<u64> {
    crate::gil_assert();
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return Some(MoltObject::none().bits());
        }
        let trace_bits = crate::exception_trace_bits(exc_ptr);
        if !traceback_payload_is_lazy(trace_bits) {
            return Some(trace_bits);
        }
        let materialized_bits = traceback_payload_to_traceback_bits(_py, trace_bits)?;
        let published = crate::builtins::exceptions::exception_publish_field_slot(
            _py,
            exc_ptr,
            crate::builtins::exceptions::ExceptionFieldSlot::Traceback,
            materialized_bits,
        );
        dec_ref_bits(_py, materialized_bits);
        published.then_some(materialized_bits)
    }
}

pub(crate) fn molt_getframe_attached_body(_py: &crate::PyToken<'_>, depth_bits: u64) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        let depth = if let Some(value) = index_i64_integral_bits(depth_bits) {
            value.to_i32()
        } else {
            let type_error = format!(
                "'{}' object cannot be interpreted as an integer",
                crate::type_name(_py, obj_from_bits(depth_bits))
            );
            let Some(value) = index_bigint_from_obj(_py, depth_bits, &type_error) else {
                return MoltObject::none().bits();
            };
            value.to_i32()
        };
        let Some(depth) = depth else {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::OverflowError,
                "Python int too large to convert to C int",
            );
        };
        match frame_at_depth(_py, depth.max(0) as usize) {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                raise_exception::<u64>(_py, BuiltinExc::ValueError, "call stack is not deep enough")
            }
            Err(()) => MoltObject::none().bits(),
        }
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
    frame_stack_top_entry()
}

/// `locals()` of the executing frame, owned. An optimized activation,
/// synchronous or stateful, answers through its bindings payload: before
/// PEP 667 the activation's one refreshed dict, afterwards an independent
/// snapshot. A module body's locals are its namespace mapping.
pub(crate) fn molt_locals_builtin_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
        if let Some(record) = records::top().filter(|record| {
            record.activation_bits() != 0 || record.bindings().has_python_bindings()
        }) {
            match bindings::frame_record_observe(_py, record) {
                Ok(0) => {}
                Ok(payload) => {
                    return bindings::frame_bindings_locals(_py, payload)
                        .unwrap_or_else(|()| MoltObject::none().bits());
                }
                Err(error) => {
                    error.raise(_py);
                    return MoltObject::none().bits();
                }
            }
        }
        let entry = top_user_frame_entry();
        if let Some(entry) = entry {
            let bits = entry.locals_bits;
            if bits != 0 && !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
                return bits;
            }
            if frame_binding_source(&entry) == MoltObject::none().bits() {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    UNMATERIALIZED_CLASS_LOCALS,
                );
            }
        }
        // Fallback: for module frames, CPython uses f_locals == f_globals.
        if let Some(entry) = entry
            && let Some(field) = frame_globals_field(_py, entry)
        {
            let bits = field.bits;
            if !field.owned && !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        empty_dict_bits(_py)
    })
}

pub(crate) fn molt_globals_builtin_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, {
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

include!("../attached_exports_generated/builtins/frames.rs");

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
        let table_ptr = crate::alloc_bytes(_py, b"\xe8\x00\xe8\x06\xe8\x14");
        let table = MoltObject::from_ptr(table_ptr).bits();
        let code_ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            7,
            table,
            empty_tuple_bits,
            empty_tuple_bits,
            0,
            0,
            0,
        );
        dec_ref_bits(_py, table);
        dec_ref_bits(_py, empty_tuple_bits);
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, name_bits);
        (code_ptr, MoltObject::from_ptr(code_ptr).bits())
    }

    /// A function an invoker hands to the compiled frame of `code`: its entry
    /// is `target` (the code's callable identity), its namespace `globals`,
    /// and its builtins the `__builtins__` entry of `globals` when it has one.
    /// The caller owns the returned function.
    pub(super) fn alloc_invoked_function(
        py: &crate::PyToken<'_>,
        code: u64,
        target: u64,
        globals: u64,
    ) -> *mut u8 {
        let function = crate::builtins::functions::alloc_runtime_function_obj(py, target, 0);
        assert!(!function.is_null());
        unsafe {
            assert!(crate::object::layout::function_set_code_bits(
                py, function, code
            ));
            crate::object::layout::function_set_globals_bits(py, function, globals)
                .expect("invoked function globals");
        }
        function
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
            let _ =
                crate::raise_exception::<u64>(py, crate::BuiltinExc::ValueError, "frame unwind");
            let before = crate::builtins::exceptions::molt_exception_last_pending_attached_body(py);
            assert!(crate::exception_pending(py));
            let result = super::molt_frame_context_set_attached_body(
                py,
                none,
                MoltObject::from_int(1).bits(),
                none,
            );
            assert!(crate::obj_from_bits(result).is_none());
            assert!(crate::exception_pending(py));
            let after = crate::builtins::exceptions::molt_exception_last_pending_attached_body(py);
            assert_eq!(before, after);
            crate::molt_exception_clear_attached_body(py);
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

            frame_stack_push_owned(_py, code_bits, 0, 0, 0);
            assert_eq!(unsafe { ref_count(code_ptr) }, 2);
            frame_stack_pop(_py);
            assert_eq!(unsafe { ref_count(code_ptr) }, 1);

            dec_ref_bits(_py, code_bits);
        });
    }

    #[test]
    fn frame_depth_counts_only_python_visible_entries() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let (_, code) = alloc_test_code(py);
            let globals = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            let builtins = MoltObject::from_ptr(alloc_dict_with_pairs(py, &[])).bits();
            for bits in [code, globals, builtins] {
                inc_ref_bits(py, bits);
            }
            frame_stack_push_owned(py, code, globals, builtins, 0);
            frame_stack_push(py, 0); // Runtime context, not a Python call.

            let frame = super::frame_at_depth(py, 0)
                .expect("frame materialization")
                .expect("visible Python frame");
            assert!(super::frame_at_depth(py, 1).unwrap().is_none());
            assert!(!crate::exception_pending(py));

            dec_ref_bits(py, frame);
            frame_stack_pop(py);
            frame_stack_pop(py);
            for bits in [code, globals, builtins] {
                dec_ref_bits(py, bits);
            }
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
            unsafe { assert!(dict_set_in_place(_py, globals_ptr, key_bits, first_bits)) };
            inc_ref_bits(_py, globals_bits);
            inc_ref_bits(_py, first_bits);
            frame_stack_push_owned(_py, 0, globals_bits, first_bits, 0);
            assert_eq!(frame_stack_active_builtins_bits(), first_bits);

            unsafe { assert!(dict_set_in_place(_py, globals_ptr, key_bits, second_bits)) };
            assert_eq!(
                frame_stack_active_builtins_bits(),
                first_bits,
                "PyEval_GetBuiltins follows stored f_builtins, not a later globals mutation"
            );
            assert_eq!(
                super::frame_effective_builtins_bits(_py, globals_bits)
                    .expect("builtins namespace lookup succeeds"),
                second_bits,
                "a new function captures the current explicit globals entry"
            );
            let unconfigured = MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[])).bits();
            assert_eq!(
                super::frame_effective_builtins_bits(_py, unconfigured)
                    .expect("builtins namespace lookup succeeds"),
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
