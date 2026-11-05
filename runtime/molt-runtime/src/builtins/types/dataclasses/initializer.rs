//! One decoration-time initializer contract, using ordinary function storage.
//!
//! A compiled candidate is selected only by its actual schema. Runtime shapes
//! retain immutable index-only body data on the code object; the normal binder,
//! defaults and lexical cells remain the function authorities. In particular,
//! replacing/reconstructing code never accidentally reuses another body's plan.

use super::field_runtime::{DcValue, dc_attr, dc_result};
use super::initializer_spec_generated::{INIT_SPEC_FIELDS, InitSpecField};
use super::*;
use crate::builtins::functions::{
    molt_func_new_closure_attached_body, molt_function_init_metadata_packed_attached_body,
};
use crate::object::class_storage::ClassReferenceSlot;
use crate::object::layout::code_names_bits;
use crate::object::seq_access::with_immutable_tuple_slice;
use std::sync::Arc;

const CODE_INIT_PLAN_WORD: usize = 25;

#[cfg(test)]
#[path = "initializer_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug)]
enum InitInput {
    Argument(usize),
    Capture(usize),
}

#[derive(Clone, Copy, Debug)]
enum ValueSource {
    Value(InitInput),
    Factory {
        factory: InitInput,
        argument: Option<usize>,
    },
}

#[derive(Debug)]
struct InitStore {
    name: usize,
    value: ValueSource,
}

#[derive(Debug)]
enum PostInitInput {
    Argument(usize),
    Global(usize),
}

/// No Python references: names belong to code, factories/defaults to cells.
#[derive(Debug)]
struct InitPlan {
    stores: Vec<InitStore>,
    post_init: Option<(usize, Vec<PostInitInput>)>,
    factory_marker: Option<InitInput>,
    frozen_object: Option<InitInput>,
    arity: usize,
    cells: usize,
    frame: crate::builtins::frames::FramePlan,
}

unsafe fn code_plan_word(code: *mut u8) -> *mut *const InitPlan {
    unsafe { code.cast::<u64>().add(CODE_INIT_PLAN_WORD).cast() }
}

pub(crate) unsafe fn clone_code_init_plan(source: *mut u8, target: *mut u8) {
    unsafe {
        let plan = *code_plan_word(source);
        if !plan.is_null() {
            Arc::increment_strong_count(plan);
            code_plan_word(target).write(plan);
        }
    }
}

pub(crate) unsafe fn drop_code_init_plan(code: *mut u8) {
    unsafe {
        let plan = code_plan_word(code).replace(std::ptr::null());
        if !plan.is_null() {
            drop(Arc::from_raw(plan));
        }
    }
}

fn invalid(py: &PyToken<'_>, message: &str) {
    raise_exception::<()>(py, BuiltinExc::SystemError, message);
}

fn tuple_items(py: &PyToken<'_>, bits: u64) -> Result<Vec<u64>, ()> {
    let items = obj_from_bits(bits)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE })
        .and_then(|ptr| unsafe { with_immutable_tuple_slice(ptr, |items| items.to_vec()) });
    items.ok_or_else(|| invalid(py, "dataclass initializer metadata must be tuples"))
}

fn index(py: &PyToken<'_>, bits: u64, bound: usize) -> Result<Option<usize>, ()> {
    match obj_from_bits(bits).as_int() {
        Some(-1) => Ok(None),
        Some(value) if value >= 0 && (value as usize) < bound => Ok(Some(value as usize)),
        _ => {
            invalid(py, "dataclass initializer metadata index is out of range");
            Err(())
        }
    }
}

fn require_index(py: &PyToken<'_>, bits: u64, bound: usize) -> Result<usize, ()> {
    index(py, bits, bound)?.ok_or_else(|| invalid(py, "dataclass initializer requires an index"))
}

fn input(
    py: &PyToken<'_>,
    argument: u64,
    capture: u64,
    parameters: usize,
    captures: usize,
) -> Result<Option<InitInput>, ()> {
    match (
        index(py, argument, parameters)?,
        index(py, capture, captures)?,
    ) {
        (Some(slot), None) => Ok(Some(InitInput::Argument(slot))),
        (None, Some(cell)) => Ok(Some(InitInput::Capture(cell))),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => {
            invalid(py, "initializer input has two bindings");
            Err(())
        }
    }
}

fn optional_input(
    py: &PyToken<'_>,
    bits: u64,
    parameters: usize,
    captures: usize,
) -> Result<Option<InitInput>, ()> {
    let row = tuple_items(py, bits)?;
    if row.len() != 2 {
        invalid(py, "invalid initializer input binding");
        return Err(());
    }
    input(py, row[0], row[1], parameters, captures)
}

/// Shape selection adds no Python equality callbacks. A string subclass or
/// malformed projection simply cannot certify a compiled template.
fn same_schema(py: &PyToken<'_>, left: u64, right: u64) -> bool {
    if left == right {
        return true;
    }
    if let Some((left, right)) = unsafe {
        crate::object::ops::exact_string_bytes(py, left)
            .zip(crate::object::ops::exact_string_bytes(py, right))
    } {
        return left == right;
    }
    let tuple = |bits| {
        obj_from_bits(bits)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE })
    };
    match (tuple(left), tuple(right)) {
        (Some(left), Some(right)) => unsafe {
            with_immutable_tuple_slice(left, |left| {
                with_immutable_tuple_slice(right, |right| {
                    left.len() == right.len()
                        && left
                            .iter()
                            .zip(right)
                            .all(|(&left, &right)| same_schema(py, left, right))
                })
                .unwrap_or(false)
            })
            .unwrap_or(false)
        },
        _ => false,
    }
}

pub(crate) fn molt_dataclasses_set_init_template_attached_body(
    _py: &crate::PyToken<'_>,
    class: u64,
    schema: u64,
    factory: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let Some(class) = obj_from_bits(class)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
        else {
            // A metaclass may return any object. This optional projection
            // cannot preempt the real decorator's validation or diagnostics.
            return MoltObject::none().bits();
        };
        let candidate = alloc_tuple(py, &[schema, factory]);
        if candidate.is_null() {
            return MoltObject::none().bits();
        }
        let candidate = DcValue::owned(py, MoltObject::from_ptr(candidate).bits());
        unsafe {
            ClassReferenceSlot::DataclassInitTemplate.replace_borrowed(py, class, candidate.bits);
        }
        MoltObject::none().bits()
    })
}

fn owned_tuple<'guard>(py: &'guard PyToken<'guard>, values: &[u64]) -> Result<DcValue<'guard>, ()> {
    dc_result(py, MoltObject::from_ptr(alloc_tuple(py, values)).bits())
}

fn text<'guard>(py: &'guard PyToken<'guard>, bytes: &[u8]) -> Result<DcValue<'guard>, ()> {
    dc_result(py, MoltObject::from_ptr(alloc_string(py, bytes)).bits())
}

/// Capture the function's actual environment before field/descriptor callbacks.
/// This only reads __dict__; it grants no writer access to arbitrary names.

pub(crate) fn molt_dataclasses_init_globals_attached_body(
    _py: &crate::PyToken<'_>,
    module: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        dc_attr(py, module, crate::NameId::DunderDict)
            .map(DcValue::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

fn prepare_globals(py: &PyToken<'_>, globals: u64, builtins: u64) -> Result<(), ()> {
    let ptr = crate::builtins::frames::globals_namespace_storage_ptr(py, globals)?
        .ok_or_else(|| invalid(py, "dataclass initializer globals must be a dictionary"))?;
    let key = DcValue::owned(py, crate::NameId::DunderBuiltins.owned_bits(py).ok_or(())?);
    if unsafe { dict_get_in_place(py, ptr, key.bits) }?.is_none() {
        // CPython's generated-function exec inserts precisely this one key.
        // Keep that bounded effect at function construction, without exposing
        // either module dictionary to an opaque Python consumer.
        let dictionary = dc_attr(py, builtins, crate::NameId::DunderDict)?;
        if !unsafe { dict_set_in_place(py, ptr, key.bits, dictionary.bits) } {
            return Err(());
        }
    }
    Ok(())
}

fn add_name<'guard>(
    py: &'guard PyToken<'guard>,
    names: &mut Vec<DcValue<'guard>>,
    name: u64,
) -> Result<usize, ()> {
    let Some(bytes) = (unsafe { crate::object::ops::exact_string_bytes(py, name) }) else {
        invalid(
            py,
            "dataclass initializer field name must be an exact string",
        );
        return Err(());
    };
    if let Some(index) = names.iter().position(|existing| unsafe {
        crate::object::ops::exact_string_bytes(py, existing.bits) == Some(bytes)
    }) {
        return Ok(index);
    }
    inc_ref_bits(py, name);
    names.push(DcValue::owned(py, name));
    Ok(names.len() - 1)
}

fn prepare_plan<'guard>(
    py: &'guard PyToken<'guard>,
    spec: &[u64],
    has_post_init: bool,
) -> Result<(InitPlan, Vec<DcValue<'guard>>), ()> {
    let parameters = tuple_items(py, spec[InitSpecField::Parameters as usize])?;
    let captures = tuple_items(py, spec[InitSpecField::Captures as usize])?;
    let mut names = Vec::new();
    let mut stores = Vec::new();
    for row in tuple_items(py, spec[InitSpecField::Stores as usize])? {
        let row = tuple_items(py, row)?;
        if row.len() != 5 {
            invalid(py, "invalid dataclass initializer store");
            return Err(());
        }
        let argument = index(py, row[1], parameters.len())?;
        let binding = input(py, row[2], row[3], parameters.len(), captures.len())?;
        let value = if row[4] == MoltObject::from_bool(true).bits() {
            ValueSource::Factory {
                factory: binding.ok_or_else(|| invalid(py, "factory binding is missing"))?,
                argument,
            }
        } else if let Some(argument) = argument {
            ValueSource::Value(InitInput::Argument(argument))
        } else {
            ValueSource::Value(binding.ok_or_else(|| invalid(py, "default binding is missing"))?)
        };
        stores.push(InitStore {
            name: add_name(py, &mut names, row[0])?,
            value,
        });
    }
    let factory_marker = optional_input(
        py,
        spec[InitSpecField::FactoryMarker as usize],
        parameters.len(),
        captures.len(),
    )?;
    if stores.iter().any(|store| {
        matches!(
            store.value,
            ValueSource::Factory {
                argument: Some(_),
                ..
            }
        )
    }) && factory_marker.is_none()
    {
        invalid(py, "factory sentinel capture is missing");
        return Err(());
    }
    let post_rows = tuple_items(py, spec[InitSpecField::PostInitInputs as usize])?;
    let post_init = if has_post_init {
        let post_name = DcValue::owned(py, crate::NameId::DunderPostInit.owned_bits(py).ok_or(())?);
        let name = add_name(py, &mut names, post_name.bits)?;
        let mut inputs = Vec::new();
        for row in post_rows {
            let row = tuple_items(py, row)?;
            if row.len() != 2 {
                invalid(py, "invalid dataclass post-init input");
                return Err(());
            }
            inputs.push(match index(py, row[1], parameters.len())? {
                Some(slot) => PostInitInput::Argument(slot),
                None => PostInitInput::Global(add_name(py, &mut names, row[0])?),
            });
        }
        Some((name, inputs))
    } else {
        None
    };
    Ok((
        InitPlan {
            stores,
            post_init,
            factory_marker,
            frozen_object: optional_input(
                py,
                spec[InitSpecField::FrozenObject as usize],
                parameters.len(),
                captures.len(),
            )?,
            arity: parameters.len(),
            cells: captures.len(),
            frame: crate::builtins::frames::FramePlan::default(),
        },
        names,
    ))
}

fn build_initializer<'guard>(
    py: &'guard PyToken<'guard>,
    class: u64,
    schema: u64,
    packed: u64,
) -> Result<DcValue<'guard>, ()> {
    let class_ptr = obj_from_bits(class)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
        .ok_or_else(|| invalid(py, "dataclass initializer requires a class"))?;
    if obj_from_bits(schema).is_none() && obj_from_bits(packed).is_none() {
        let _candidate = DcValue::owned(py, unsafe {
            ClassReferenceSlot::DataclassInitTemplate.exchange_owned(class_ptr, 0)
        });
        return Ok(DcValue::owned(py, MoltObject::none().bits()));
    }
    let spec = tuple_items(py, packed)?;
    if spec.len() != INIT_SPEC_FIELDS {
        invalid(py, "invalid dataclass initializer specification");
        return Err(());
    }
    let shape = tuple_items(py, schema)?;
    if shape.len() != 5 {
        invalid(py, "invalid dataclass initializer schema");
        return Err(());
    }
    prepare_globals(
        py,
        spec[InitSpecField::Globals as usize],
        spec[InitSpecField::Builtins as usize],
    )?;
    let candidate_owner = DcValue::owned(py, unsafe {
        ClassReferenceSlot::DataclassInitTemplate.exchange_owned(class_ptr, 0)
    });
    let candidate = if candidate_owner.bits == 0 {
        None
    } else {
        let candidate_items = tuple_items(py, candidate_owner.bits)?;
        (candidate_items.len() == 2 && same_schema(py, candidate_items[0], schema))
            .then_some(candidate_items[1])
    };
    let function = if let Some(factory) = candidate {
        dc_result(py, unsafe {
            call_callable1(py, factory, spec[InitSpecField::Captures as usize])
        })?
    } else {
        let (mut plan, names) =
            prepare_plan(py, &spec, shape[2] == MoltObject::from_bool(true).bits())?;
        let name_values: Vec<_> = names.iter().map(|name| name.bits).collect();
        let names_tuple = owned_tuple(py, &name_values)?;
        let parameters = tuple_items(py, spec[InitSpecField::Parameters as usize])?;
        let positional = require_index(
            py,
            spec[InitSpecField::Positional as usize],
            parameters.len() + 1,
        )?;
        let name = DcValue::owned(py, crate::NameId::DunderInit.owned_bits(py).ok_or(())?);
        let filename = text(py, b"<string>")?;
        let code = crate::alloc_code_obj(
            py,
            filename.bits,
            name.bits,
            2,
            MoltObject::none().bits(),
            spec[InitSpecField::Parameters as usize],
            names_tuple.bits,
            positional as u64,
            0,
            (parameters.len() - positional) as u64,
        );
        if code.is_null() {
            return Err(());
        }
        let code = DcValue::owned(py, MoltObject::from_ptr(code).bits());
        let capture_values = tuple_items(py, spec[InitSpecField::Captures as usize])?;
        let mut cells = Vec::new();
        for value in capture_values {
            cells.push(dc_result(
                py,
                crate::object::cells::molt_cell_new_attached_body(py, value),
            )?);
        }
        let cell_values: Vec<_> = cells.iter().map(|cell| cell.bits).collect();
        let closure = owned_tuple(py, &cell_values)?;
        let target = fn_addr!(molt_dataclasses_init_trampoline);
        let function = dc_result(
            py,
            molt_func_new_closure_attached_body(
                py,
                target,
                target,
                parameters.len() as u64,
                if cells.is_empty() { 0 } else { closure.bits },
                molt_codegen_abi::ENTRY_CUSTODY_ADOPTS,
            ),
        )?;
        let qualname = text(py, b"__create_fn__.<locals>.__init__")?;
        let positional_names = owned_tuple(py, &parameters[..positional])?;
        let keyword_names = owned_tuple(py, &parameters[positional..])?;
        let empty = owned_tuple(py, &[])?;
        let none = MoltObject::none().bits();
        let metadata = owned_tuple(
            py,
            &[
                name.bits,
                qualname.bits,
                spec[InitSpecField::Module as usize],
                positional_names.bits,
                MoltObject::from_int(0).bits(),
                keyword_names.bits,
                none,
                none,
                spec[InitSpecField::Defaults as usize],
                spec[InitSpecField::Kwdefaults as usize],
                none,
                MoltObject::from_int(0).bits(),
                spec[InitSpecField::CaptureNames as usize],
                empty.bits,
            ],
        )?;
        dc_result(
            py,
            molt_function_init_metadata_packed_attached_body(
                py,
                function.bits,
                metadata.bits,
                code.bits,
                none,
            ),
        )?;
        plan.frame = crate::builtins::frames::FramePlan::for_code(py, code.bits)
            .map_err(|message| invalid(py, message))?;
        unsafe {
            code_plan_word(obj_from_bits(code.bits).as_ptr().unwrap())
                .write(Arc::into_raw(Arc::new(plan)));
        }
        function
    };
    // Static templates already own code/signature/cells. Both production paths
    // expose exactly the captured defaults and annotations through real fields.
    let ptr = obj_from_bits(function.bits)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == crate::TYPE_ID_FUNCTION })
        .ok_or_else(|| invalid(py, "dataclass template did not return a function"))?;
    // The transform captured the class module namespace before field callbacks.
    // A class naming an absent module uses its own empty globals dictionary.
    unsafe {
        crate::function_set_globals_bits(py, ptr, spec[InitSpecField::Globals as usize])?;
    }
    let set = |name, bits| unsafe {
        crate::call::class_init::function_set_attr_name(py, ptr, name, bits)
    };
    if !set(
        crate::NameId::DunderDefaults,
        spec[InitSpecField::Defaults as usize],
    ) || !set(
        crate::NameId::DunderKwdefaults,
        spec[InitSpecField::Kwdefaults as usize],
    ) || !set(
        crate::NameId::DunderAnnotations,
        spec[InitSpecField::Annotations as usize],
    ) {
        return Err(());
    }
    let qualname = dc_attr(py, class, crate::NameId::DunderQualname)?;
    let qualname = string_obj_to_owned(obj_from_bits(qualname.bits))
        .ok_or_else(|| invalid(py, "dataclass qualname must be a string"))?;
    let qualname = text(py, format!("{qualname}.__init__").as_bytes())?;
    if !set(crate::NameId::DunderQualname, qualname.bits)
        || !set(
            crate::NameId::DunderModule,
            spec[InitSpecField::Module as usize],
        )
    {
        return Err(());
    }
    Ok(function)
}

pub(crate) fn molt_dataclasses_init_attached_body(
    _py: &crate::PyToken<'_>,
    class: u64,
    schema: u64,
    spec: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        build_initializer(py, class, schema, spec)
            .map(DcValue::into_bits)
            .unwrap_or_else(|()| MoltObject::none().bits())
    })
}

fn cell_value<'guard>(
    py: &'guard PyToken<'guard>,
    cells: &[u64],
    index: usize,
) -> Result<DcValue<'guard>, ()> {
    dc_result(
        py,
        crate::object::cells::molt_cell_get_attached_body(py, cells[index]),
    )
}

unsafe fn execute_initializer(
    py: &PyToken<'_>,
    closure: u64,
    argv: u64,
    argc: u64,
) -> Result<(), ()> {
    let count = usize::try_from(argc).map_err(|_| invalid(py, "invalid initializer arity"))?;
    if argv == 0 || count == 0 {
        invalid(py, "invalid bound dataclass initializer arguments");
        return Err(());
    }
    let args = unsafe { std::slice::from_raw_parts(argv as *const u64, count) };
    let pending = crate::call::function::PendingMove::new(py, args);
    let target = fn_key!(molt_dataclasses_init_trampoline);
    let context = crate::builtins::frames::acquire_pending_invocation_context(py, target)
        .ok_or_else(|| invalid(py, "dataclass initializer has no function invocation"))?;
    let [globals, builtins, code] = context.map(|__ptr| DcValue::owned(py, __ptr));
    let code_ptr = obj_from_bits(code.bits).as_ptr().unwrap();
    let plan = unsafe { (*code_plan_word(code_ptr)).as_ref() }
        .ok_or_else(|| invalid(py, "dataclass initializer code has no body plan"))?;
    if count != plan.arity {
        invalid(py, "invalid bound dataclass initializer arity");
        return Err(());
    }
    let cells_owner = if closure == 0 {
        None
    } else {
        let ptr = obj_from_bits(closure)
            .as_ptr()
            .ok_or_else(|| invalid(py, "invalid dataclass initializer closure"))?;
        Some(
            unsafe { crate::object::seq_access::pin_tuple(py, ptr) }
                .ok_or_else(|| invalid(py, "dataclass initializer closure must be a tuple"))?,
        )
    };
    let cells: &[u64] = cells_owner.as_deref().unwrap_or(&[]);
    if cells.len() != plan.cells {
        invalid(py, "invalid dataclass initializer closure");
        return Err(());
    }
    let frame =
        crate::builtins::frames::PythonFunctionFrame::adopt(py, context, plan.frame, args, cells)?;
    pending.into_entry();
    let argument = |slot| frame.argument(slot).map(|__ptr| DcValue::owned(py, __ptr));
    let read = |source| match source {
        InitInput::Argument(slot) => argument(slot),
        InitInput::Capture(cell) => cell_value(py, cells, cell),
    };
    let names_ptr = obj_from_bits(unsafe { code_names_bits(code_ptr) })
        .as_ptr()
        .unwrap();
    let names = unsafe { crate::object::seq_access::pin_tuple(py, names_ptr) }.ok_or(())?;
    for store in &plan.stores {
        // Frozen assignment is an ordinary object.__setattr__ call: its callee
        // and receiver are evaluated BEFORE the factory expression. A normal
        // attribute assignment evaluates its RHS before reading its receiver.
        let frozen_call = if let Some(source) = plan.frozen_object {
            let object = read(source)?;
            Some((
                dc_attr(py, object.bits, crate::NameId::DunderSetattr)?,
                argument(0)?,
            ))
        } else {
            None
        };
        let value = match store.value {
            ValueSource::Value(source) => read(source)?,
            ValueSource::Factory {
                factory,
                argument: slot,
            } => {
                let use_factory = if let Some(slot) = slot {
                    argument(slot)?.bits == read(plan.factory_marker.unwrap())?.bits
                } else {
                    true
                };
                if use_factory {
                    let factory = read(factory)?;
                    dc_result(py, unsafe { call_callable0(py, factory.bits) })?
                } else {
                    argument(slot.unwrap())?
                }
            }
        };
        if let Some((setter, receiver)) = frozen_call {
            dc_result(py, unsafe {
                crate::call_callable3(
                    py,
                    setter.bits,
                    receiver.bits,
                    names[store.name],
                    value.bits,
                )
            })?;
        } else {
            let receiver = argument(0)?;
            dc_result(
                py,
                molt_set_attr_name_attached_body(py, receiver.bits, names[store.name], value.bits),
            )?;
        }
    }
    if let Some((method, inputs)) = &plan.post_init {
        let receiver = argument(0)?;
        let method = dc_result(
            py,
            crate::builtins::attributes::molt_get_attr_name_attached_body(
                py,
                receiver.bits,
                names[*method],
            ),
        )?;
        let mut values = Vec::new();
        for input in inputs {
            values.push(match *input {
                PostInitInput::Argument(slot) => argument(slot)?,
                PostInitInput::Global(name) => {
                    let value = crate::builtins::modules::lookup_namespace_item(
                        py,
                        globals.bits,
                        names[name],
                    )?;
                    let value = match value {
                        Some(value) => Some(value),
                        None => crate::builtins::modules::lookup_builtin_global(
                            py,
                            names[name],
                            builtins.bits,
                        )?,
                    };
                    let Some(value) = value else {
                        let name = string_obj_to_owned(obj_from_bits(names[name])).unwrap();
                        raise_exception::<()>(
                            py,
                            BuiltinExc::NameError,
                            &format!("name '{name}' is not defined"),
                        );
                        return Err(());
                    };
                    DcValue::owned(py, value)
                }
            });
        }
        let builder = crate::molt_callargs_new_attached_body(py, values.len() as u64, 0);
        if exception_pending(py) {
            return Err(());
        }
        for value in &values {
            unsafe {
                crate::molt_callargs_push_pos_attached_body(py, builder, value.bits);
            }
            if exception_pending(py) {
                dec_ref_bits(py, builder);
                return Err(());
            }
        }
        dc_result(
            py,
            crate::molt_call_bind_attached_body(py, method.bits, builder),
        )?;
    }
    Ok(())
}

pub(crate) fn molt_dataclasses_init_trampoline_attached_body(
    _py: &crate::PyToken<'_>,
    closure: u64,
    argv: u64,
    argc: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, py, {
        let _ = unsafe { execute_initializer(py, closure, argv, argc) };
        MoltObject::none().bits()
    })
}

include!("../../../attached_exports_generated/builtins/types/dataclasses/initializer.rs");
