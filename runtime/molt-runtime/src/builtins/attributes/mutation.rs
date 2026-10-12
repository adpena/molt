use super::*;
use crate::builtins::attr::{
    DescriptorMutation, DescriptorMutationOutcome, class_inferred_field_offset, descriptor_call1,
    descriptor_call2, descriptor_mutate,
};
use molt_cpython_abi::hooks::AttributeMutation;

unsafe fn readonly_descriptor_metadata(
    py: &PyToken<'_>,
    object: *mut u8,
    name: &str,
) -> Option<u64> {
    if unsafe { object_type_id(object) } == crate::TYPE_ID_NATIVE_DESCRIPTOR
        && native_descriptor_metadata_field(name).is_some()
    {
        Some(raise_exception::<u64>(
            py,
            "AttributeError",
            "readonly attribute",
        ))
    } else {
        None
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CustomMutationDefaultPolicy {
    InvokeAnyHook,
    ContinueOnObjectDefault,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CustomMutationDispatch {
    ContinueStorage,
    Handled,
}

/// Capture the action hook once. A counterpart probe may mutate its namespace.
unsafe fn selected_mutation_hook<'a, 'py>(
    py: &'a PyToken<'py>,
    class: *mut u8,
    mutation: DescriptorMutation,
) -> Result<Option<crate::builtins::exceptions::ExceptionValue<'a, 'py>>, ()> {
    unsafe {
        let names = &runtime_state(py).interned;
        let name = match mutation {
            DescriptorMutation::Set(_) => {
                intern_static_name(py, &names.setattr_name, b"__setattr__")
            }
            DescriptorMutation::Delete => {
                intern_static_name(py, &names.delattr_name, b"__delattr__")
            }
        };
        Ok(class_attr_lookup_raw_mro(py, class, name)?
            .map(|bits| crate::builtins::exceptions::ExceptionValue::pin(py, bits)))
    }
}

unsafe fn invoke_selected_mutation(
    py: &PyToken<'_>,
    hook: crate::builtins::exceptions::ExceptionValue<'_, '_>,
    class: *mut u8,
    object: *mut u8,
    name: u64,
    mutation: DescriptorMutation,
) -> Result<CustomMutationDispatch, ()> {
    unsafe {
        let object_bits = MoltObject::from_ptr(object).bits();
        let result = match mutation {
            DescriptorMutation::Set(value) => {
                descriptor_call2(py, hook.bits(), class, Some(object_bits), name, value)
            }
            DescriptorMutation::Delete => {
                descriptor_call1(py, hook.bits(), class, Some(object_bits), name)
            }
        };
        drop(hook);
        if let Some(result) = result? {
            crate::call::discard_owned_call_result(py, result);
        }
        Ok(CustomMutationDispatch::Handled)
    }
}

unsafe fn dispatch_custom_mutation(
    py: &PyToken<'_>,
    class: *mut u8,
    object: *mut u8,
    name: u64,
    mutation: DescriptorMutation,
    default_policy: CustomMutationDefaultPolicy,
) -> Result<CustomMutationDispatch, ()> {
    unsafe {
        let Some(hook) = selected_mutation_hook(py, class, mutation)? else {
            return Ok(CustomMutationDispatch::ContinueStorage);
        };
        let default = match mutation {
            DescriptorMutation::Set(_) => fn_key!(crate::molt_object_setattr),
            DescriptorMutation::Delete => fn_key!(crate::molt_object_delattr),
        };
        if default_policy == CustomMutationDefaultPolicy::ContinueOnObjectDefault
            && crate::call::type_policy::callable_matches_runtime_symbol(Some(hook.bits()), default)
        {
            return Ok(CustomMutationDispatch::ContinueStorage);
        }
        invoke_selected_mutation(py, hook, class, object, name, mutation)
    }
}

/// A known exact namespace can classify paired defaults without callbacks.
unsafe fn pure_mutation_default_access(
    py: &PyToken<'_>,
    class: *mut u8,
) -> Option<AttributeMutation> {
    unsafe {
        let setter = crate::builtins::attr::class_namespace_lookup_mro_pure(
            py,
            class,
            b"__setattr__",
            None,
        )?;
        let deleter = crate::builtins::attr::class_namespace_lookup_mro_pure(
            py,
            class,
            b"__delattr__",
            None,
        )?;
        for (set, delete, access) in [
            (
                fn_key!(crate::molt_object_setattr),
                fn_key!(crate::molt_object_delattr),
                AttributeMutation::Generic,
            ),
            (
                fn_key!(crate::builtins::methods::type_setattr),
                fn_key!(crate::builtins::methods::type_delattr),
                AttributeMutation::TypeDefault,
            ),
        ] {
            if crate::call::type_policy::callable_matches_runtime_symbol(setter, set)
                && crate::call::type_policy::callable_matches_runtime_symbol(deleter, delete)
            {
                return Some(access);
            }
        }
        None
    }
}

/// Only a selected default needs its counterpart to identify a shared slot.
/// Keep the selected owner pinned; custom hooks never probe the other action.
unsafe fn selected_mutation_default_access(
    py: &PyToken<'_>,
    class: *mut u8,
    mutation: DescriptorMutation,
    selected: u64,
) -> Result<Option<AttributeMutation>, ()> {
    unsafe {
        for (set, delete, access) in [
            (
                fn_key!(crate::molt_object_setattr),
                fn_key!(crate::molt_object_delattr),
                AttributeMutation::Generic,
            ),
            (
                fn_key!(crate::builtins::methods::type_setattr),
                fn_key!(crate::builtins::methods::type_delattr),
                AttributeMutation::TypeDefault,
            ),
        ] {
            let (action, counterpart, expected) = match mutation {
                DescriptorMutation::Set(_) => (set, DescriptorMutation::Delete, delete),
                DescriptorMutation::Delete => (
                    delete,
                    DescriptorMutation::Set(MoltObject::none().bits()),
                    set,
                ),
            };
            if crate::call::type_policy::callable_matches_runtime_symbol(Some(selected), action) {
                let other = selected_mutation_hook(py, class, counterpart)?;
                return Ok(crate::call::type_policy::callable_matches_runtime_symbol(
                    other.as_ref().map(|value| value.bits()),
                    expected,
                )
                .then_some(access));
            }
        }
        Ok(None)
    }
}

/// Two object defaults install the generic slot even on metaclasses; two type
/// defaults install the type slot. A mixed pair invokes the selected wrapper.
unsafe fn normal_class_mutation_access(
    py: &PyToken<'_>,
    class: *mut u8,
    metaclass: *mut u8,
    name: u64,
    mutation: DescriptorMutation,
) -> Result<Option<AttributeMutation>, ()> {
    unsafe {
        if let Some(access) = pure_mutation_default_access(py, metaclass) {
            return Ok(Some(access));
        }
        let Some(selected) = selected_mutation_hook(py, metaclass, mutation)? else {
            return Ok(Some(AttributeMutation::TypeDefault));
        };
        if let Some(access) =
            selected_mutation_default_access(py, metaclass, mutation, selected.bits())?
        {
            return Ok(Some(access));
        }
        invoke_selected_mutation(py, selected, metaclass, class, name, mutation)?;
        Ok(None)
    }
}

/// Shared runtime epoch and C publication operations. Namespace transactions
/// publish before retirement; metadata descriptors preserve their own CPython
/// callback/retirement order. The ABI transaction is the sole owner of views,
/// descendants and physical slots.
pub(crate) struct TypeMutation<'a, 'gil> {
    py: &'a PyToken<'gil>,
    class: *mut u8,
    native: Option<molt_cpython_abi::bridge::RuntimeTypeMutation>,
}

impl<'a, 'gil> TypeMutation<'a, 'gil> {
    /// Direct name/qualname setters update runtime identity without C cache
    /// invalidation. A surrounding TypeDefault operation publishes separately.
    pub(crate) fn runtime_only(py: &'a PyToken<'gil>, class: *mut u8) -> Self {
        Self {
            py,
            class,
            native: None,
        }
    }

    pub(crate) unsafe fn prepare(py: &'a PyToken<'gil>, class: *mut u8, name: u64) -> Option<Self> {
        match unsafe {
            molt_cpython_abi::bridge::RuntimeTypeMutation::prepare(
                MoltObject::from_ptr(class).bits(),
                name,
            )
        } {
            Ok(native) => Some(Self {
                py,
                class,
                native: Some(native),
            }),
            Err(molt_cpython_abi::ErrorIndicatorSet) => {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "class mutation preparation",
                );
                None
            }
        }
    }

    pub(crate) unsafe fn publish(&mut self) -> bool {
        unsafe {
            class_bump_layout_version(self.class);
            self.invalidate_native()
        }
    }

    /// Some CPython-owned descriptors invalidate before storage mutation. They
    /// retain that order while publishing the runtime epoch at their own commit.
    pub(crate) unsafe fn invalidate_native(&mut self) -> bool {
        unsafe {
            let Some(native) = self.native.as_mut() else {
                return true;
            };
            struct PublicationRetirement<'a>(&'a mut molt_cpython_abi::bridge::RuntimeTypeMutation);
            impl Drop for PublicationRetirement<'_> {
                fn drop(&mut self) {
                    crate::object::foreign::retire_native_reference(|| self.0.retire_published());
                }
            }
            // Declared outside the saved-error scope: restoration runs before
            // this guard on both return and unwind, before caller displaced
            // owners. Prepared declaration/registry pins stay in the transaction.
            let retirement = PublicationRetirement(native);
            let mut publish = || {
                if retirement.0.publish() == 0 {
                    Ok(())
                } else {
                    Err(molt_cpython_abi::ErrorIndicatorSet)
                }
            };
            let status = if molt_cpython_abi::api::errors::raised_error_pending() {
                molt_cpython_abi::api::errors::with_saved_error(publish)
            } else {
                publish()
            };
            drop(retirement);
            if status.is_ok() {
                true
            } else {
                crate::cpython_abi_hooks::propagate_native_failure(
                    self.py,
                    "class mutation publication",
                );
                false
            }
        }
    }
}

impl Drop for TypeMutation<'_, '_> {
    fn drop(&mut self) {
        crate::object::foreign::retire_native_reference(|| drop(self.native.take()));
    }
}

unsafe fn mutate_class_descriptor(
    py: &PyToken<'_>,
    class: *mut u8,
    descriptor: u64,
    name: u64,
    mutation: DescriptorMutation,
    access: AttributeMutation,
) -> Result<bool, ()> {
    unsafe {
        let mut publication = if access == AttributeMutation::TypeDefault {
            Some(TypeMutation::prepare(py, class, name).ok_or(())?)
        } else {
            None
        };
        let applied = apply_descriptor_mutation(
            py,
            descriptor,
            MoltObject::from_ptr(class).bits(),
            mutation,
        )?;
        if applied
            && let Some(publication) = publication.as_mut()
            && !publication.publish()
        {
            return Err(());
        }
        Ok(applied)
    }
}

/// Translate protocol-level mutation outcomes at the attribute boundary, where
/// the owner and attribute name needed by CPython-compatible diagnostics live.
/// `Ok(false)` means the class entry is not a data descriptor and ordinary storage
/// mutation should continue.
#[inline]
unsafe fn apply_descriptor_mutation(
    py: &PyToken<'_>,
    descriptor_bits: u64,
    instance_bits: u64,
    mutation: DescriptorMutation,
) -> Result<bool, ()> {
    match unsafe { descriptor_mutate(py, descriptor_bits, instance_bits, mutation) } {
        DescriptorMutationOutcome::Applied => Ok(true),
        DescriptorMutationOutcome::Error => Err(()),
        DescriptorMutationOutcome::NotDescriptor => Ok(false),
    }
}

/// Callable class descriptors share one mutation dispatch. Managed execution
/// fields retain their typed setters; ordinary attributes, including __dict__,
/// reach this descriptor tier before dictionary insertion or deletion.
#[inline]
unsafe fn mutate_callable_descriptor(
    py: &PyToken<'_>,
    object: *mut u8,
    name: u64,
    mutation: DescriptorMutation,
) -> Result<bool, ()> {
    unsafe {
        let Some(class) =
            obj_from_bits(type_of_bits(py, MoltObject::from_ptr(object).bits())).as_ptr()
        else {
            return Ok(false);
        };
        let Some(descriptor) = class_attr_lookup_raw_mro(py, class, name)? else {
            return Ok(false);
        };
        apply_descriptor_mutation(
            py,
            descriptor,
            MoltObject::from_ptr(object).bits(),
            mutation,
        )
    }
}

#[inline]
unsafe fn mutate_cell_descriptor(
    py: &PyToken<'_>,
    cell_ptr: *mut u8,
    name_bits: u64,
    mutation: DescriptorMutation,
) -> Result<bool, ()> {
    let class_bits = crate::builtins::types::cell_class(py);
    let Some(class) = obj_from_bits(class_bits)
        .as_ptr()
        .filter(|class| unsafe { object_type_id(*class) } == TYPE_ID_TYPE)
    else {
        return Err(());
    };
    let Some(descriptor) = (unsafe { class_attr_lookup_raw_mro(py, class, name_bits) })? else {
        return Ok(false);
    };
    unsafe {
        apply_descriptor_mutation(
            py,
            descriptor,
            MoltObject::from_ptr(cell_ptr).bits(),
            mutation,
        )
    }
}

/// Canonical type defaults use one exact string while normal overrides and
/// raw generic storage keep their original name. Use the admitted string bytes;
/// the caller already owns the diagnostic text and need not convert it again.
unsafe fn canonical_type_mutation_name(
    py: &PyToken<'_>,
    name: u64,
    access: AttributeMutation,
) -> Option<(u64, Option<crate::PtrDropGuard>)> {
    unsafe {
        if access == AttributeMutation::Generic || type_of_bits(py, name) == builtin_classes(py).str
        {
            return Some((name, None));
        }
        let source = obj_from_bits(name).as_ptr()?;
        let pointer = alloc_string(
            py,
            std::slice::from_raw_parts(string_bytes(source), string_len(source)),
        );
        if pointer.is_null() {
            return None;
        }
        Some((
            MoltObject::from_ptr(pointer).bits(),
            Some(crate::PtrDropGuard::preserving(pointer)),
        ))
    }
}

/// One namespace transaction for ordinary class assignment and deletion.
/// Key lookup may call Python; after it commits, displaced values stay owned
/// until declaration metadata and the type-cache version are coherent.
unsafe fn mutate_class_namespace(
    py: &PyToken<'_>,
    class_ptr: *mut u8,
    name_bits: u64,
    name: &str,
    value: Option<u64>,
) -> Result<bool, ()> {
    unsafe {
        let Some(dict_ptr) = obj_from_bits(class_dict_bits(class_ptr)).as_ptr() else {
            return Ok(false);
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return Ok(false);
        }
        let mut publication = TypeMutation::prepare(py, class_ptr, name_bits).ok_or(())?;
        let retired = match value {
            Some(bits) => {
                match crate::object::ops::dict_set_deferred(py, dict_ptr, name_bits, bits) {
                    Ok(retired) => retired,
                    Err(()) => return Err(()),
                }
            }
            None => match crate::object::ops::dict_del_deferred(py, dict_ptr, name_bits)? {
                Some(retired) => retired,
                None => return Ok(false),
            },
        };
        if name == "__del__" {
            crate::object::class_refresh_declared_finalizer_flag(py, class_ptr);
        }
        let published = publication.publish();
        drop(retired);
        drop(publication);
        if published { Ok(true) } else { Err(()) }
    }
}

/// Raw generic class mutation shares the physical dictionary owner with the
/// generic getter. Heap classes alias their namespace; static classes have a
/// separate instance dictionary. Neither path invalidates semantic type caches.
unsafe fn mutate_class_physical_dictionary(
    py: &PyToken<'_>,
    class: *mut u8,
    name: u64,
    value: Option<u64>,
) -> Result<bool, ()> {
    unsafe {
        if let Some(value) = value {
            let retired = crate::object::field_storage::set_item_deferred(py, class, name, value)?;
            drop(retired);
            return Ok(true);
        }
        let Some(dictionary) = crate::object::field_storage::current_dictionary(py, class)? else {
            return Ok(false);
        };
        inc_ref_bits(py, dictionary);
        let retired = crate::object::ops::dict_del_deferred(
            py,
            obj_from_bits(dictionary).as_ptr().unwrap(),
            name,
        );
        let applied = match &retired {
            Ok(retired) => Ok(retired.is_some()),
            Err(()) => Err(()),
        };
        drop(retired);
        dec_ref_bits(py, dictionary);
        applied
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(py, {
            if obj_ptr.is_null() {
                return raise_exception::<u64>(py, "AttributeError", "object has no attribute");
            }
            mutate_attr_bytes(
                py,
                MoltObject::from_ptr(obj_ptr).bits(),
                attr_name_ptr,
                attr_name_len_bits,
                DescriptorMutation::Set(val_bits),
            )
        })
    }
}

unsafe fn set_attr_ptr_with_name(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
    access: AttributeMutation,
) -> Result<(), ()> {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if readonly_descriptor_metadata(_py, obj_ptr, attr_name).is_some() {
            return Err(());
        }
        // Foreign (C-extension) object: route through its own `tp_setattro`
        // via the ABI bridge. This is the shared setattr helper every entry
        // point funnels through, so foreign dispatch lives here.
        if type_id == crate::TYPE_ID_FOREIGN {
            let c_ptr = crate::object::foreign::foreign_ptr_from_obj(obj_ptr);
            let rc = molt_cpython_abi::bridge::molt_foreign_setattr(
                c_ptr,
                attr_bits,
                Some(val_bits),
                access,
            );
            if rc < 0 {
                crate::cpython_abi_hooks::propagate_native_failure(
                    _py,
                    "native attribute assignment",
                );
                return Err(());
            }
            return Ok(());
        }
        if type_id == TYPE_ID_MODULE {
            let result = if access == AttributeMutation::Normal
                && let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                    CustomMutationDefaultPolicy::InvokeAnyHook,
                )? == CustomMutationDispatch::Handled
            {
                Ok(())
            } else {
                object_setattr_raw(_py, obj_ptr, attr_bits, attr_name, val_bits)
            };
            return result;
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            let metaclass_bits = type_of_bits(_py, class_bits);
            let access = if access == AttributeMutation::Normal {
                let metaclass = obj_from_bits(metaclass_bits).as_ptr().ok_or(())?;
                let Some(access) = normal_class_mutation_access(
                    _py,
                    obj_ptr,
                    metaclass,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                )?
                else {
                    return Ok(());
                };
                access
            } else {
                access
            };
            if access != AttributeMutation::Generic
                && crate::object::class_is_immutable(_py, obj_ptr)
            {
                // CPython: setting an attribute on an immutable builtin type
                // raises `cannot set '<attr>' attribute of immutable type
                // '<type>'` (version-stable across 3.12/3.13/3.14).
                let class_label = class_name_for_error(class_bits);
                let msg =
                    format!("cannot set '{attr_name}' attribute of immutable type '{class_label}'");
                raise_exception::<()>(_py, "TypeError", &msg);
                return Err(());
            }
            if crate::object::class_definition_is_finished(obj_ptr)
                && matches!(attr_name, "__molt_layout_size__" | "__molt_field_offsets__")
            {
                raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
                return Err(());
            }
            // A class object is an instance of its metaclass. Its metaclass
            // data descriptors therefore own mutation before the class's
            // local metadata and namespace paths.
            let (attr_bits, _canonical_name) =
                canonical_type_mutation_name(_py, attr_bits, access).ok_or(())?;
            if let Some(metaclass) = obj_from_bits(metaclass_bits)
                .as_ptr()
                .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
                && let Some(descriptor) = class_attr_lookup_raw_mro(_py, metaclass, attr_bits)?
                && mutate_class_descriptor(
                    _py,
                    obj_ptr,
                    descriptor,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                    access,
                )?
            {
                return Ok(());
            }
            let applied = if access == AttributeMutation::Generic {
                mutate_class_physical_dictionary(_py, obj_ptr, attr_bits, Some(val_bits))?
            } else {
                mutate_class_namespace(_py, obj_ptr, attr_bits, attr_name, Some(val_bits))?
            };
            if applied {
                return Ok(());
            }
            attr_error(_py, "type", attr_name);
            return Err(());
        }
        if type_id == crate::TYPE_ID_CELL {
            let result =
                mutate_cell_descriptor(_py, obj_ptr, attr_bits, DescriptorMutation::Set(val_bits))?;
            if result {
                return Ok(());
            }
            attr_error(_py, "cell", attr_name);
            return Err(());
        }
        if NativeCallableKind::from_class(_py, object_class_bits(obj_ptr)).is_some() {
            let result = mutate_callable_descriptor(
                _py,
                obj_ptr,
                attr_bits,
                DescriptorMutation::Set(val_bits),
            )?;
            if result {
                return Ok(());
            }
            attr_error(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
            );
            return Err(());
        }
        if type_id == TYPE_ID_FUNCTION {
            // Runtime metadata writers retain direct access. Public mutation
            // follows the sealed Python type, with its writable module member.
            if !crate::object::field_storage::class_allows_dictionary(_py, obj_ptr) {
                attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
                return Err(());
            }
            if attr_name == "__code__" {
                if builtin_classes(_py).is_builtin_callable_class(object_class_bits(obj_ptr)) {
                    raise_exception::<()>(
                        _py,
                        "AttributeError",
                        &format!(
                            "'{}' object has no attribute '__code__'",
                            type_name(_py, MoltObject::from_ptr(obj_ptr)),
                        ),
                    );
                    return Err(());
                }
                let val_obj = obj_from_bits(val_bits);
                let Some(val_ptr) = val_obj.as_ptr() else {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "function __code__ must be a code object",
                    );
                    return Err(());
                };
                if object_type_id(val_ptr) != TYPE_ID_CODE {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "function __code__ must be a code object",
                    );
                    return Err(());
                }
                let Some(code_identity) = crate::object::layout::code_callable_identity(val_ptr)
                else {
                    raise_exception::<()>(
                        _py,
                        "ValueError",
                        "function __code__ must have a published callable identity",
                    );
                    return Err(());
                };
                if !code_identity.call_abi.is_reconstructible() {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "cannot assign opaque runtime-context code to a Python function",
                    );
                    return Err(());
                }
                let function_abi = function_call_abi(obj_ptr);
                if !function_abi.is_reconstructible() {
                    raise_exception::<()>(
                        _py,
                        "TypeError",
                        "cannot replace code on an opaque runtime-context function",
                    );
                    return Err(());
                }
                if function_abi != code_identity.call_abi {
                    raise_exception::<()>(
                        _py,
                        "ValueError",
                        "function and code object use different callable context ABIs",
                    );
                    return Err(());
                }
                let closure_bits = function_closure_bits(obj_ptr);
                let closure_len = if closure_bits == 0 || obj_from_bits(closure_bits).is_none() {
                    0
                } else {
                    let Some(closure) = crate::object::cells::inspect_cell_tuple(closure_bits)
                    else {
                        raise_exception::<()>(
                            _py,
                            "SystemError",
                            "function closure is not a tuple",
                        );
                        return Err(());
                    };
                    if closure.first_non_cell.is_some() {
                        raise_exception::<()>(
                            _py,
                            "SystemError",
                            "function closure contains a non-cell value",
                        );
                        return Err(());
                    }
                    closure.len
                };
                let freevars_bits = code_freevars_bits(val_ptr);
                let Some(freevars_ptr) = obj_from_bits(freevars_bits).as_ptr() else {
                    raise_exception::<()>(
                        _py,
                        "SystemError",
                        "code freevars metadata is not a tuple",
                    );
                    return Err(());
                };
                if object_type_id(freevars_ptr) != TYPE_ID_TUPLE {
                    raise_exception::<()>(
                        _py,
                        "SystemError",
                        "code freevars metadata is not a tuple",
                    );
                    return Err(());
                }
                let freevars_len = crate::object::seq_access::len(freevars_ptr);
                if closure_len != freevars_len {
                    let name = string_obj_to_owned(obj_from_bits(function_name_bits(_py, obj_ptr)))
                        .unwrap_or_else(|| "<function>".to_string());
                    let msg = format!(
                        "{name}() requires a code object with {closure_len} free vars, not {freevars_len}"
                    );
                    raise_exception::<()>(_py, "ValueError", &msg);
                    return Err(());
                }
                if !crate::builtins::functions::function_replace_code_bits(_py, obj_ptr, val_bits) {
                    return Err(());
                }
                return Ok(());
            }
            if attr_name == "__closure__" {
                raise_exception::<()>(_py, "AttributeError", "readonly attribute");
                return Err(());
            }
            if attr_name == "__annotate__" && pep649_enabled(_py) {
                let val_obj = obj_from_bits(val_bits);
                if !val_obj.is_none() {
                    let callable_ok = crate::builtins::callable::is_callable_impl(_py, val_bits);
                    if !callable_ok {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__annotate__ must be callable or None",
                        );
                        return Err(());
                    }
                    function_set_annotations_bits(_py, obj_ptr, 0);
                }
                function_set_annotate_bits(_py, obj_ptr, val_bits);
                return Ok(());
            }
            if attr_name == "__annotations__" {
                let val_obj = obj_from_bits(val_bits);
                let ann_bits = if val_obj.is_none() {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return Err(());
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                } else {
                    let Some(val_ptr) = val_obj.as_ptr() else {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__annotations__ must be set to a dict object",
                        );
                        return Err(());
                    };
                    if object_type_id(val_ptr) != TYPE_ID_DICT {
                        raise_exception::<()>(
                            _py,
                            "TypeError",
                            "__annotations__ must be set to a dict object",
                        );
                        return Err(());
                    }
                    val_bits
                };
                function_set_annotations_bits(_py, obj_ptr, ann_bits);
                if pep649_enabled(_py) {
                    function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                }
                return Ok(());
            }
            let result = mutate_callable_descriptor(
                _py,
                obj_ptr,
                attr_bits,
                DescriptorMutation::Set(val_bits),
            )?;
            if result {
                return Ok(());
            }
            return crate::object::field_storage::set_item(_py, obj_ptr, attr_bits, val_bits);
        }
        if type_id == TYPE_ID_CODE {
            attr_error(_py, "code", attr_name);
            return Err(());
        }
        if type_id == TYPE_ID_DATACLASS {
            if access == AttributeMutation::Normal
                && let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                    CustomMutationDefaultPolicy::InvokeAnyHook,
                )? == CustomMutationDispatch::Handled
            {
                return Ok(());
            }
            let result = dataclass_setattr_inner(
                _py,
                obj_ptr,
                attr_bits,
                attr_name,
                val_bits,
                access == AttributeMutation::Normal,
            );
            return result;
        }
        if crate::object::heap_kind_has_class_shape(type_id)
            || crate::object::native_instance::has_fields(obj_ptr)
            || !crate::object::instance_dict_bits_ptr(obj_ptr).is_null()
        {
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && access == AttributeMutation::Normal
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Set(val_bits),
                    CustomMutationDefaultPolicy::ContinueOnObjectDefault,
                )? == CustomMutationDispatch::Handled
            {
                return Ok(());
            }
            let result = object_setattr_raw(_py, obj_ptr, attr_bits, attr_name, val_bits);
            return result;
        }
        setattr_no_attr_error_with_obj(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        );
        Err(())
    }
}

unsafe fn module_delattr_namespace(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> Result<(), ()> {
    unsafe {
        if attr_name == "__dict__" {
            raise_exception::<()>(_py, "AttributeError", "readonly attribute");
            return Err(());
        }
        let dict_bits = module_dict_bits(obj_ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            let annotations_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotations_name,
                b"__annotations__",
            );
            if annotations_bits == 0 {
                return Err(());
            }
            if crate::object::ops_compare::string_storage_equal(attr_bits, annotations_bits) {
                if dict_del_in_place(_py, dict_ptr, annotations_bits)? {
                    if pep649_enabled(_py) {
                        let annotate_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.annotate_name,
                            b"__annotate__",
                        );
                        if annotate_bits == 0 {
                            return Err(());
                        }
                        let none_bits = MoltObject::none().bits();
                        if !dict_set_in_place(_py, dict_ptr, annotate_bits, none_bits) {
                            return Err(());
                        }
                    }
                    return Ok(());
                }
                let module_name = string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
                raise_exception::<()>(_py, "AttributeError", &msg);
                return Err(());
            }
            let annotate_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.annotate_name,
                b"__annotate__",
            );
            if annotate_bits == 0 {
                return Err(());
            }
            if crate::object::ops_compare::string_storage_equal(attr_bits, annotate_bits)
                && pep649_enabled(_py)
            {
                raise_exception::<()>(_py, "TypeError", "cannot delete __annotate__ attribute");
                return Err(());
            }
            if dict_del_in_place(_py, dict_ptr, attr_bits)? {
                return Ok(());
            }
        }
        let module_name =
            string_obj_to_owned(obj_from_bits(module_name_bits(obj_ptr))).unwrap_or_default();
        let msg = format!("module '{module_name}' has no attribute '{attr_name}'");
        attr_error_with_message(_py, &msg);
        Err(())
    }
}

unsafe fn del_attr_ptr_with_access(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    access: AttributeMutation,
) -> Result<(), ()> {
    unsafe {
        let type_id = object_type_id(obj_ptr);
        if readonly_descriptor_metadata(_py, obj_ptr, attr_name).is_some() {
            return Err(());
        }
        if type_id == crate::TYPE_ID_FOREIGN {
            let c_ptr = crate::object::foreign::foreign_ptr_from_obj(obj_ptr);
            let rc = molt_cpython_abi::bridge::molt_foreign_setattr(c_ptr, attr_bits, None, access);
            if rc < 0 {
                crate::cpython_abi_hooks::propagate_native_failure(
                    _py,
                    "native attribute deletion",
                );
                return Err(());
            }
            return Ok(());
        }
        if type_id == TYPE_ID_MODULE {
            if access == AttributeMutation::Normal
                && let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Delete,
                    CustomMutationDefaultPolicy::InvokeAnyHook,
                )? == CustomMutationDispatch::Handled
            {
                return Ok(());
            }
            return object_delattr_raw(_py, obj_ptr, attr_bits, attr_name);
        }
        if type_id == TYPE_ID_TYPE {
            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            let metaclass_bits = type_of_bits(_py, class_bits);
            let access = if access == AttributeMutation::Normal {
                let metaclass = obj_from_bits(metaclass_bits).as_ptr().ok_or(())?;
                let Some(access) = normal_class_mutation_access(
                    _py,
                    obj_ptr,
                    metaclass,
                    attr_bits,
                    DescriptorMutation::Delete,
                )?
                else {
                    return Ok(());
                };
                access
            } else {
                access
            };
            if access != AttributeMutation::Generic
                && crate::object::class_is_immutable(_py, obj_ptr)
            {
                // CPython routes `del <builtin_type>.<attr>` through the same
                // immutable-type guard as set, yielding `cannot set '<attr>'
                // attribute of immutable type '<type>'` (version-stable).
                let class_label = class_name_for_error(class_bits);
                let msg =
                    format!("cannot set '{attr_name}' attribute of immutable type '{class_label}'");
                raise_exception::<()>(_py, "TypeError", &msg);
                return Err(());
            }
            if crate::object::class_definition_is_finished(obj_ptr)
                && matches!(attr_name, "__molt_layout_size__" | "__molt_field_offsets__")
            {
                raise_exception::<()>(_py, "TypeError", "class layout metadata is immutable");
                return Err(());
            }
            let (attr_bits, _canonical_name) =
                canonical_type_mutation_name(_py, attr_bits, access).ok_or(())?;
            if let Some(metaclass) = obj_from_bits(metaclass_bits)
                .as_ptr()
                .filter(|ptr| object_type_id(*ptr) == TYPE_ID_TYPE)
                && let Some(descriptor) = class_attr_lookup_raw_mro(_py, metaclass, attr_bits)?
                && mutate_class_descriptor(
                    _py,
                    obj_ptr,
                    descriptor,
                    attr_bits,
                    DescriptorMutation::Delete,
                    access,
                )?
            {
                return Ok(());
            }
            let applied = if access == AttributeMutation::Generic {
                mutate_class_physical_dictionary(_py, obj_ptr, attr_bits, None)?
            } else {
                mutate_class_namespace(_py, obj_ptr, attr_bits, attr_name, None)?
            };
            if applied {
                return Ok(());
            }
            if access == AttributeMutation::Generic {
                attr_error(_py, class_name_for_error(metaclass_bits), attr_name);
                return Err(());
            }
            let class_name =
                string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr))).unwrap_or_default();
            let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
            attr_error_with_message(_py, &msg);
            return Err(());
        }
        if type_id == crate::TYPE_ID_CELL {
            let result =
                mutate_cell_descriptor(_py, obj_ptr, attr_bits, DescriptorMutation::Delete)?;
            if result {
                return Ok(());
            }
            attr_error(_py, "cell", attr_name);
            return Err(());
        }
        if NativeCallableKind::from_class(_py, object_class_bits(obj_ptr)).is_some() {
            let result =
                mutate_callable_descriptor(_py, obj_ptr, attr_bits, DescriptorMutation::Delete)?;
            if result {
                return Ok(());
            }
            attr_error(
                _py,
                type_name(_py, MoltObject::from_ptr(obj_ptr)),
                attr_name,
            );
            return Err(());
        }
        if type_id == TYPE_ID_FUNCTION {
            // Runtime metadata writers retain direct access. Public mutation
            // follows the sealed Python type, with its writable module member.
            if !crate::object::field_storage::class_allows_dictionary(_py, obj_ptr) {
                attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
                return Err(());
            }
            if attr_name == "__annotate__" && pep649_enabled(_py) {
                raise_exception::<()>(_py, "TypeError", "cannot delete __annotate__ attribute");
                return Err(());
            }
            if attr_name == "__annotations__" {
                function_set_annotations_bits(_py, obj_ptr, 0);
                if pep649_enabled(_py) {
                    function_set_annotate_bits(_py, obj_ptr, MoltObject::none().bits());
                }
                return Ok(());
            }
            let result =
                mutate_callable_descriptor(_py, obj_ptr, attr_bits, DescriptorMutation::Delete)?;
            if result {
                return Ok(());
            }
            if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits)? {
                return Ok(());
            }
            attr_error(_py, "function", attr_name);
            return Err(());
        }
        if type_id == TYPE_ID_DATACLASS {
            if access == AttributeMutation::Normal
                && let Some(class_ptr) = obj_from_bits(object_class_bits(obj_ptr)).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Delete,
                    CustomMutationDefaultPolicy::InvokeAnyHook,
                )? == CustomMutationDispatch::Handled
            {
                return Ok(());
            }
            return dataclass_delattr_inner(
                _py,
                obj_ptr,
                attr_bits,
                attr_name,
                access == AttributeMutation::Normal,
            );
        }
        if crate::object::heap_kind_has_class_shape(type_id)
            || crate::object::native_instance::has_fields(obj_ptr)
            || !crate::object::instance_dict_bits_ptr(obj_ptr).is_null()
        {
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && access == AttributeMutation::Normal
                && dispatch_custom_mutation(
                    _py,
                    class_ptr,
                    obj_ptr,
                    attr_bits,
                    DescriptorMutation::Delete,
                    CustomMutationDefaultPolicy::ContinueOnObjectDefault,
                )? == CustomMutationDispatch::Handled
            {
                return Ok(());
            }
            return object_delattr_raw(_py, obj_ptr, attr_bits, attr_name);
        }
        // Final fallthrough: DEL of a missing attribute on a no-`__dict__` heap
        // builtin (str/tuple/bytes/frozenset/...). CPython routes del through the
        // generic-setattr-with-NULL path, so the message carries the same
        // version-gated "no __dict__ for setting new attributes" clause (3.13+).
        setattr_no_attr_error_with_obj(
            _py,
            type_name(_py, MoltObject::from_ptr(obj_ptr)),
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        );
        Err(())
    }
}

pub(crate) unsafe fn object_setattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
) -> Result<(), ()> {
    unsafe {
        if readonly_descriptor_metadata(_py, obj_ptr, attr_name).is_some() {
            return Err(());
        }
        let _header = header_from_obj_ptr(obj_ptr);
        if object_type_id(obj_ptr) == TYPE_ID_OBJECT
            && crate::object::object_shape_is_task(crate::object::object_shape_id(obj_ptr))
        {
            attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
            return Err(());
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            attr_error_with_obj(
                _py,
                "object",
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
            return Err(());
        }
        let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
        let class_ptr = obj_from_bits(class_bits)
            .as_ptr()
            .filter(|class| object_type_id(*class) == TYPE_ID_TYPE);
        let mut slots_info = None;
        if let Some(class_ptr) = class_ptr {
            slots_info = class_slots_info(_py, class_ptr);
            let class_attribute = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
            if let Some(desc_bits) = class_attribute
                && apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Set(val_bits),
                )?
            {
                return Ok(());
            }
        }
        if let Some(class_ptr) = class_ptr
            && let Some(offset) = class_inferred_field_offset(_py, class_ptr, attr_bits)?
        {
            return object_field_set_ptr_raw(_py, obj_ptr, offset, val_bits);
        }
        if object_type_id(obj_ptr) == TYPE_ID_MODULE {
            if attr_name == "__dict__" {
                raise_exception::<()>(_py, "AttributeError", "readonly attribute");
                return Err(());
            }
            return crate::builtins::modules::module_set_attr(
                _py,
                MoltObject::from_ptr(obj_ptr).bits(),
                attr_bits,
                val_bits,
            );
        }
        if slots_info.is_none_or(|info| !info.allows_dict) {
            // `__slots__` instance (no `__dict__`) rejecting a non-slot attribute
            // via the `setattr()` builtin path: version-gated no-`__dict__` SET
            // message (3.13+), matching `molt_set_attr_generic`.
            let type_label = class_name_for_error(class_bits);
            setattr_no_attr_error_with_obj(
                _py,
                type_label,
                attr_name,
                MoltObject::from_ptr(obj_ptr).bits(),
            );
            return Err(());
        }
        crate::object::field_storage::set_item(_py, obj_ptr, attr_bits, val_bits)
    }
}

unsafe fn dataclass_setattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    val_bits: u64,
    enforce_frozen: bool,
) -> Result<(), ()> {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if enforce_frozen && !desc_ptr.is_null() && (*desc_ptr).frozen {
            raise_exception::<()>(_py, "TypeError", "cannot assign to frozen dataclass field");
            return Err(());
        }
        if !desc_ptr.is_null() {
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name)
                && crate::object::field_storage::field_at_offset(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                )
                .is_some_and(|field| field.kind.is_declared_slot())
            {
                return crate::object::accessors::object_field_set_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                    val_bits,
                );
            }
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?
                && apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Set(val_bits),
                )?
            {
                return Ok(());
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                return crate::object::accessors::object_field_set_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                    val_bits,
                );
            }
            if !(*desc_ptr).allows_dict {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
                return Err(());
            }
        }
        crate::object::field_storage::set_item(_py, obj_ptr, attr_bits, val_bits)
    }
}

pub(crate) unsafe fn object_delattr_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
) -> Result<(), ()> {
    unsafe {
        if readonly_descriptor_metadata(_py, obj_ptr, attr_name).is_some() {
            return Err(());
        }
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let _header = header_from_obj_ptr(obj_ptr);
        if object_type_id(obj_ptr) == TYPE_ID_OBJECT
            && crate::object::object_shape_is_task(crate::object::object_shape_id(obj_ptr))
        {
            attr_error_with_obj(
                _py,
                class_name_for_error(type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits())),
                attr_name,
                obj_bits,
            );
            return Err(());
        }
        let payload = object_payload_size(obj_ptr);
        if payload < std::mem::size_of::<u64>() {
            attr_error_with_obj(
                _py,
                class_name_for_error(type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits())),
                attr_name,
                obj_bits,
            );
            return Err(());
        }
        let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
        let class_attribute = match obj_from_bits(class_bits)
            .as_ptr()
            .filter(|class| object_type_id(*class) == TYPE_ID_TYPE)
        {
            Some(class) => class_attr_lookup_raw_mro(_py, class, attr_bits)?,
            None => None,
        };
        if let Some(desc_bits) = class_attribute
            && apply_descriptor_mutation(
                _py,
                desc_bits,
                instance_bits_for_call(obj_ptr),
                DescriptorMutation::Delete,
            )?
        {
            return Ok(());
        }
        if object_type_id(obj_ptr) == TYPE_ID_MODULE && attr_name == "__dict__" {
            raise_exception::<()>(_py, "AttributeError", "readonly attribute");
            return Err(());
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(offset) = class_inferred_field_offset(_py, class_ptr, attr_bits)?
        {
            if !crate::object::accessors::object_field_delete_ptr_raw(_py, obj_ptr, offset)? {
                attr_error(_py, class_name_for_error(class_bits), attr_name);
                return Err(());
            }
            return Ok(());
        }
        if object_type_id(obj_ptr) == TYPE_ID_MODULE {
            return module_delattr_namespace(_py, obj_ptr, attr_bits, attr_name);
        }
        if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits)? {
            return Ok(());
        }
        // Deleting a non-existent attribute. CPython appends "and no __dict__ for
        // setting new attributes" (3.13+) ONLY when the instance has no `__dict__`
        // — i.e. a `__slots__`-only class. A class that allows a `__dict__` keeps
        // the bare `'X' object has no attribute 'Y'` message on every version.
        let slots_only = class_bits != 0
            && obj_from_bits(class_bits).as_ptr().is_some_and(|class_ptr| {
                object_type_id(class_ptr) == TYPE_ID_TYPE
                    && class_slots_info(_py, class_ptr).is_some_and(|info| !info.allows_dict)
            });
        if slots_only {
            setattr_no_attr_error_with_obj(
                _py,
                class_name_for_error(class_bits),
                attr_name,
                obj_bits,
            );
            return Err(());
        }
        attr_error(_py, class_name_for_error(class_bits), attr_name);
        Err(())
    }
}

unsafe fn dataclass_delattr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    attr_name: &str,
    enforce_frozen: bool,
) -> Result<(), ()> {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if !desc_ptr.is_null() {
            let class_bits = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            if class_bits != 0
                && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                && object_type_id(class_ptr) == TYPE_ID_TYPE
                && let Some(desc_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?
                && apply_descriptor_mutation(
                    _py,
                    desc_bits,
                    instance_bits_for_call(obj_ptr),
                    DescriptorMutation::Delete,
                )?
            {
                return Ok(());
            }
            if enforce_frozen && (*desc_ptr).frozen {
                raise_exception::<()>(_py, "TypeError", "cannot delete frozen dataclass field");
                return Err(());
            }
            if let Some(&index) = (*desc_ptr).field_name_to_index.get(attr_name) {
                if crate::object::accessors::object_field_delete_ptr_raw(
                    _py,
                    obj_ptr,
                    index * std::mem::size_of::<u64>(),
                )? {
                    return Ok(());
                }
                attr_error_with_obj(
                    _py,
                    &(*desc_ptr).name,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
                return Err(());
            }
            if !(*desc_ptr).allows_dict {
                let name = &(*desc_ptr).name;
                let type_label = if name.is_empty() {
                    "dataclass"
                } else {
                    name.as_str()
                };
                attr_error_with_obj(
                    _py,
                    type_label,
                    attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                );
                return Err(());
            }
        }
        if crate::object::accessors::instance_attribute_delete(_py, obj_ptr, attr_bits)? {
            return Ok(());
        }
        let type_label = if !desc_ptr.is_null() {
            let name = &(*desc_ptr).name;
            if name.is_empty() {
                "dataclass"
            } else {
                name.as_str()
            }
        } else {
            "dataclass"
        };
        attr_error_with_obj(
            _py,
            type_label,
            attr_name,
            MoltObject::from_ptr(obj_ptr).bits(),
        );
        Err(())
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            molt_set_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits, val_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_generic(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(py, {
            if obj_ptr.is_null() {
                return raise_exception::<u64>(py, "AttributeError", "object has no attribute");
            }
            mutate_attr_bytes(
                py,
                MoltObject::from_ptr(obj_ptr).bits(),
                attr_name_ptr,
                attr_name_len_bits,
                DescriptorMutation::Delete,
            )
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_ptr(
    obj_ptr: *mut u8,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            molt_del_attr_generic(obj_ptr, attr_name_ptr, attr_name_len_bits)
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_set_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
    val_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(py, {
            mutate_attr_bytes(
                py,
                obj_bits,
                attr_name_ptr,
                attr_name_len_bits,
                DescriptorMutation::Set(val_bits),
            )
        })
    }
}

/// # Safety
/// Dereferences raw pointers. Caller must ensure attr_name_ptr is valid UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_del_attr_object(
    obj_bits: u64,
    attr_name_ptr: *const u8,
    attr_name_len_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry_nopanic!(py, {
            mutate_attr_bytes(
                py,
                obj_bits,
                attr_name_ptr,
                attr_name_len_bits,
                DescriptorMutation::Delete,
            )
        })
    }
}

/// Byte ABI entries create one exact string owner, then join the named
/// transaction. Set/delete and pointer/tagged receiver entrypoints share this
/// conversion; the name's bytes are never reconstructed inside dispatch.
unsafe fn mutate_attr_bytes(
    py: &PyToken<'_>,
    object: u64,
    name_ptr: *const u8,
    name_len: u64,
    mutation: DescriptorMutation,
) -> u64 {
    unsafe {
        let Some(length) = usize_from_bits(name_len) else {
            return raise_exception::<u64>(py, "OverflowError", "attribute name is too large");
        };
        let bytes = std::slice::from_raw_parts(name_ptr, length);
        let Some(name) = attr_name_bits_from_bytes(py, bytes) else {
            return MoltObject::none().bits();
        };
        let _ = mutate_attr_name(py, object, name, mutation, AttributeMutation::Normal);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, name));
        MoltObject::none().bits()
    }
}

/// Named mutation shares one validation/storage boundary. Generic never calls
/// __setattr__/__delattr__; explicit object methods admit before entering here.
pub(crate) fn mutate_attr_name(
    py: &PyToken<'_>,
    object: u64,
    name: u64,
    mutation: DescriptorMutation,
    access: AttributeMutation,
) -> Result<(), ()> {
    let Some(name_ptr) = obj_from_bits(name).as_ptr() else {
        raise_attr_name_type_error(py, name);
        return Err(());
    };
    unsafe {
        if object_type_id(name_ptr) != TYPE_ID_STRING {
            raise_attr_name_type_error(py, name);
            return Err(());
        }
        inc_ref_bits(py, name);
        let _name_owner = crate::PtrDropGuard::new(name_ptr);
        let attribute =
            string_obj_to_owned(obj_from_bits(name)).unwrap_or_else(|| "<attr>".to_string());
        let Some(object_ptr) = maybe_ptr_from_bits(object) else {
            setattr_no_attr_error_with_obj(
                py,
                type_name(py, obj_from_bits(object)),
                &attribute,
                object,
            );
            return Err(());
        };
        match mutation {
            DescriptorMutation::Set(value) => {
                set_attr_ptr_with_name(py, object_ptr, name, &attribute, value, access)
            }
            DescriptorMutation::Delete => {
                del_attr_ptr_with_access(py, object_ptr, name, &attribute, access)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_attr_name(object: u64, name: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = mutate_attr_name(
            py,
            object,
            name,
            DescriptorMutation::Set(value),
            AttributeMutation::Normal,
        );
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_attr_name(object: u64, name: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _ = mutate_attr_name(
            py,
            object,
            name,
            DescriptorMutation::Delete,
            AttributeMutation::Normal,
        );
        MoltObject::none().bits()
    })
}

/// Raw C PyObject_GenericSetAttr assignment: descriptor/storage only.
pub(crate) fn generic_set_attr_name(object: u64, name: u64, value: u64) -> Result<(), ()> {
    crate::with_gil_entry_nopanic!(py, {
        mutate_attr_name(
            py,
            object,
            name,
            DescriptorMutation::Set(value),
            AttributeMutation::Generic,
        )
    })
}

/// Raw C PyObject_GenericSetAttr deletion (NULL is carried separately).
pub(crate) fn generic_del_attr_name(object: u64, name: u64) -> Result<(), ()> {
    crate::with_gil_entry_nopanic!(py, {
        mutate_attr_name(
            py,
            object,
            name,
            DescriptorMutation::Delete,
            AttributeMutation::Generic,
        )
    })
}

/// Explicit defaults admit by real receiver type and the shared paired slot.
/// A foreign C type in the MRO delegates its slot check to the C authority.
fn explicit_mutation_admitted(
    py: &PyToken<'_>,
    object: u64,
    type_default: bool,
    delete: bool,
) -> bool {
    unsafe {
        let class = match crate::object::class_layout::real_type_bits(py, object) {
            Ok(class) => class,
            Err(()) => {
                crate::cpython_abi_hooks::propagate_native_failure(py, "explicit mutation type");
                return false;
            }
        };
        let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
            dec_ref_bits(py, class);
            return false;
        };
        let _class_owner = crate::PtrDropGuard::new(class_ptr);
        let foreign = object_type_id(class_ptr) == TYPE_ID_FOREIGN
            || (object_type_id(class_ptr) == TYPE_ID_TYPE
                && class_mro_view(py, class_ptr).iter().any(|base| {
                    obj_from_bits(*base)
                        .as_ptr()
                        .is_some_and(|pointer| object_type_id(pointer) == TYPE_ID_FOREIGN)
                }));
        if foreign {
            let admitted =
                molt_cpython_abi::bridge::native_setter_type_admitted(class, type_default, delete);
            if !admitted {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "explicit mutation admission",
                );
            }
            return admitted;
        }
        let is_metaclass = class == builtin_classes(py).type_obj
            || class_mro_view(py, class_ptr).contains(&builtin_classes(py).type_obj);
        if type_default == is_metaclass {
            return true;
        }
        if !type_default && is_metaclass {
            let access = if let Some(access) = pure_mutation_default_access(py, class_ptr) {
                Some(access)
            } else {
                let mutation = if delete {
                    DescriptorMutation::Delete
                } else {
                    DescriptorMutation::Set(MoltObject::none().bits())
                };
                let selected = match selected_mutation_hook(py, class_ptr, mutation) {
                    Ok(selected) => selected,
                    Err(()) => return false,
                };
                match selected {
                    Some(selected) => match selected_mutation_default_access(
                        py,
                        class_ptr,
                        mutation,
                        selected.bits(),
                    ) {
                        Ok(access) => access,
                        Err(()) => return false,
                    },
                    None => None,
                }
            };
            if access == Some(AttributeMutation::Generic) {
                return true;
            }
        }
        let operation = if delete { "__delattr__" } else { "__setattr__" };
        raise_exception::<u64>(
            py,
            "TypeError",
            &format!(
                "can't apply this {operation} to {} object",
                class_name_for_error(class),
            ),
        );
        false
    }
}

pub(crate) fn explicit_object_mutation_admitted(
    py: &PyToken<'_>,
    object: u64,
    delete: bool,
) -> bool {
    if let Some(pointer) = obj_from_bits(object).as_ptr() {
        unsafe {
            if object_type_id(pointer) == TYPE_ID_FOREIGN {
                let admitted = molt_cpython_abi::bridge::foreign_object_setter_admitted(
                    crate::object::foreign::foreign_ptr_from_obj(pointer),
                    delete,
                );
                if !admitted {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "object mutation admission",
                    );
                }
                return admitted;
            }
        }
    }
    explicit_mutation_admitted(py, object, false, delete)
}

pub(crate) fn explicit_type_mutate_attr_name(object: u64, name: u64, value: Option<u64>) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(pointer) = obj_from_bits(object).as_ptr() {
            unsafe {
                if object_type_id(pointer) == TYPE_ID_FOREIGN {
                    if molt_cpython_abi::bridge::molt_foreign_type_setattr(
                        crate::object::foreign::foreign_ptr_from_obj(pointer),
                        name,
                        value,
                    ) < 0
                    {
                        crate::cpython_abi_hooks::propagate_native_failure(py, "type mutation");
                    }
                    return MoltObject::none().bits();
                }
                if object_type_id(pointer) == TYPE_ID_TYPE {
                    if !explicit_mutation_admitted(py, object, true, value.is_none()) {
                        return MoltObject::none().bits();
                    }
                    let _ = type_mutate_attr_name(object, name, value);
                    return MoltObject::none().bits();
                }
            }
        }
        let operation = if value.is_some() {
            "__setattr__"
        } else {
            "__delattr__"
        };
        raise_exception::<u64>(
            py,
            "TypeError",
            &format!(
                "descriptor '{operation}' requires a 'type' object but received a '{}'",
                type_name(py, obj_from_bits(object)),
            ),
        )
    })
}

/// Default type mutation bypasses metaclass overrides but publishes the
/// canonical namespace/cache transaction. Explicit callers own admission.
pub(crate) fn type_mutate_attr_name(object: u64, name: u64, value: Option<u64>) -> Result<(), ()> {
    crate::with_gil_entry_nopanic!(py, {
        mutate_attr_name(
            py,
            object,
            name,
            value.map_or(DescriptorMutation::Delete, DescriptorMutation::Set),
            AttributeMutation::TypeDefault,
        )
    })
}

#[cfg(test)]
mod function_metadata_tests {
    use super::*;
    use crate::object::layout::function_call_target_ptr;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TARGET: AtomicU64 = AtomicU64::new(0);
    static CALLS: AtomicU64 = AtomicU64::new(0);
    static VERSION: AtomicU64 = AtomicU64::new(0);
    static BINDER: AtomicU64 = AtomicU64::new(0);
    static REENTRY: AtomicU64 = AtomicU64::new(0);

    extern "C" fn echo(value: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, value);
            value
        })
    }

    extern "C" fn add_pair(first: u64, second: u64) -> u64 {
        let first = obj_from_bits(first).as_int().unwrap();
        let second = obj_from_bits(second).as_int().unwrap();
        MoltObject::from_int(first + second).bits()
    }

    extern "C" fn observe_metadata_release(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let target = TARGET.load(Ordering::SeqCst);
                let ptr = obj_from_bits(target).as_ptr().unwrap();
                CALLS.fetch_add(1, Ordering::SeqCst);
                VERSION.store(
                    crate::object::layout::function_mutation_version(ptr),
                    Ordering::SeqCst,
                );
                BINDER.store(
                    u64::from(crate::call::bind::function_requires_binder_flag(ptr)),
                    Ordering::SeqCst,
                );
                let result = call_callable1(py, target, MoltObject::from_int(91).bits());
                REENTRY.store(result, Ordering::SeqCst);
                dec_ref_bits(py, result);
                MoltObject::none().bits()
            }
        })
    }

    unsafe fn runtime_function(py: &PyToken<'_>, name: &str, target: *const (), arity: u64) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(name, target),
            arity,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    #[test]
    fn function_code_assignment_rejects_opaque_context_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let opaque = runtime_function(py, "opaque_code_source", echo as *const (), 0);
                let opaque_ptr = obj_from_bits(opaque).as_ptr().unwrap();
                let context = alloc_tuple(py, &[MoltObject::from_int(7).bits()]);
                assert!(!context.is_null());
                let context_bits = MoltObject::from_ptr(context).bits();
                function_set_closure_bits(
                    py,
                    opaque_ptr,
                    context_bits,
                    FunctionCallAbi::OpaqueContextFirst,
                );
                dec_ref_bits(py, context_bits);
                let opaque_code = ensure_function_code_bits(py, opaque_ptr);

                let target = runtime_function(py, "opaque_code_target", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                let original_code = ensure_function_code_bits(py, target_ptr);
                let original_fn = function_fn_ptr(target_ptr);
                let original_trampoline = function_trampoline_ptr(target_ptr);
                let original_arity = function_arity(target_ptr);
                let original_call_target = function_call_target_ptr(target_ptr);
                let result = molt_set_attr_generic(
                    target_ptr,
                    b"__code__".as_ptr(),
                    b"__code__".len() as u64,
                    opaque_code,
                );
                assert!(obj_from_bits(result).is_none());
                assert!(exception_pending(py));
                assert_eq!(function_code_bits(target_ptr), original_code);
                assert_eq!(function_fn_ptr(target_ptr), original_fn);
                assert_eq!(function_trampoline_ptr(target_ptr), original_trampoline);
                assert_eq!(function_arity(target_ptr), original_arity);
                assert_eq!(function_call_target_ptr(target_ptr), original_call_target);
                clear_exception(py);

                dec_ref_bits(py, target);
                dec_ref_bits(py, opaque);
            }
        });
    }

    #[test]
    fn function_code_assignment_retargets_callable_and_projects_code_signature() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let replacement =
                    runtime_function(py, "replacement_pair", add_pair as *const (), 2);
                let replacement_ptr = obj_from_bits(replacement).as_ptr().unwrap();
                let first_name = alloc_string(py, b"first");
                let second_name = alloc_string(py, b"second");
                let empty = alloc_tuple(py, &[]);
                assert!(!first_name.is_null() && !second_name.is_null() && !empty.is_null());
                let first_name_bits = MoltObject::from_ptr(first_name).bits();
                let second_name_bits = MoltObject::from_ptr(second_name).bits();
                let empty_bits = MoltObject::from_ptr(empty).bits();
                let replacement_names = alloc_tuple(py, &[first_name_bits, second_name_bits]);
                assert!(!replacement_names.is_null());
                let replacement_names_bits = MoltObject::from_ptr(replacement_names).bits();
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    replacement_ptr,
                    b"__molt_arg_names__",
                    replacement_names_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    replacement_ptr,
                    b"__molt_kwonly_names__",
                    empty_bits,
                ));
                let replacement_code = ensure_function_code_bits(py, replacement_ptr);

                let target = runtime_function(py, "replacement_owner", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                let original_name = alloc_string(py, b"value");
                assert!(!original_name.is_null());
                let original_name_bits = MoltObject::from_ptr(original_name).bits();
                let original_names = alloc_tuple(py, &[original_name_bits]);
                let defaults = alloc_tuple(py, &[MoltObject::from_int(97).bits()]);
                assert!(!original_names.is_null() && !defaults.is_null());
                let original_names_bits = MoltObject::from_ptr(original_names).bits();
                let defaults_bits = MoltObject::from_ptr(defaults).bits();
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__molt_arg_names__",
                    original_names_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__molt_kwonly_names__",
                    empty_bits,
                ));
                assert!(crate::call::class_init::function_set_attr_name(
                    py,
                    target_ptr,
                    b"__defaults__",
                    defaults_bits,
                ));

                let result = molt_set_attr_generic(
                    target_ptr,
                    b"__code__".as_ptr(),
                    b"__code__".len() as u64,
                    replacement_code,
                );
                assert!(obj_from_bits(result).is_none());
                assert!(!exception_pending(py));
                assert_eq!(function_code_bits(target_ptr), replacement_code);
                assert_eq!(
                    function_fn_ptr(target_ptr),
                    function_fn_ptr(replacement_ptr)
                );
                assert_eq!(function_arity(target_ptr), 2);
                assert_eq!(
                    crate::call::function::function_metadata_bits(
                        py,
                        target_ptr,
                        b"__molt_arg_names__",
                    ),
                    replacement_names_bits
                );
                assert_eq!(
                    crate::call::function::function_metadata_bits(py, target_ptr, b"__defaults__"),
                    defaults_bits
                );

                let called = call_callable1(py, target, MoltObject::from_int(101).bits());
                assert_eq!(obj_from_bits(called).as_int(), Some(198));
                dec_ref_bits(py, called);

                dec_ref_bits(py, target);
                dec_ref_bits(py, replacement);
                for bits in [
                    defaults_bits,
                    original_names_bits,
                    original_name_bits,
                    replacement_names_bits,
                    empty_bits,
                    second_name_bits,
                    first_name_bits,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn function_metadata_set_and_delete_commit_before_finalizer_reentry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let target = runtime_function(py, "metadata_reentry_echo", echo as *const (), 1);
                let target_ptr = obj_from_bits(target).as_ptr().unwrap();
                TARGET.store(target, Ordering::SeqCst);
                let name = attr_name_bits_from_bytes(py, b"MetadataFinalizerProbe").unwrap();
                let class = crate::molt_class_new(name);
                crate::molt_class_set_base(class, builtin_classes(py).object);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                crate::object::class_finish_definition(py, class_ptr).unwrap();
                let finalizer = runtime_function(
                    py,
                    "metadata_release_observer",
                    observe_metadata_release as *const (),
                    1,
                );
                let del_name = attr_name_bits_from_bytes(py, b"__del__").unwrap();
                molt_set_attr_name(class, del_name, finalizer);
                assert!(!exception_pending(py));

                let mut version = 0;
                let mut calls = 0;
                CALLS.store(0, Ordering::SeqCst);
                for key in [
                    b"__defaults__".as_slice(),
                    b"__kwdefaults__",
                    b"__molt_is_generator__",
                ] {
                    let key_bits = attr_name_bits_from_bytes(py, key).unwrap();
                    for delete in [false, true] {
                        let victim = crate::alloc_instance_for_class(py, class_ptr);
                        let value = if key == b"__defaults__" {
                            let tuple = alloc_tuple(py, &[victim]);
                            assert!(!tuple.is_null());
                            dec_ref_bits(py, victim);
                            MoltObject::from_ptr(tuple).bits()
                        } else if key == b"__kwdefaults__" {
                            let dict = alloc_dict_with_pairs(py, &[name, victim]);
                            assert!(!dict.is_null());
                            dec_ref_bits(py, victim);
                            MoltObject::from_ptr(dict).bits()
                        } else {
                            victim
                        };
                        assert!(crate::call::class_init::function_set_attr_bits(
                            py, target_ptr, key_bits, value
                        ));
                        dec_ref_bits(py, value);
                        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
                        if delete {
                            molt_del_attr_name(target, key_bits);
                        } else {
                            molt_set_attr_name(target, key_bits, MoltObject::none().bits());
                        }
                        assert!(!exception_pending(py));
                        calls += 1;
                        if matches!(key, b"__defaults__" | b"__kwdefaults__") {
                            version += 1;
                        }
                        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
                        assert_eq!(VERSION.load(Ordering::SeqCst), version);
                        assert_eq!(BINDER.load(Ordering::SeqCst), 0);
                        assert_eq!(
                            REENTRY.load(Ordering::SeqCst),
                            MoltObject::from_int(91).bits()
                        );
                    }
                    dec_ref_bits(py, key_bits);
                }
                TARGET.store(0, Ordering::SeqCst);
                for bits in [del_name, finalizer, class, name, target] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }
}
