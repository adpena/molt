//! One public-locals authority for created and suspended stateful activations.
//!
//! The compiler registers one immutable typed layout per stateful poll target
//! (`stateful_locals_register`). Generator, coroutine and async-generator frame
//! views, the inspect helpers and pre-entry throw frames all project that
//! layout against the activation's own payload:
//!
//! * created: constructor-bound parameters and captured free cells only; body
//!   locals are unbound until the compiled prologue runs;
//! * started: every public slot, dereferencing the closure cells that the
//!   compiled prologue published, omitting unbound (missing) bindings;
//! * terminal: no bindings.
//!
//! Compiled frames pin the activation; projections follow each cell's actual
//! publication ordinal, including prologue reentry and terminal unwinding.

use std::collections::HashMap;

use molt_codegen_abi::{FRAME_HOME_CELL, FRAME_HOME_HOLDS_REFERENCE, FRAME_HOME_PLAIN};
use molt_obj_model::MoltObject;

use super::bindings::{
    Displacement, FramePolicy, frame_bindings_detach_activation, frame_bindings_policy,
    frame_bindings_release_extras, frame_bindings_retire_activation, frame_policy_for_code, shared,
};
use super::namespace::admit_compiled_namespace;
use crate::object::aux_header::{
    object_frame_bindings_bits, object_frame_code_bits, object_frame_locals_phase,
    object_frame_locals_prestart, object_mark_frame_locals_prestart, object_set_frame_locals_phase,
    object_take_frame_bindings_bits,
};
use crate::object::cells::{cell_ptr_from_bits, cell_replace_value, cell_value_bits};
use crate::object::code_layout::code_slots;
use crate::object::payload_refs::exchange_owned;
use crate::state::runtime_state::{
    StatefulLocalSlot, StatefulLocalsLayout, StatefulLocalsLease, release_stateful_locals_layout,
};
use crate::{
    FRAME_STACK, HEADER_FLAG_GEN_RUNNING, HEADER_FLAG_GEN_STARTED, HEADER_FLAG_TASK_DONE, PyToken,
    TYPE_ID_GENERATOR, TYPE_ID_STRING, TYPE_ID_TUPLE, alloc_dict_with_pairs, dec_ref_bits,
    exception_pending, header_from_obj_ptr, inc_ref_bits, is_missing_bits, missing_bits,
    obj_from_bits, object_type_id, raise_exception, runtime_state, to_i64,
};

/// Edges retained across an allocation that may run finalizers. Released
/// together after the consumer has taken its own references.
struct RetainedEdges<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: Vec<u64>,
}

impl RetainedEdges<'_, '_> {
    fn push(&mut self, bits: u64) {
        inc_ref_bits(self.py, bits);
        self.bits.push(bits);
    }
}

impl Drop for RetainedEdges<'_, '_> {
    fn drop(&mut self) {
        for bits in self.bits.drain(..) {
            dec_ref_bits(self.py, bits);
        }
    }
}

fn schema_error<T>(py: &PyToken<'_>, message: &str) -> Option<T> {
    raise_exception::<u64>(py, "TypeError", message);
    None
}

fn tuple_items(py: &PyToken<'_>, bits: u64, what: &str) -> Option<Vec<u64>> {
    let items = obj_from_bits(bits).as_ptr().and_then(|ptr| unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.to_vec())
    });
    match items {
        Some(items) => Some(items),
        None => schema_error(py, &format!("stateful locals {what} must be a tuple")),
    }
}

fn slot_offset(py: &PyToken<'_>, bits: u64, what: &str) -> Option<usize> {
    match to_i64(obj_from_bits(bits)) {
        Some(value) if value >= 0 && value % 8 == 0 => usize::try_from(value).ok(),
        _ => schema_error(
            py,
            &format!("stateful locals {what} must be an aligned nonnegative offset"),
        ),
    }
}

/// Validate the compiler's `(names, (parameter_count, offsets, cells,
/// closure_offset))` schema. Name edges are borrowed from the tuples.
fn parse_layout(
    py: &PyToken<'_>,
    names_bits: u64,
    layout_bits: u64,
) -> Option<StatefulLocalsLayout> {
    let names = tuple_items(py, names_bits, "names")?;
    for &bits in &names {
        let is_str = obj_from_bits(bits)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_STRING);
        if !is_str {
            return schema_error(py, "stateful locals names must be str");
        }
    }
    let fields = tuple_items(py, layout_bits, "layout")?;
    let &[count_bits, offsets_bits, cells_bits, closure_bits] = fields.as_slice() else {
        return schema_error(py, "stateful locals layout must have four fields");
    };
    let Some(parameter_count) =
        to_i64(obj_from_bits(count_bits)).and_then(|count| usize::try_from(count).ok())
    else {
        return schema_error(
            py,
            "stateful locals parameter count must be a nonnegative int",
        );
    };
    let offsets = tuple_items(py, offsets_bits, "offsets")?;
    let cells = tuple_items(py, cells_bits, "cell ordinals")?;
    if offsets.len() != cells.len()
        || offsets.len() > names.len()
        || parameter_count > offsets.len()
    {
        return schema_error(py, "stateful locals layout shape mismatch");
    }
    let mut slots = Vec::with_capacity(offsets.len());
    let mut previous: Option<usize> = None;
    for (index, (&offset_bits, &cell_bits)) in offsets.iter().zip(&cells).enumerate() {
        let offset = slot_offset(py, offset_bits, "slot")?;
        if previous.is_some_and(|previous| offset <= previous) {
            return schema_error(py, "stateful local slots must have increasing offsets");
        }
        previous = Some(offset);
        if obj_from_bits(cell_bits).as_bool().is_some() {
            return schema_error(
                py,
                "stateful local cell ordinals must be integers, not flags",
            );
        }
        let cell = match to_i64(obj_from_bits(cell_bits)) {
            Some(-1) => None,
            Some(value) if value >= 0 => Some(value as usize),
            _ => return schema_error(py, "stateful local cell ordinal must be -1 or nonnegative"),
        };
        slots.push(StatefulLocalSlot {
            name_bits: names[index],
            offset,
            parameter: index < parameter_count,
            cell,
        });
    }
    let mut cells: Vec<usize> = slots.iter().filter_map(|slot| slot.cell).collect();
    cells.sort_unstable();
    if cells.iter().copied().ne(0..cells.len()) {
        return schema_error(py, "stateful cell ordinals must be unique and contiguous");
    }
    let free_var_names = names[offsets.len()..].to_vec();
    let closure_offset = if obj_from_bits(closure_bits).is_none() {
        None
    } else {
        Some(slot_offset(py, closure_bits, "closure")?)
    };
    if closure_offset.is_some() == free_var_names.is_empty() {
        return schema_error(py, "stateful free variables and closure slot must agree");
    }
    if closure_offset.is_some_and(|closure| slots.iter().any(|slot| slot.offset == closure)) {
        return schema_error(
            py,
            "stateful closure slot must be distinct from local slots",
        );
    }
    Some(StatefulLocalsLayout {
        slots,
        closure_offset,
        free_var_names,
    })
}

/// Publish the compiler's immutable locals layout for one exact poll target.
#[unsafe(no_mangle)]
pub extern "C" fn molt_stateful_locals_register(
    fn_ptr: u64,
    names_bits: u64,
    layout_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if fn_ptr == 0 {
            return MoltObject::none().bits();
        }
        let Some(layout) = parse_layout(py, names_bits, layout_bits) else {
            return MoltObject::none().bits();
        };
        for bits in layout.name_bits() {
            inc_ref_bits(py, bits);
        }
        let previous = runtime_state(py)
            .stateful_locals
            .lock()
            .unwrap()
            .insert(fn_ptr, std::sync::Arc::new(layout));
        // Release displaced names outside the registry lock: a finalizer may
        // define another stateful function and re-enter registration.
        if let Some(previous) = previous {
            release_stateful_locals_layout(py, previous);
        }
        MoltObject::none().bits()
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationState {
    Created,
    Started,
    Terminal,
}

unsafe fn activation_state(ptr: *mut u8) -> ActivationState {
    unsafe {
        let flags = (*header_from_obj_ptr(ptr)).load_synchronized_flags();
        let terminal = if object_type_id(ptr) == TYPE_ID_GENERATOR {
            crate::async_rt::generators::generator_closed(ptr)
        } else {
            (flags & HEADER_FLAG_TASK_DONE) != 0
        };
        if terminal {
            ActivationState::Terminal
        } else if (flags & HEADER_FLAG_GEN_STARTED) != 0 {
            ActivationState::Started
        } else {
            ActivationState::Created
        }
    }
}

unsafe fn payload_word(ptr: *mut u8, payload_size: usize, offset: usize) -> Option<u64> {
    let end = offset.checked_add(std::mem::size_of::<u64>())?;
    (end <= payload_size).then(|| unsafe { *(ptr.add(offset) as *const u64) })
}

pub(super) struct LocalsProjectionError(&'static str, &'static str);

impl LocalsProjectionError {
    pub(super) fn raise(self, py: &PyToken<'_>) {
        if !exception_pending(py) {
            raise_exception::<u64>(py, self.0, self.1);
        }
    }
}

fn layout_violation<T>(
    _py: &PyToken<'_>,
    message: &'static str,
) -> Result<T, LocalsProjectionError> {
    Err(LocalsProjectionError("SystemError", message))
}

fn try_owned_dict(py: &PyToken<'_>, pairs: &[u64]) -> Result<u64, LocalsProjectionError> {
    let ptr = alloc_dict_with_pairs(py, pairs);
    if ptr.is_null() {
        return Err(LocalsProjectionError(
            "MemoryError",
            "cannot allocate frame locals",
        ));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn owned_dict(py: &PyToken<'_>, pairs: &[u64]) -> Option<u64> {
    let ptr = alloc_dict_with_pairs(py, pairs);
    if ptr.is_null() {
        if !exception_pending(py) {
            raise_exception::<u64>(py, "MemoryError", "cannot allocate frame locals");
        }
        return None;
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

/// An activation with no Python bindings to report. `None`: exception pending.
pub(crate) fn empty_locals_bits(py: &PyToken<'_>) -> Option<u64> {
    owned_dict(py, &[])
}

/// Project an activation's Python-visible locals into a new owned dict: the
/// locals a pre-entry activation frame reports and `inspect`'s generator and
/// coroutine locals. A finished activation has no frame and reports none.
/// `None` means an exception is pending. Runtime-native tasks register no
/// layout and report none.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task.
pub(crate) unsafe fn activation_locals_bits(py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    if unsafe { activation_state(ptr) } == ActivationState::Terminal {
        return owned_dict(py, &[]);
    }
    let result = unsafe { try_activation_binding_pairs(py, ptr) }.and_then(|pairs| {
        let flat: Vec<u64> = pairs
            .iter()
            .flat_map(|&(name, value)| [name, value])
            .collect();
        let dict = try_owned_dict(py, &flat);
        for (name, value) in pairs {
            dec_ref_bits(py, name);
            dec_ref_bits(py, value);
        }
        dict
    });
    match result {
        Ok(bits) => Some(bits),
        Err(error) => {
            error.raise(py);
            None
        }
    }
}

/// Every bound public binding of a live stateful activation in its layout
/// order, as owned `(name, value)` pairs: parameters, body locals (a cell
/// variable's published cell shows its contents) and free variables. Before
/// the prologue runs only the constructor-bound parameters are bindings.
/// `Err`: an exception is pending; a layout violation is `SystemError`.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task.
pub(crate) unsafe fn activation_binding_pairs(
    py: &PyToken<'_>,
    ptr: *mut u8,
) -> Result<Vec<(u64, u64)>, ()> {
    unsafe { try_activation_binding_pairs(py, ptr) }.map_err(|error| error.raise(py))
}

/// The registered layout of an activation, leased. `None`: a runtime-native
/// task, which has no Python bindings.
fn activation_layout<'a, 'py>(
    py: &'a PyToken<'py>,
    ptr: *mut u8,
) -> Option<StatefulLocalsLease<'a, 'py>> {
    let key = crate::object::object_poll_fn(ptr);
    let registry = runtime_state(py).stateful_locals.lock().unwrap();
    registry
        .get(&key)
        .map(|layout| StatefulLocalsLease::acquire(py, layout))
}

/// The closure cells of an activation's free variables with their names, both
/// borrowed from the task payload and the layout.
unsafe fn activation_free_cells(
    py: &PyToken<'_>,
    ptr: *mut u8,
    layout: &StatefulLocalsLayout,
    payload_size: usize,
) -> Result<Vec<(u64, u64)>, LocalsProjectionError> {
    let Some(closure_offset) = layout.closure_offset else {
        return Ok(Vec::new());
    };
    let Some(closure_bits) = (unsafe { payload_word(ptr, payload_size, closure_offset) }) else {
        return layout_violation(py, "stateful closure lies outside its activation");
    };
    let closure_cells = obj_from_bits(closure_bits)
        .as_ptr()
        .filter(|closure| unsafe { object_type_id(*closure) } == TYPE_ID_TUPLE)
        .and_then(|closure| unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(closure, |items| items.to_vec())
        });
    let Some(closure_cells) =
        closure_cells.filter(|cells| cells.len() == layout.free_var_names.len())
    else {
        return layout_violation(py, "stateful closure does not match its free variables");
    };
    let mut cells = Vec::with_capacity(closure_cells.len());
    for (&name_bits, &cell_bits) in layout.free_var_names.iter().zip(&closure_cells) {
        if cell_ptr_from_bits(cell_bits).is_none() {
            return layout_violation(py, "stateful closure item is not a cell");
        }
        cells.push((name_bits, cell_bits));
    }
    Ok(cells)
}

unsafe fn try_activation_binding_pairs(
    py: &PyToken<'_>,
    ptr: *mut u8,
) -> Result<Vec<(u64, u64)>, LocalsProjectionError> {
    unsafe {
        let Some(layout) = activation_layout(py, ptr) else {
            return Ok(Vec::new());
        };
        let phase = object_frame_locals_phase(ptr);
        let payload_size = crate::object::object_payload_size(ptr);
        // Retained names and values, released together unless the pairs are
        // returned.
        let mut edges = RetainedEdges {
            py,
            bits: Vec::new(),
        };
        let mut pairs = Vec::with_capacity(layout.slots.len() + layout.free_var_names.len());
        for slot in &layout.slots {
            let StatefulLocalSlot {
                name_bits,
                offset,
                parameter,
                cell,
            } = *slot;
            // Only the constructor has written a created activation's slots.
            if phase == 0 && !parameter {
                continue;
            }
            let Some(bits) = payload_word(ptr, payload_size, offset) else {
                return layout_violation(py, "stateful local slot lies outside its activation");
            };
            // A cell variable holds its plain initial binding until the
            // prologue publishes its cell.
            let value = if cell.is_some_and(|ordinal| phase > ordinal + 1) {
                let Some(cell_ptr) = cell_ptr_from_bits(bits) else {
                    return layout_violation(py, "stateful cell slot does not hold a cell");
                };
                cell_value_bits(cell_ptr)
            } else {
                bits
            };
            if is_missing_bits(py, value) {
                continue;
            }
            edges.push(name_bits);
            edges.push(value);
            pairs.push((name_bits, value));
        }
        for (name_bits, cell_bits) in activation_free_cells(py, ptr, &layout, payload_size)? {
            let value =
                cell_value_bits(cell_ptr_from_bits(cell_bits).expect("checked closure cell"));
            if is_missing_bits(py, value) {
                continue;
            }
            edges.push(name_bits);
            edges.push(value);
            pairs.push((name_bits, value));
        }
        // The pairs own every retained edge now.
        edges.bits.clear();
        Ok(pairs)
    }
}

/// Whether a stateful activation has finished. Its frame reports no bindings
/// to a new observer (`gi_frame` is None): its terminal transition handed
/// them to a shared frame object or released them.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task.
pub(crate) unsafe fn activation_finished(ptr: *mut u8) -> bool {
    unsafe { activation_state(ptr) == ActivationState::Terminal }
}

/// The contents of a validated `str` name.
///
/// # Safety
/// The name must outlive the returned slice.
unsafe fn name_key<'a>(bits: u64) -> &'a [u8] {
    unsafe {
        let name = obj_from_bits(bits)
            .as_ptr()
            .expect("validated binding name");
        std::slice::from_raw_parts(
            crate::object::string_bytes(name),
            crate::object::string_len(name),
        )
    }
}

/// One binding taken out of a finishing activation: its code slot (`None`
/// when the code object does not name it), storage kind and owned bits.
type TakenBinding = (Option<usize>, i64, u64);

/// Take every public binding out of a finishing activation's task payload,
/// each task slot left None: a published cell moves as the frame's cell, a
/// plain binding as itself; unbound slots and a created activation's
/// unwritten body slots contribute nothing. With `free_cells`, each free
/// variable's closure cell is retained as well (the closure keeps its own).
unsafe fn take_activation_bindings(
    py: &PyToken<'_>,
    ptr: *mut u8,
    layout: &StatefulLocalsLayout,
    code_bits: u64,
    free_cells: bool,
) -> Vec<TakenBinding> {
    unsafe {
        let payload_size = crate::object::object_payload_size(ptr);
        // Registration validated increasing offsets: the last one admits all.
        if layout.slots.last().is_some_and(|slot| {
            slot.offset
                .checked_add(8)
                .is_none_or(|end| end > payload_size)
        }) {
            return Vec::new();
        }
        let code_names = obj_from_bits(code_bits)
            .as_ptr()
            .and_then(|code| code_slots(code).ok())
            .map(|slots| slots.names)
            .unwrap_or_default();
        let mut by_name: HashMap<&[u8], usize> = HashMap::with_capacity(code_names.len());
        for (index, &name) in code_names.iter().enumerate() {
            by_name.entry(name_key(name)).or_insert(index);
        }
        let phase = object_frame_locals_phase(ptr);
        let none = MoltObject::none().bits();
        let mut taken = Vec::with_capacity(layout.slots.len() + layout.free_var_names.len());
        for slot in &layout.slots {
            if phase == 0 && !slot.parameter {
                continue;
            }
            let published = slot.cell.is_some_and(|ordinal| phase > ordinal + 1);
            inc_ref_bits(py, none);
            let bits = exchange_owned(py, ptr, slot.offset, none);
            if is_missing_bits(py, bits) {
                dec_ref_bits(py, bits);
                continue;
            }
            let kind = if published {
                FRAME_HOME_CELL
            } else {
                FRAME_HOME_PLAIN
            };
            taken.push((by_name.get(name_key(slot.name_bits)).copied(), kind, bits));
        }
        if free_cells && let Ok(cells) = activation_free_cells(py, ptr, layout, payload_size) {
            for (name_bits, cell_bits) in cells {
                inc_ref_bits(py, cell_bits);
                taken.push((
                    by_name.get(name_key(name_bits)).copied(),
                    FRAME_HOME_CELL,
                    cell_bits,
                ));
            }
        }
        taken
    }
}

/// Release taken bindings in the target's frame-clear order over their code
/// slots, then any the code object does not name, in layout order.
fn release_taken(py: &PyToken<'_>, taken: Vec<TakenBinding>, policy: FramePolicy) {
    let count = taken
        .iter()
        .filter_map(|&(slot, _, _)| slot)
        .max()
        .map_or(0, |slot| slot + 1);
    let mut by_slot: Vec<Option<u64>> = vec![None; count];
    let mut unnamed = Vec::new();
    for (slot, kind, bits) in taken {
        if kind & FRAME_HOME_HOLDS_REFERENCE == 0 {
            continue;
        }
        match slot {
            Some(slot) if by_slot[slot].is_none() => by_slot[slot] = Some(bits),
            _ => unnamed.push(bits),
        }
    }
    policy.for_each_slot(count, |slot| {
        if let Some(bits) = by_slot[slot].take() {
            dec_ref_bits(py, bits);
        }
    });
    for bits in unnamed {
        dec_ref_bits(py, bits);
    }
}

/// The terminal transition of a stateful activation, run once after its
/// terminal state is published and outside every lock (CPython's
/// `gen_clear_frame`). A frame object an observer shares takes the bindings
/// over in its code object's localsplus layout (`take_ownership`); otherwise
/// the frame object's own extras go first (3.13+ extra locals, then 3.14
/// overwritten values newest first), the bindings follow in the target's
/// clear order, and a 3.12 frame's `f_locals` dict ends last. Before 3.14 a
/// created activation that closes keeps its arguments, and its frame object
/// keeps reading them, until the task dies. Control and scratch slots and the
/// closure stay with the task object that owns them.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task under the GIL,
/// whose compiled body no longer runs.
pub(crate) unsafe fn activation_exit_bindings(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        let code_bits = object_frame_code_bits(ptr);
        if code_bits == 0 {
            // A runtime-native task owns no Python bindings.
            return;
        }
        let policy = match obj_from_bits(object_frame_bindings_bits(ptr)).as_ptr() {
            Some(payload) => frame_bindings_policy(payload),
            None => frame_policy_for_code(py, code_bits),
        };
        let started =
            ((*header_from_obj_ptr(ptr)).load_synchronized_flags() & HEADER_FLAG_GEN_STARTED) != 0;
        if !started && !policy.releases_unstarted() {
            return;
        }
        let payload_bits = object_take_frame_bindings_bits(ptr);
        let payload = obj_from_bits(payload_bits).as_ptr();
        let Some(layout) = activation_layout(py, ptr) else {
            if let Some(payload) = payload {
                frame_bindings_detach_activation(payload);
            }
            dec_ref_bits(py, payload_bits);
            return;
        };
        if let Some(payload) = payload
            && shared(payload)
        {
            let taken = take_activation_bindings(py, ptr, &layout, code_bits, true);
            let mut unnamed = Vec::new();
            let retired: Vec<(usize, i64, u64)> = taken
                .into_iter()
                .filter_map(|(slot, kind, bits)| match slot {
                    Some(slot) => Some((slot, kind, bits)),
                    None => {
                        unnamed.push(bits);
                        None
                    }
                })
                .collect();
            frame_bindings_retire_activation(py, payload, retired);
            drop(layout);
            for bits in unnamed {
                dec_ref_bits(py, bits);
            }
            dec_ref_bits(py, payload_bits);
            return;
        }
        let locals = match payload {
            Some(payload) => {
                frame_bindings_detach_activation(payload);
                frame_bindings_release_extras(py, payload)
            }
            None => 0,
        };
        let taken = take_activation_bindings(py, ptr, &layout, code_bits, false);
        drop(layout);
        release_taken(py, taken, policy);
        dec_ref_bits(py, locals);
        dec_ref_bits(py, payload_bits);
    }
}

/// Cycle collection or destruction of a task whose frame object's payload is
/// still attached. An observer that shares the payload takes the bindings
/// over, as the terminal transition would; otherwise the payload stops
/// reading the task. The task's reference goes to `detach`. Moves and retains
/// only: no finalizer runs here.
///
/// # Safety
/// `ptr` must be a generator or native coroutine task under the GIL.
pub(crate) unsafe fn activation_detach_frame_bindings(
    py: &PyToken<'_>,
    ptr: *mut u8,
    detach: &mut dyn FnMut(u64),
) {
    unsafe {
        let payload_bits = object_take_frame_bindings_bits(ptr);
        let Some(payload) = obj_from_bits(payload_bits).as_ptr() else {
            return;
        };
        let code_bits = object_frame_code_bits(ptr);
        let layout = if shared(payload) && code_bits != 0 {
            activation_layout(py, ptr)
        } else {
            None
        };
        match layout {
            Some(layout) => {
                let taken = take_activation_bindings(py, ptr, &layout, code_bits, true);
                drop(layout);
                let mut retired = Vec::with_capacity(taken.len());
                for (slot, kind, bits) in taken {
                    match slot {
                        Some(slot) => retired.push((slot, kind, bits)),
                        None => detach(bits),
                    }
                }
                frame_bindings_retire_activation(py, payload, retired);
            }
            None => frame_bindings_detach_activation(payload),
        }
        detach(payload_bits);
    }
}

fn frame_write_violation<T>(py: &PyToken<'_>, message: &'static str) -> Result<T, ()> {
    LocalsProjectionError("SystemError", message).raise(py);
    Err(())
}

/// Initialize a created activation's complete body-binding prefix, every body
/// slot unbound, before any cell allocation can reenter Python: the compiled
/// prologue's first step, or a frame proxy's write that precedes it.
/// Constructor arguments and captured free cells remain intact. `Err`:
/// `SystemError` is pending and nothing changed.
unsafe fn init_body_prefix(
    py: &PyToken<'_>,
    ptr: *mut u8,
    layout: &StatefulLocalsLayout,
) -> Result<(), ()> {
    unsafe {
        let size = crate::object::object_payload_size(ptr);
        // Registration validates increasing offsets; checking the final one
        // admits the entire slot set without another linear extent scan.
        if layout
            .slots
            .last()
            .is_some_and(|slot| slot.offset.checked_add(8).is_none_or(|end| end > size))
        {
            return frame_write_violation(py, "stateful local slot lies outside its activation");
        }
        let missing = missing_bits(py);
        // The constructor initializes the entire payload to None; before this
        // sole initialization, only parameters/control/scratch slots may have
        // been written. Check every body slot before mutating any. A
        // violation is a storage bug, never permission to discard an owner.
        if layout
            .slots
            .iter()
            .filter(|slot| !slot.parameter)
            .any(|slot| !obj_from_bits(*ptr.add(slot.offset).cast::<u64>()).is_none())
        {
            return frame_write_violation(
                py,
                "stateful body slot was initialized before its prologue",
            );
        }
        for slot in layout.slots.iter().filter(|slot| !slot.parameter) {
            inc_ref_bits(py, missing);
            let previous = exchange_owned(py, ptr, slot.offset, missing);
            // Proven None above: this cannot run a finalizer midway through
            // initialization and needs no allocated displaced-edge buffer.
            dec_ref_bits(py, previous);
        }
        Ok(())
    }
}

/// A PEP 667 proxy write of `value` to the code slot `key` of a live stateful
/// activation's frame, whose payload is `payload`. The task payload is that
/// frame's binding storage and the compiled body reads every binding from it
/// at each access. A plain slot is published before what it displaces is
/// released (3.13) or kept by the frame object (3.14); a published cell's
/// contents and a free variable's closure cell release their old contents
/// during the write. A created activation's body local is written into the
/// initialized prefix its prologue then adopts (a cell variable's initial
/// contents). `Err`: an exception is pending.
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task under the GIL, and
/// `payload` its attached frame payload.
pub(crate) unsafe fn activation_write_binding(
    py: &PyToken<'_>,
    ptr: *mut u8,
    payload: *mut u8,
    key: u64,
    value: u64,
) -> Result<(), ()> {
    unsafe {
        let Some(key) = obj_from_bits(key)
            .as_ptr()
            .filter(|key| object_type_id(*key) == TYPE_ID_STRING)
            .map(|key| {
                std::slice::from_raw_parts(
                    crate::object::string_bytes(key),
                    crate::object::string_len(key),
                )
            })
        else {
            return frame_write_violation(py, "frame slot key is not a str");
        };
        if activation_state(ptr) == ActivationState::Terminal {
            return frame_write_violation(py, "finished activation frame is not live");
        }
        let Some(layout) = activation_layout(py, ptr) else {
            return frame_write_violation(py, "stateful frame has no registered layout");
        };
        let payload_size = crate::object::object_payload_size(ptr);
        if let Some(slot) = layout
            .slots
            .iter()
            .find(|slot| name_key(slot.name_bits) == key)
        {
            let mut phase = object_frame_locals_phase(ptr);
            if phase == 0 && !slot.parameter {
                init_body_prefix(py, ptr, &layout)?;
                object_mark_frame_locals_prestart(ptr);
                phase = 1;
            }
            let Some(bits) = payload_word(ptr, payload_size, slot.offset) else {
                return frame_write_violation(
                    py,
                    "stateful local slot lies outside its activation",
                );
            };
            if slot.cell.is_some_and(|ordinal| phase > ordinal + 1) {
                let Some(cell) = cell_ptr_from_bits(bits) else {
                    return frame_write_violation(py, "stateful cell slot does not hold a cell");
                };
                cell_replace_value(py, cell, value);
            } else if bits != value {
                let displacement = if is_missing_bits(py, bits) {
                    None
                } else {
                    Some(Displacement::plan(py, payload, bits)?)
                };
                inc_ref_bits(py, value);
                let previous = exchange_owned(py, ptr, slot.offset, value);
                // Published first: a finalizer the release runs sees the new
                // binding.
                match displacement {
                    Some(displacement) => displacement.finish(py, previous),
                    None => dec_ref_bits(py, previous),
                }
            }
        } else if let Some(index) = layout
            .free_var_names
            .iter()
            .position(|&name| name_key(name) == key)
        {
            let cells = match activation_free_cells(py, ptr, &layout, payload_size) {
                Ok(cells) => cells,
                Err(error) => {
                    error.raise(py);
                    return Err(());
                }
            };
            let cell = cells
                .get(index)
                .and_then(|&(_, cell)| cell_ptr_from_bits(cell))
                .expect("checked closure cell");
            cell_replace_value(py, cell, value);
        } else {
            return frame_write_violation(
                py,
                "stateful frame slot has no storage in its activation",
            );
        }
        if exception_pending(py) {
            Err(())
        } else {
            Ok(())
        }
    }
}

/// `frame.clear()` of a live stateful activation's frame. An executing
/// activation raises `RuntimeError`, and from 3.13 so does a suspended one.
/// Otherwise, as CPython's `_PyGen_Finalize`, a generator (created, or
/// suspended before 3.13) is closed and its terminal transition releases or
/// hands off the bindings; a close error is reported as unraisable, never
/// raised by `clear`. A created coroutine is left to its owner (CPython warns
/// that it was never awaited; Molt has no such warning).
///
/// # Safety
/// `ptr` must be a live generator or native coroutine task under the GIL.
pub(crate) unsafe fn activation_frame_clear(
    py: &PyToken<'_>,
    ptr: *mut u8,
    policy: FramePolicy,
) -> Result<(), ()> {
    unsafe {
        let flags = (*header_from_obj_ptr(ptr)).load_synchronized_flags();
        if flags & HEADER_FLAG_GEN_RUNNING != 0 {
            raise_exception::<u64>(py, "RuntimeError", "cannot clear an executing frame");
            return Err(());
        }
        if activation_state(ptr) == ActivationState::Terminal {
            return Ok(());
        }
        let started = flags & HEADER_FLAG_GEN_STARTED != 0;
        if started && policy.pep667() {
            raise_exception::<u64>(py, "RuntimeError", "cannot clear a suspended frame");
            return Err(());
        }
        let generator = object_type_id(ptr) == TYPE_ID_GENERATOR;
        if !generator && !started {
            return Ok(());
        }
        let task = MoltObject::from_ptr(ptr).bits();
        crate::builtins::exceptions::run_unraisable_with_policy(
            py,
            || (task, None),
            || {
                let result = if generator {
                    crate::async_rt::generators::molt_generator_close(task)
                } else {
                    crate::async_rt::awaitable::molt_coroutine_close_method(task)
                };
                dec_ref_bits(py, result);
            },
        );
        if exception_pending(py) {
            Err(())
        } else {
            Ok(())
        }
    }
}

/// The single Python frame of an activation that raises before its compiled
/// body is entered: a throw into a created generator, coroutine or async
/// generator. Admission is the compiled-invocation rule; the entry owns the
/// creation-time code, globals and builtins plus the task, whose payload holds
/// the frame's bindings as it does for the running activation. No invocation
/// handoff is published, so nothing remains for a later compiled entry to
/// consume, and no user code runs.
pub(crate) struct ActivationFrameScope<'a, 'py> {
    py: &'a PyToken<'py>,
    depth: Option<usize>,
}

impl<'a, 'py> ActivationFrameScope<'a, 'py> {
    /// `Err` means an exception is pending and no frame was published.
    ///
    /// # Safety
    /// `ptr` must be a live, not-yet-started generator or native coroutine
    /// task whose execution custody the caller holds.
    pub(crate) unsafe fn enter(py: &'a PyToken<'py>, ptr: *mut u8) -> Result<Self, ()> {
        let [globals_bits, builtins_bits, code_bits] =
            crate::object::aux_header::object_frame_context_bits(ptr);
        if code_bits == 0 {
            // Runtime-native tasks own no Python frame to report.
            return Ok(Self { py, depth: None });
        }
        match admit_compiled_namespace(py, code_bits, globals_bits) {
            Err(()) => return Err(()),
            Ok(None) => return Ok(Self { py, depth: None }),
            Ok(Some(_)) => {}
        }
        let task = MoltObject::from_ptr(ptr).bits();
        for bits in [code_bits, globals_bits, builtins_bits, task] {
            inc_ref_bits(py, bits);
        }
        super::frame_stack_push_owned(py, code_bits, globals_bits, builtins_bits, task);
        let depth = FRAME_STACK.with(|stack| stack.borrow().len() - 1);
        Ok(Self {
            py,
            depth: Some(depth),
        })
    }
}

impl Drop for ActivationFrameScope<'_, '_> {
    fn drop(&mut self) {
        let Some(depth) = self.depth else {
            return;
        };
        let len = FRAME_STACK.with(|stack| stack.borrow().len());
        assert_eq!(len, depth + 1, "activation frame custody is not LIFO");
        super::frame_stack_pop(self.py);
    }
}

/// Read the active compiled poll's retained owner and immutable binding layout.
fn active_locals_layout<'a, 'py>(
    py: &'a PyToken<'py>,
) -> Option<(u64, StatefulLocalsLease<'a, 'py>)> {
    let bits = super::frame_stack_active_activation_bits();
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return schema_error(py, "stateful locals publication has no compiled activation");
    };
    let key = crate::object::object_poll_fn(ptr);
    let layout = runtime_state(py)
        .stateful_locals
        .lock()
        .unwrap()
        .get(&key)
        .map(|layout| StatefulLocalsLease::acquire(py, layout));
    let Some(layout) = layout else {
        return schema_error(py, "stateful locals publication has no registered layout");
    };
    Some((bits, layout))
}

/// Initialize the complete body-binding prefix before any cell allocation can
/// reenter Python. Constructor arguments and captured free cells remain intact.
/// A prefix a frame proxy initialized before the body started (writing a body
/// local of the created activation) is adopted as it stands.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_locals_begin() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((owner, layout)) = active_locals_layout(py) else {
            return MoltObject::none().bits();
        };
        let ptr = obj_from_bits(owner).as_ptr().unwrap();
        if object_frame_locals_prestart(ptr) {
            unsafe { object_set_frame_locals_phase(ptr, 1) };
            return MoltObject::none().bits();
        }
        if object_frame_locals_phase(ptr) != 0 {
            return raise_exception::<u64>(py, "SystemError", "stateful prologue entered twice");
        }
        if unsafe { init_body_prefix(py, ptr, &layout) }.is_err() {
            return MoltObject::none().bits();
        }
        unsafe { object_set_frame_locals_phase(ptr, 1) };
        MoltObject::none().bits()
    })
}

/// Atomically publish one prologue cell and its interpretation before releasing
/// a displaced argument. The immutable ordinal validates publication order.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frame_cell_publish(offset_bits: u64, cell_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some((owner, layout)) = active_locals_layout(py) else {
            return MoltObject::none().bits();
        };
        let ptr = obj_from_bits(owner).as_ptr().unwrap();
        let Some(offset) = to_i64(obj_from_bits(offset_bits)).and_then(|n| usize::try_from(n).ok())
        else {
            return raise_exception::<u64>(py, "SystemError", "stateful cell offset is invalid");
        };
        let phase = crate::object::aux_header::object_frame_locals_phase(ptr);
        let admitted = layout
            .slots
            .binary_search_by_key(&offset, |slot| slot.offset)
            .ok()
            .is_some_and(|index| {
                layout.slots[index]
                    .cell
                    .is_some_and(|ordinal| phase == ordinal + 1)
            });
        let size = unsafe { crate::object::object_payload_size(ptr) };
        if !admitted
            || offset.checked_add(8).is_none_or(|end| end > size)
            || cell_ptr_from_bits(cell_bits).is_none()
        {
            return raise_exception::<u64>(
                py,
                "SystemError",
                "stateful cell publication violates its layout",
            );
        }
        inc_ref_bits(py, owner);
        inc_ref_bits(py, cell_bits);
        let previous =
            unsafe { crate::object::payload_refs::exchange_owned(py, ptr, offset, cell_bits) };
        unsafe { crate::object::aux_header::object_set_frame_locals_phase(ptr, phase + 1) };
        dec_ref_bits(py, previous);
        dec_ref_bits(py, owner);
        MoltObject::none().bits()
    })
}
