//! Shared builtin member/getset descriptor representation and protocol.
//!
//! Descriptor payloads retain ordinary runtime callable objects. The payload
//! therefore has one portable representation on native and WASM. Six owned
//! reference words retain the owner, metadata and runtime callables; the existing
//! scalar control word packs the flavor and a constructor-sealed operation tag.
//! Callbacks receive the descriptor itself as typed context and decode the tag
//! without allocating names, scanning publication tables or consulting a side
//! registry. No host function pointer is embedded in the object.

use super::*;
use crate::builtins::functions::native_callable::{NativeCallableKind, NativeCallableSpec};
pub(crate) use crate::object::layout::{
    NativeDescriptorFlavor, native_descriptor_deleter_bits, native_descriptor_doc_bits,
    native_descriptor_flavor, native_descriptor_getter_bits, native_descriptor_name_bits,
    native_descriptor_owner_bits, native_descriptor_setter_bits,
};
use crate::{TYPE_ID_MODULE, call_callable3};

#[derive(Clone, Copy)]
pub(crate) struct NativeDescriptorSpec {
    pub(crate) flavor: NativeDescriptorFlavor,
    /// Scalar callback selector; zero means the callback needs no selector.
    pub(crate) operation: u32,
    pub(crate) owner: u64,
    pub(crate) name: u64,
    pub(crate) doc: u64,
    pub(crate) getter: u64,
    pub(crate) setter: u64,
    pub(crate) deleter: u64,
}

impl NativeDescriptorFlavor {
    fn type_name(self) -> &'static str {
        match self {
            Self::Member | Self::ManagedSlot => "member_descriptor",
            Self::GetSet
            | Self::InstanceDictionary
            | Self::CallableMetadata
            | Self::RootMetadata => "getset_descriptor",
        }
    }
}

fn insert_class_attr(py: &PyToken<'_>, dict: *mut u8, name: &[u8], value: u64) -> bool {
    let Some(name) = attr_name_bits_from_bytes(py, name) else {
        return false;
    };
    unsafe { dict_set_in_place(py, dict, name, value) };
    dec_ref_bits(py, name);
    !exception_pending(py)
}

fn build_descriptor_class(py: &PyToken<'_>, flavor: NativeDescriptorFlavor, name: &str) -> u64 {
    let name_ptr = alloc_string(py, name.as_bytes());
    if name_ptr.is_null() {
        return 0;
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let class_ptr = alloc_class_obj(py, name_bits);
    dec_ref_bits(py, name_bits);
    if class_ptr.is_null() {
        return 0;
    }
    let class = MoltObject::from_ptr(class_ptr).bits();
    unsafe {
        crate::object::class_storage::class_declare_native_slots(
            class_ptr,
            crate::object::class_storage::ClassSlotPolicy::default(),
        )
    };
    let builtins = builtin_classes(py);
    if !unsafe {
        crate::object::object_init_class_edge_unpublished(
            py,
            class_ptr,
            builtins.type_obj,
            ClassEdgeOwnership::Owned,
        )
    } {
        dec_ref_bits(py, class);
        return 0;
    }
    let base_result = molt_class_set_base(class, builtins.object);
    dec_ref_bits(py, base_result);
    if exception_pending(py) {
        dec_ref_bits(py, class);
        return 0;
    }

    let new = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        NativeCallableSpec::constructor(class),
        molt_native_descriptor_new as *const () as usize as u64,
    );
    let get = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__get__"),
        molt_native_descriptor_get as *const () as usize as u64,
    );
    let set = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__set__"),
        molt_native_descriptor_set as *const () as usize as u64,
    );
    let delete = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__delete__"),
        molt_native_descriptor_delete as *const () as usize as u64,
    );
    let repr = crate::builtins::methods::builtin_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::WrapperDescriptor, class, "__repr__"),
        molt_native_descriptor_repr as *const () as usize as u64,
        1,
    );
    let reduce = crate::builtins::methods::builtin_func_bits(
        py,
        NativeCallableSpec::declared(NativeCallableKind::MethodDescriptor, class, "__reduce__"),
        molt_native_descriptor_reduce as *const () as usize as u64,
        1,
    );
    if [new, get, set, delete, repr, reduce]
        .into_iter()
        .any(|bits| bits == 0 || exception_pending(py))
    {
        dec_ref_bits(py, class);
        return 0;
    }
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    let Some(dict) = obj_from_bits(dict_bits).as_ptr() else {
        dec_ref_bits(py, class);
        return 0;
    };
    let module_ptr = alloc_string(py, b"builtins");
    if module_ptr.is_null() {
        dec_ref_bits(py, class);
        return 0;
    }
    let module = MoltObject::from_ptr(module_ptr).bits();
    let complete = insert_class_attr(py, dict, b"__module__", module)
        && insert_class_attr(py, dict, b"__new__", new)
        && insert_class_attr(py, dict, b"__get__", get)
        && insert_class_attr(py, dict, b"__set__", set)
        && insert_class_attr(py, dict, b"__delete__", delete)
        && insert_class_attr(py, dict, b"__repr__", repr)
        && insert_class_attr(py, dict, b"__reduce__", reduce);
    dec_ref_bits(py, module);
    if !complete
        || unsafe { crate::object::class_finish_definition(py, class_ptr) }.is_err()
        || !unsafe {
            crate::object::class_storage::ClassSemanticPolicy::static_type(false)
                .apply(py, class_ptr)
        }
    {
        dec_ref_bits(py, class);
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(
                py,
                "SystemError",
                &format!("{} class publication failed", flavor.type_name()),
            );
        }
        return 0;
    }
    class
}

pub(crate) fn member_descriptor_class(py: &PyToken<'_>) -> u64 {
    init_atomic_bits(py, &types_state(py).member_descriptor_class, || {
        build_descriptor_class(py, NativeDescriptorFlavor::Member, "member_descriptor")
    })
}

pub(crate) fn getset_descriptor_class(py: &PyToken<'_>) -> u64 {
    init_atomic_bits(py, &types_state(py).getset_descriptor_class, || {
        build_descriptor_class(py, NativeDescriptorFlavor::GetSet, "getset_descriptor")
    })
}

pub(crate) fn native_descriptor_class(py: &PyToken<'_>, flavor: NativeDescriptorFlavor) -> u64 {
    match flavor {
        NativeDescriptorFlavor::Member | NativeDescriptorFlavor::ManagedSlot => {
            member_descriptor_class(py)
        }
        NativeDescriptorFlavor::GetSet
        | NativeDescriptorFlavor::InstanceDictionary
        | NativeDescriptorFlavor::CallableMetadata
        | NativeDescriptorFlavor::RootMetadata => getset_descriptor_class(py),
    }
}

/// Construct an immutable descriptor from already-materialized runtime
/// callables. None is the only absent-callback sentinel; raw zero is rejected.
pub(crate) fn alloc_native_descriptor(py: &PyToken<'_>, spec: NativeDescriptorSpec) -> u64 {
    let Some(owner_ptr) = obj_from_bits(spec.owner).as_ptr() else {
        return raise_exception::<_>(py, "SystemError", "native descriptor owner is not a type");
    };
    if unsafe { object_type_id(owner_ptr) } != TYPE_ID_TYPE {
        return raise_exception::<_>(py, "SystemError", "native descriptor owner is not a type");
    }
    let Some(name_ptr) = obj_from_bits(spec.name).as_ptr() else {
        return raise_exception::<_>(py, "SystemError", "native descriptor name is not a string");
    };
    if unsafe { object_type_id(name_ptr) } != TYPE_ID_STRING {
        return raise_exception::<_>(py, "SystemError", "native descriptor name is not a string");
    }
    let doc = obj_from_bits(spec.doc);
    if !doc.is_none()
        && !doc
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_STRING)
    {
        return raise_exception::<_>(
            py,
            "SystemError",
            "native descriptor documentation is not str or None",
        );
    }
    for (role, callback) in [
        ("getter", spec.getter),
        ("setter", spec.setter),
        ("deleter", spec.deleter),
    ] {
        if callback == 0 {
            return raise_exception::<_>(
                py,
                "SystemError",
                &format!("native descriptor {role} is raw zero"),
            );
        }
        if !obj_from_bits(callback).is_none()
            && !crate::builtins::callable::is_callable_impl(py, callback)
        {
            return raise_exception::<_>(
                py,
                "SystemError",
                &format!("native descriptor {role} is not callable or None"),
            );
        }
    }
    let class = native_descriptor_class(py, spec.flavor);
    if class == 0 || exception_pending(py) {
        return MoltObject::none().bits();
    }
    let ptr = crate::object::builders::alloc_native_descriptor_obj(
        py,
        class,
        spec.flavor,
        spec.operation,
        spec.owner,
        spec.name,
        spec.doc,
        spec.getter,
        spec.setter,
        spec.deleter,
    );
    if ptr.is_null() {
        if !exception_pending(py) {
            return raise_exception::<_>(py, "MemoryError", "native descriptor allocation failed");
        }
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

struct DescriptorInputs<'a, 'py, const N: usize> {
    py: &'a PyToken<'py>,
    bits: [u64; N],
}

impl<'a, 'py, const N: usize> DescriptorInputs<'a, 'py, N> {
    fn new(py: &'a PyToken<'py>, bits: [u64; N]) -> Self {
        for value in bits {
            inc_ref_bits(py, value);
        }
        Self { py, bits }
    }
}

impl<const N: usize> Drop for DescriptorInputs<'_, '_, N> {
    fn drop(&mut self) {
        for value in self.bits {
            dec_ref_bits(self.py, value);
        }
    }
}

fn descriptor_ptr(
    py: &PyToken<'_>,
    descriptor: u64,
    method: &str,
) -> Option<(*mut u8, NativeDescriptorFlavor)> {
    let Some(ptr) = obj_from_bits(descriptor).as_ptr() else {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("descriptor '{method}' requires a native descriptor object"),
        );
    };
    let Some(flavor) = (unsafe { native_descriptor_flavor(ptr) }) else {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("descriptor '{method}' requires a native descriptor object"),
        );
    };
    let expected_class = native_descriptor_class(py, flavor);
    if expected_class == 0 || exception_pending(py) {
        return None;
    }
    if unsafe { object_class_bits(ptr) } != expected_class {
        return raise_exception::<_>(
            py,
            "SystemError",
            &format!(
                "{} object has an inconsistent native descriptor class edge",
                flavor.type_name(),
            ),
        );
    }
    Some((ptr, flavor))
}

unsafe fn descriptor_text(ptr: *mut u8) -> (String, String) {
    let name = string_obj_to_owned(obj_from_bits(unsafe { native_descriptor_name_bits(ptr) }))
        .unwrap_or_else(|| "?".to_owned());
    let owner = class_name_for_error(unsafe { native_descriptor_owner_bits(ptr) });
    (name, owner)
}

unsafe fn validate_receiver(py: &PyToken<'_>, ptr: *mut u8, instance: u64) -> bool {
    let owner = unsafe { native_descriptor_owner_bits(ptr) };
    match unsafe { crate::object::class_layout::try_is_real_instance(py, instance, owner) } {
        Ok(true) => return true,
        Err(()) => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "native descriptor admission");
            return false;
        }
        Ok(false) => {}
    }
    let (name, owner_name) = unsafe { descriptor_text(ptr) };
    raise_exception::<bool>(
        py,
        "TypeError",
        &format!(
            "descriptor '{name}' for '{owner_name}' objects doesn't apply to a '{}' object",
            type_name(py, obj_from_bits(instance)),
        ),
    )
}

/// A managed descriptor addresses the declaring class's immutable physical row.
/// Receiver admission uses the real class edge, never __class__ spoofing or
/// metaclass __instancecheck__ callbacks before reading object storage.
/// Resolve the declaring class's canonical row without using the receiver's
/// visible name map. Dataclass preparation and descriptor access share this
/// identity, so an ancestor and a same-name child slot cannot collapse.
pub(crate) unsafe fn managed_slot_field(
    py: &PyToken<'_>,
    descriptor: u64,
) -> Option<crate::object::class_layout::ClassField> {
    unsafe {
        let descriptor = obj_from_bits(descriptor).as_ptr()?;
        if object_type_id(descriptor) != crate::TYPE_ID_NATIVE_DESCRIPTOR
            || native_descriptor_flavor(descriptor) != Some(NativeDescriptorFlavor::ManagedSlot)
        {
            return None;
        }
        let owner = obj_from_bits(native_descriptor_owner_bits(descriptor)).as_ptr()?;
        let name = native_descriptor_name_bits(descriptor);
        let offset = crate::builtins::attr::class_own_slot_field_offset(py, owner, name)?;
        crate::object::class_layout::field_at_offset(owner, offset)
            .filter(|field| field.kind.is_declared_slot())
    }
}

unsafe fn managed_slot_location(
    py: &PyToken<'_>,
    descriptor: *mut u8,
    instance: u64,
) -> Option<(*mut u8, usize)> {
    unsafe {
        let owner = native_descriptor_owner_bits(descriptor);
        let instance_ptr = obj_from_bits(instance).as_ptr();
        let actual = instance_ptr.map_or(0, |ptr| object_class_bits(ptr));
        if actual == 0 || !crate::object::class_layout::is_real_subtype(py, actual, owner) {
            let (name, owner_name) = descriptor_text(descriptor);
            raise_exception::<()>(
                py,
                "TypeError",
                &format!(
                    "descriptor '{name}' for '{owner_name}' objects doesn't apply to a '{}' object",
                    type_name(py, obj_from_bits(instance)),
                ),
            );
            return None;
        }
        let object = instance_ptr.unwrap();
        if let Some(field) = managed_slot_field(py, MoltObject::from_ptr(descriptor).bits()) {
            let offset = if object_type_id(object) == TYPE_ID_DATACLASS {
                crate::object::field_storage::dataclass_slot_storage_offset(object, field.offset)
            } else {
                Some(field.offset)
            };
            if let Some(offset) = offset {
                return Some((object, offset));
            }
        }
        raise_exception::<()>(
            py,
            "SystemError",
            "managed slot is absent from its physical layout",
        );
        None
    }
}

unsafe fn managed_slot_missing(py: &PyToken<'_>, descriptor: *mut u8, instance: u64) -> u64 {
    let (name, _) = unsafe { descriptor_text(descriptor) };
    crate::builtins::attr::attr_error_with_obj(
        py,
        type_name(py, obj_from_bits(instance)),
        &name,
        instance,
    )
}

unsafe fn managed_slot_get(py: &PyToken<'_>, descriptor: *mut u8, instance: u64) -> u64 {
    unsafe {
        let Some((object, offset)) = managed_slot_location(py, descriptor, instance) else {
            return MoltObject::none().bits();
        };
        let value = crate::object_field_get_ptr_raw(py, object, offset);
        if exception_pending(py) {
            dec_ref_bits(py, value);
            return MoltObject::none().bits();
        }
        if crate::is_missing_bits(py, value) {
            dec_ref_bits(py, value);
            return managed_slot_missing(py, descriptor, instance);
        }
        value
    }
}

unsafe fn managed_slot_mutate(
    py: &PyToken<'_>,
    descriptor: *mut u8,
    instance: u64,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let Some((object, offset)) = managed_slot_location(py, descriptor, instance) else {
            return MoltObject::none().bits();
        };
        if let Some(value) = value {
            crate::object_field_set_ptr_raw(py, object, offset, value)
        } else if crate::object::accessors::object_field_delete_ptr_raw(py, object, offset)
            || exception_pending(py)
        {
            MoltObject::none().bits()
        } else {
            managed_slot_missing(py, descriptor, instance)
        }
    }
}

unsafe fn missing_accessor(
    py: &PyToken<'_>,
    ptr: *mut u8,
    flavor: NativeDescriptorFlavor,
    readable: bool,
) -> u64 {
    if flavor == NativeDescriptorFlavor::Member && !readable {
        return raise_exception::<_>(py, "AttributeError", "readonly attribute");
    }
    let (name, owner) = unsafe { descriptor_text(ptr) };
    let operation = if readable { "readable" } else { "writable" };
    raise_exception::<_>(
        py,
        "AttributeError",
        &format!("attribute '{name}' of '{owner}' objects is not {operation}"),
    )
}

pub(crate) unsafe fn native_descriptor_get(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: Option<u64>,
    owner: Option<u64>,
) -> u64 {
    let Some((ptr, flavor)) = descriptor_ptr(py, descriptor, "__get__") else {
        return MoltObject::none().bits();
    };
    let Some(instance) = instance else {
        if owner.is_none() {
            return raise_exception::<_>(py, "TypeError", "__get__(None, None) is invalid");
        }
        inc_ref_bits(py, descriptor);
        return descriptor;
    };
    if flavor == NativeDescriptorFlavor::ManagedSlot {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance]);
        return unsafe { managed_slot_get(py, ptr, instance) };
    }
    if !unsafe { validate_receiver(py, ptr, instance) } {
        return MoltObject::none().bits();
    }
    if flavor == NativeDescriptorFlavor::InstanceDictionary {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance]);
        let Some(object) = obj_from_bits(instance).as_ptr() else {
            return raise_exception::<_>(py, "AttributeError", "object has no instance dictionary");
        };
        let Some(value) = (unsafe { crate::object::field_storage::materialize(py, object) }) else {
            return MoltObject::none().bits();
        };
        inc_ref_bits(py, value);
        return value;
    }
    if flavor == NativeDescriptorFlavor::CallableMetadata {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance]);
        let field = crate::builtins::functions::native_callable::CallableMetadata::from_operation(
            unsafe { crate::object::layout::native_descriptor_operation(ptr) }.unwrap(),
        )
        .unwrap();
        let Some(object) = obj_from_bits(instance).as_ptr() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "callable metadata requires a heap receiver",
            );
        };
        return match unsafe {
            crate::builtins::attributes::callable_metadata::read(py, object, field)
        } {
            Some(value) => value,
            None if exception_pending(py) => MoltObject::none().bits(),
            None => raise_exception::<_>(py, "AttributeError", field.name()),
        };
    }
    if flavor == NativeDescriptorFlavor::RootMetadata {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance]);
        let operation = unsafe { crate::object::layout::native_descriptor_operation(ptr) }.unwrap();
        let Some(field) =
            molt_cpython_abi::api::typeobj::TypeAttributeField::from_operation(operation)
        else {
            return raise_exception::<_>(py, "SystemError", "invalid root metadata operation");
        };
        return unsafe { crate::builtins::attributes::type_metadata::read(py, instance, field) };
    }
    let getter = unsafe { native_descriptor_getter_bits(ptr) };
    if obj_from_bits(getter).is_none() {
        return unsafe { missing_accessor(py, ptr, flavor, true) };
    }
    let _inputs = DescriptorInputs::new(py, [descriptor, instance, getter]);
    unsafe { call_callable2(py, getter, descriptor, instance) }
}

pub(crate) unsafe fn native_descriptor_mutate(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
    value: Option<u64>,
) -> u64 {
    let method = if value.is_some() {
        "__set__"
    } else {
        "__delete__"
    };
    let Some((ptr, flavor)) = descriptor_ptr(py, descriptor, method) else {
        return MoltObject::none().bits();
    };
    if flavor == NativeDescriptorFlavor::ManagedSlot {
        let _inputs = DescriptorInputs::new(
            py,
            [
                descriptor,
                instance,
                value.unwrap_or(MoltObject::none().bits()),
            ],
        );
        return unsafe { managed_slot_mutate(py, ptr, instance, value) };
    }
    if !unsafe { validate_receiver(py, ptr, instance) } {
        return MoltObject::none().bits();
    }
    if flavor == NativeDescriptorFlavor::InstanceDictionary {
        let _inputs = DescriptorInputs::new(
            py,
            [
                descriptor,
                instance,
                value.unwrap_or(MoltObject::none().bits()),
            ],
        );
        let Some(object) = obj_from_bits(instance).as_ptr() else {
            return raise_exception::<_>(py, "AttributeError", "object has no instance dictionary");
        };
        if unsafe { object_type_id(object) } == TYPE_ID_MODULE {
            return raise_exception::<_>(py, "AttributeError", "readonly attribute");
        }
        unsafe {
            crate::object::field_storage::replace_dictionary(py, object, value);
        }
        return MoltObject::none().bits();
    }
    if flavor == NativeDescriptorFlavor::CallableMetadata {
        let _inputs = DescriptorInputs::new(
            py,
            [
                descriptor,
                instance,
                value.unwrap_or(MoltObject::none().bits()),
            ],
        );
        let field = crate::builtins::functions::native_callable::CallableMetadata::from_operation(
            unsafe { crate::object::layout::native_descriptor_operation(ptr) }.unwrap(),
        )
        .unwrap();
        let Some(object) = obj_from_bits(instance).as_ptr() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "callable metadata requires a heap receiver",
            );
        };
        return unsafe {
            crate::builtins::attributes::callable_metadata::write(py, object, field, value)
        };
    }
    if flavor == NativeDescriptorFlavor::RootMetadata {
        let _inputs = DescriptorInputs::new(
            py,
            [
                descriptor,
                instance,
                value.unwrap_or(MoltObject::none().bits()),
            ],
        );
        let operation = unsafe { crate::object::layout::native_descriptor_operation(ptr) }.unwrap();
        let Some(field) =
            molt_cpython_abi::api::typeobj::TypeAttributeField::from_operation(operation)
        else {
            return raise_exception::<_>(py, "SystemError", "invalid root metadata operation");
        };
        return unsafe {
            crate::builtins::attributes::type_metadata::write(py, instance, field, value)
        };
    }
    let callback = if value.is_some() {
        unsafe { native_descriptor_setter_bits(ptr) }
    } else {
        unsafe { native_descriptor_deleter_bits(ptr) }
    };
    if obj_from_bits(callback).is_none() {
        return unsafe { missing_accessor(py, ptr, flavor, false) };
    }
    let result = if let Some(value) = value {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance, value, callback]);
        unsafe { call_callable3(py, callback, descriptor, instance, value) }
    } else {
        let _inputs = DescriptorInputs::new(py, [descriptor, instance, callback]);
        unsafe { call_callable2(py, callback, descriptor, instance) }
    };
    crate::call::discard_owned_call_result(py, result);
    MoltObject::none().bits()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeDescriptorMetadata {
    Name,
    Qualname,
    Owner,
    Doc,
}

unsafe fn native_descriptor_qualname_from_ptr(py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let owner = unsafe { native_descriptor_owner_bits(ptr) };
    let owner_qualname = obj_from_bits(owner)
        .as_ptr()
        .and_then(|owner_ptr| {
            string_obj_to_owned(obj_from_bits(unsafe {
                crate::object::layout::class_qualname_bits(owner_ptr)
            }))
        })
        .unwrap_or_else(|| class_name_for_error(owner));
    let name = string_obj_to_owned(obj_from_bits(unsafe { native_descriptor_name_bits(ptr) }))
        .unwrap_or_else(|| "?".to_owned());
    let qualname = format!("{owner_qualname}.{name}");
    let value = alloc_string(py, qualname.as_bytes());
    if value.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(value).bits()
    }
}

/// Return one immutable descriptor metadata field as an owned runtime value.
/// Attribute routing chooses the field; this representation layer never
/// classifies wrapper or numeric attribute names.
pub(crate) unsafe fn native_descriptor_metadata(
    py: &PyToken<'_>,
    descriptor: u64,
    field: NativeDescriptorMetadata,
) -> u64 {
    let Some((ptr, _)) = descriptor_ptr(py, descriptor, "metadata") else {
        return MoltObject::none().bits();
    };
    let value = match field {
        NativeDescriptorMetadata::Name => unsafe { native_descriptor_name_bits(ptr) },
        NativeDescriptorMetadata::Qualname => {
            return unsafe { native_descriptor_qualname_from_ptr(py, ptr) };
        }
        NativeDescriptorMetadata::Owner => unsafe { native_descriptor_owner_bits(ptr) },
        NativeDescriptorMetadata::Doc => unsafe { native_descriptor_doc_bits(ptr) },
    };
    inc_ref_bits(py, value);
    value
}

/// CPython-style stable representation of member and getset descriptors.
pub(crate) unsafe fn native_descriptor_repr(py: &PyToken<'_>, descriptor: u64) -> u64 {
    let Some((ptr, flavor)) = descriptor_ptr(py, descriptor, "__repr__") else {
        return MoltObject::none().bits();
    };
    let (name, owner) = unsafe { descriptor_text(ptr) };
    let label = match flavor {
        NativeDescriptorFlavor::Member | NativeDescriptorFlavor::ManagedSlot => "member",
        NativeDescriptorFlavor::GetSet
        | NativeDescriptorFlavor::InstanceDictionary
        | NativeDescriptorFlavor::CallableMetadata
        | NativeDescriptorFlavor::RootMetadata => "attribute",
    };
    let text = format!("<{label} '{name}' of '{owner}' objects>");
    let value = alloc_string(py, text.as_bytes());
    if value.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(value).bits()
    }
}

/// CPython's descriptor reducer resolves getattr through the active builtins
/// namespace (_PyEval_GetBuiltin), preserving mapping failures and converting
/// only an absent key to AttributeError. The descriptor owns its original type.
pub(crate) unsafe fn native_descriptor_reduce(py: &PyToken<'_>, descriptor: u64) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let Some((ptr, _)) = descriptor_ptr(py, descriptor, "__reduce__") else {
        return MoltObject::none().bits();
    };
    let owner = unsafe { native_descriptor_owner_bits(ptr) };
    let name = unsafe { native_descriptor_name_bits(ptr) };
    let _inputs = DescriptorInputs::new(py, [descriptor, owner, name]);
    let Some(getattr) = crate::builtins::functions::lookup_builtin_name(py, "getattr") else {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        return raise_exception::<_>(py, "AttributeError", "getattr");
    };
    let args_ptr = alloc_tuple(py, &[owner, name]);
    if args_ptr.is_null() {
        dec_ref_bits(py, getattr);
        return MoltObject::none().bits();
    }
    let args = MoltObject::from_ptr(args_ptr).bits();
    let result_ptr = alloc_tuple(py, &[getattr, args]);
    dec_ref_bits(py, args);
    dec_ref_bits(py, getattr);
    if result_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(result_ptr).bits()
    }
}

#[derive(Clone, Copy)]
enum NativeDescriptorOperation {
    New,
    Get,
    Set,
    Delete,
}

fn native_descriptor_method(
    py: &PyToken<'_>,
    operation: NativeDescriptorOperation,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let method = match operation {
        NativeDescriptorOperation::New => "__new__",
        NativeDescriptorOperation::Get => "__get__",
        NativeDescriptorOperation::Set => "__set__",
        NativeDescriptorOperation::Delete => "__delete__",
    };
    let Some(args) = call_vararg_args(py, method, args_bits) else {
        return MoltObject::none().bits();
    };
    let Some((_, keywords)) = call_vararg_kwargs(py, method, kwargs_bits) else {
        return MoltObject::none().bits();
    };
    let Some((&receiver, args)) = args.split_first() else {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("descriptor '{method}' needs an argument"),
        );
    };
    if matches!(operation, NativeDescriptorOperation::New) {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "cannot create '{}' instances",
                class_name_for_error(receiver)
            ),
        );
    }
    if !keywords.is_empty() {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("{method}() takes no keyword arguments"),
        );
    }
    let Some((_, flavor)) = descriptor_ptr(py, receiver, method) else {
        return MoltObject::none().bits();
    };
    let (min, max) = match operation {
        NativeDescriptorOperation::Get => (1, 2),
        NativeDescriptorOperation::Set => (2, 2),
        NativeDescriptorOperation::Delete => (1, 1),
        NativeDescriptorOperation::New => unreachable!(),
    };
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            min.to_string()
        } else {
            format!("{min} or {max}")
        };
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "descriptor '{method}' for '{}' objects expected {expected} arguments, got {}",
                flavor.type_name(),
                args.len(),
            ),
        );
    }
    unsafe {
        match operation {
            NativeDescriptorOperation::Get => {
                let instance = (!obj_from_bits(args[0]).is_none()).then_some(args[0]);
                let owner = args
                    .get(1)
                    .copied()
                    .filter(|value| !obj_from_bits(*value).is_none());
                native_descriptor_get(py, receiver, instance, owner)
            }
            NativeDescriptorOperation::Set => {
                native_descriptor_mutate(py, receiver, args[0], Some(args[1]))
            }
            NativeDescriptorOperation::Delete => {
                native_descriptor_mutate(py, receiver, args[0], None)
            }
            NativeDescriptorOperation::New => unreachable!(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        native_descriptor_method(py, NativeDescriptorOperation::New, args, kwargs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_get(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        native_descriptor_method(py, NativeDescriptorOperation::Get, args, kwargs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_set(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        native_descriptor_method(py, NativeDescriptorOperation::Set, args, kwargs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_delete(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        native_descriptor_method(py, NativeDescriptorOperation::Delete, args, kwargs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_repr(descriptor: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { unsafe { native_descriptor_repr(py, descriptor) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_native_descriptor_reduce(descriptor: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { unsafe { native_descriptor_reduce(py, descriptor) } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TYPE_ID_NATIVE_DESCRIPTOR;

    extern "C" fn return_instance(_descriptor: u64, instance: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, instance);
            instance
        })
    }

    fn string_bits(py: &PyToken<'_>, value: &[u8]) -> u64 {
        let ptr = alloc_string(py, value);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn callback_bits(py: &PyToken<'_>) -> u64 {
        let ptr = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "native_descriptor_test_return_instance",
                return_instance as *const (),
            ),
            2,
        );
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn is_immortal(bits: u64) -> bool {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe {
            (*crate::object::header_from_obj_ptr(ptr)).has_flag(crate::object::HEADER_FLAG_IMMORTAL)
        }
    }

    #[test]
    fn descriptor_control_roundtrips_operation_without_growing_payload_and_rejects_corruption() {
        use crate::object::layout::{
            NATIVE_DESCRIPTOR_PREFIX_WORDS, NATIVE_DESCRIPTOR_REFERENCE_WORDS,
            native_descriptor_control, native_descriptor_operation,
        };
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = builtin_classes(py).int;
            let name = string_bits(py, b"control");
            let callback = callback_bits(py);
            let none = MoltObject::none().bits();
            let mut payload_size = None;
            assert_eq!(NATIVE_DESCRIPTOR_REFERENCE_WORDS, 6);
            assert_eq!(NATIVE_DESCRIPTOR_PREFIX_WORDS, 7);
            assert_eq!(NativeDescriptorFlavor::from_raw(0x101), None);
            for flavor in [
                NativeDescriptorFlavor::Member,
                NativeDescriptorFlavor::GetSet,
                NativeDescriptorFlavor::ManagedSlot,
            ] {
                for operation in [0, 1, u32::MAX] {
                    let descriptor = alloc_native_descriptor(
                        py,
                        NativeDescriptorSpec {
                            flavor,
                            operation,
                            owner,
                            name,
                            doc: none,
                            getter: callback,
                            setter: none,
                            deleter: none,
                        },
                    );
                    assert!(!exception_pending(py));
                    let ptr = obj_from_bits(descriptor).as_ptr().unwrap();
                    unsafe {
                        let size = crate::object::object_payload_size(ptr);
                        assert_eq!(size, *payload_size.get_or_insert(size));
                        assert_eq!(native_descriptor_flavor(ptr), Some(flavor));
                        assert_eq!(native_descriptor_operation(ptr), Some(operation));
                        let control = ptr.cast::<u64>().add(NATIVE_DESCRIPTOR_REFERENCE_WORDS);
                        let valid = native_descriptor_control(flavor, operation);
                        assert_eq!(*control, valid);
                        for corrupt in [
                            valid | (1_u64 << 40),
                            valid | (1_u64 << 63),
                            valid & !0xff,
                            (valid & !0xff) | 0xff,
                        ] {
                            *control = corrupt;
                            assert_eq!(native_descriptor_flavor(ptr), None);
                            assert_eq!(native_descriptor_operation(ptr), None);
                        }
                        *control = valid;
                    }
                    dec_ref_bits(py, descriptor);
                }
            }
            dec_ref_bits(py, callback);
            dec_ref_bits(py, name);
        });
    }

    #[test]
    fn receiver_admission_preserves_pending_channels_and_projection_failures() {
        use crate::builtins::exceptions::alloc_exception;
        use crate::builtins::functions::native_callable::{
            configure_native_callable, native_descriptor_receiver,
        };
        use crate::builtins::methods::object_method_bits;
        use crate::object::class_layout::{is_real_instance, is_real_subtype, try_is_real_subtype};
        use molt_cpython_abi::abi_types::*;
        use molt_cpython_abi::api::errors;
        use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let builtins = builtin_classes(py);
                let owner_name = string_bits(py, b"ReceiverProjectionOwner");
                let owner_ptr = alloc_class_obj(py, owner_name);
                assert!(!owner_ptr.is_null());
                let owner = MoltObject::from_ptr(owner_ptr).bits();
                let name = string_bits(py, b"sample");
                let callback = callback_bits(py);
                let function = obj_from_bits(callback).as_ptr().unwrap();
                assert!(configure_native_callable(
                    py,
                    function,
                    NativeCallableSpec::declared(
                        NativeCallableKind::MethodDescriptor,
                        owner,
                        "sample",
                    )
                ));
                let none = MoltObject::none().bits();
                let make_descriptor = |declaring| {
                    alloc_native_descriptor(
                        py,
                        NativeDescriptorSpec {
                            flavor: NativeDescriptorFlavor::GetSet,
                            operation: 0,
                            owner: declaring,
                            name,
                            doc: none,
                            getter: callback,
                            setter: none,
                            deleter: none,
                        },
                    )
                };
                let valid = make_descriptor(builtins.object);
                let invalid = make_descriptor(owner);
                let valid_ptr = obj_from_bits(valid).as_ptr().unwrap();
                let invalid_ptr = obj_from_bits(invalid).as_ptr().unwrap();
                let callable = object_method_bits(py, "__repr__").unwrap();
                inc_ref_bits(py, callable);

                let mut class: PyTypeObject = std::mem::zeroed();
                class.ob_base.ob_base.ob_refcnt = 1;
                class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
                class.tp_base = &raw mut PyBaseObject_Type;
                class.tp_name = c"NativeReceiverProjection".as_ptr();
                let mut value = PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut class,
                };
                let class_bits = GLOBAL_BRIDGE
                    .molt_value_for_pyobj((&raw mut class).cast())
                    .unwrap();
                let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(&raw mut value).unwrap();
                // A malformed cold type projection must remain distinguishable
                // from an ordinary unrelated receiver. Bypass the public name
                // setter only to exercise this defensive bridge failure.
                crate::object::class_storage::ClassReferenceSlot::Name
                    .replace_borrowed(py, owner_ptr, none);
                assert!(!is_real_subtype(py, class_bits, owner));
                assert!(!is_real_instance(py, bits, owner));
                assert!(!exception_pending(py));
                assert!(errors::PyErr_Occurred().is_null());

                errors::PyErr_SetString(
                    (&raw mut PyExc_KeyError).cast(),
                    c"C admission outer".as_ptr(),
                );
                let c_error = errors::take_current_error().expect("C pending error");
                let c_identity = c_error.value;
                errors::restore_current_error_exact(c_error);
                let runtime_error = MoltObject::from_ptr(alloc_exception(
                    py,
                    "RuntimeError",
                    "runtime admission outer",
                ))
                .bits();
                crate::record_exception(py, obj_from_bits(runtime_error).as_ptr().unwrap());
                assert!(is_real_subtype(py, class_bits, builtins.object));
                assert!(is_real_instance(py, bits, builtins.object));
                assert!(!is_real_subtype(py, class_bits, owner));
                assert!(!is_real_instance(py, bits, owner));
                assert!(validate_receiver(py, valid_ptr, bits));
                assert!(matches!(native_descriptor_receiver(
                    py, obj_from_bits(callable).as_ptr().unwrap(),
                    NativeCallableKind::WrapperDescriptor, None, Some(bits),
                ), Ok(Some(receiver)) if receiver.bits() == bits));
                assert!(
                    matches!(native_descriptor_receiver(
                    py, obj_from_bits(callable).as_ptr().unwrap(),
                    NativeCallableKind::WrapperDescriptor, None, Some(none),
                ), Ok(Some(receiver)) if receiver.bits() == none),
                    "an explicit Python None receiver is not absent"
                );
                assert_eq!(crate::exception_last_bits_noinc(py), Some(runtime_error));
                let c_error = errors::take_current_error().expect("C error survived admission");
                assert_eq!(c_error.value, c_identity);
                assert_eq!(c_error.exc_type, (&raw mut PyExc_KeyError).cast());
                drop(c_error);
                crate::clear_exception(py);
                dec_ref_bits(py, runtime_error);

                assert_eq!(try_is_real_subtype(py, class_bits, owner), Err(()));
                let projection = errors::take_current_error().expect("projection failure retained");
                assert_eq!(projection.exc_type, (&raw mut PyExc_SystemError).cast());
                drop(projection);
                for callable_admission in [true, false] {
                    if callable_admission {
                        assert!(
                            native_descriptor_receiver(
                                py,
                                function,
                                NativeCallableKind::MethodDescriptor,
                                None,
                                Some(bits),
                            )
                            .is_err()
                        );
                    } else {
                        assert!(!validate_receiver(py, invalid_ptr, bits));
                    }
                    let pending = crate::exception_last_bits_noinc(py).expect("admission failure");
                    assert!(
                        crate::builtins::exceptions::exception_matches_builtin_name(
                            py,
                            pending,
                            "SystemError",
                        ),
                        "a projection failure must not become a receiver TypeError"
                    );
                    crate::clear_exception(py);
                    assert!(errors::PyErr_Occurred().is_null());
                }
                crate::object::class_storage::ClassReferenceSlot::Name
                    .replace_borrowed(py, owner_ptr, owner_name);
                for owned in [
                    class_bits, bits, callable, invalid, valid, callback, name, owner, owner_name,
                ] {
                    dec_ref_bits(py, owned);
                }
                assert_eq!(value.ob_refcnt, 1);
                assert_eq!(class.ob_base.ob_base.ob_refcnt, 1);
            }
        });
    }

    #[test]
    fn fixed_descriptor_owns_class_metadata_and_callback_protocol() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _provider = crate::test_support::NativeProviderTestNamespace::new(py, "builtins");
            let owner = builtin_classes(py).int;
            let name = string_bits(py, b"sample");
            let callback = callback_bits(py);
            let none = MoltObject::none().bits();
            let descriptor = alloc_native_descriptor(
                py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::GetSet,
                    operation: 0,
                    owner,
                    name,
                    doc: none,
                    getter: callback,
                    setter: none,
                    deleter: none,
                },
            );
            assert!(!exception_pending(py));
            assert_eq!(type_of_bits(py, descriptor), getset_descriptor_class(py));
            let ptr = obj_from_bits(descriptor).as_ptr().unwrap();
            assert_eq!(unsafe { object_type_id(ptr) }, TYPE_ID_NATIVE_DESCRIPTOR);
            assert_eq!(unsafe { native_descriptor_name_bits(ptr) }, name);
            let metadata_owner = unsafe {
                native_descriptor_metadata(py, descriptor, NativeDescriptorMetadata::Owner)
            };
            assert_eq!(metadata_owner, owner);
            dec_ref_bits(py, metadata_owner);
            let qualname = unsafe {
                native_descriptor_metadata(py, descriptor, NativeDescriptorMetadata::Qualname)
            };
            assert_eq!(
                string_obj_to_owned(obj_from_bits(qualname)).as_deref(),
                Some("int.sample"),
            );
            dec_ref_bits(py, qualname);
            let returned = unsafe {
                native_descriptor_get(py, descriptor, Some(MoltObject::from_int(17).bits()), None)
            };
            assert_eq!(returned, MoltObject::from_int(17).bits());
            dec_ref_bits(py, returned);
            let class_value = unsafe { native_descriptor_get(py, descriptor, None, Some(owner)) };
            assert_eq!(class_value, descriptor);
            dec_ref_bits(py, class_value);
            let repr = unsafe { native_descriptor_repr(py, descriptor) };
            assert_eq!(
                string_obj_to_owned(obj_from_bits(repr)).as_deref(),
                Some("<attribute 'sample' of 'int' objects>"),
            );
            dec_ref_bits(py, repr);
            let reduced = unsafe { native_descriptor_reduce(py, descriptor) };
            let reduced_ptr = obj_from_bits(reduced).as_ptr().expect("reduce tuple");
            let reduced_items = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(reduced_ptr, |items| {
                    items.to_vec()
                })
            }
            .expect("immutable reduce tuple");
            assert_eq!(reduced_items.len(), 2);
            let getattr = crate::builtins::functions::lookup_builtin_name(py, "getattr")
                .expect("builtin getattr");
            assert_eq!(reduced_items[0], getattr);
            dec_ref_bits(py, getattr);
            let reduce_args_ptr = obj_from_bits(reduced_items[1])
                .as_ptr()
                .expect("reduce arguments tuple");
            let reduce_args = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(reduce_args_ptr, |items| {
                    items.to_vec()
                })
            }
            .expect("immutable reduce arguments tuple");
            assert_eq!(reduce_args, [owner, name]);
            dec_ref_bits(py, reduced);

            let globals = MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[])).bits();
            let captured_ptr = crate::alloc_dict_with_pairs(py, &[]);
            let captured = MoltObject::from_ptr(captured_ptr).bits();
            let getattr_name = string_bits(py, b"getattr");
            unsafe {
                crate::dict_set_in_place(
                    py,
                    captured_ptr,
                    getattr_name,
                    MoltObject::from_int(37).bits(),
                );
            }
            inc_ref_bits(py, globals);
            inc_ref_bits(py, captured);
            crate::builtins::frames::frame_stack_push_owned(py, 0, globals, captured, 0);
            let reduced = unsafe { native_descriptor_reduce(py, descriptor) };
            assert!(!exception_pending(py));
            let items = unsafe {
                crate::object::seq_access::with_immutable_tuple_slice(
                    obj_from_bits(reduced).as_ptr().unwrap(),
                    |items| items.to_vec(),
                )
            }
            .unwrap();
            assert_eq!(
                items[0],
                MoltObject::from_int(37).bits(),
                "reduction captures the active binding without checking callability"
            );
            dec_ref_bits(py, reduced);
            let declared_owner = unsafe {
                native_descriptor_metadata(py, descriptor, NativeDescriptorMetadata::Owner)
            };
            assert_eq!(
                declared_owner, owner,
                "__objclass__ remains the retained descriptor owner"
            );
            dec_ref_bits(py, declared_owner);
            unsafe {
                crate::dict_del_in_place(py, captured_ptr, getattr_name);
            }
            let missing = unsafe { native_descriptor_reduce(py, descriptor) };
            assert!(exception_pending(py));
            let error = crate::builtins::exceptions::exception_last_bits_noinc(py).unwrap();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "AttributeError"
            ));
            clear_exception(py);
            dec_ref_bits(py, missing);
            crate::builtins::frames::frame_stack_pop(py);
            inc_ref_bits(py, globals);
            crate::builtins::frames::frame_stack_push_owned(py, 0, globals, none, 0);
            let invalid = unsafe { native_descriptor_reduce(py, descriptor) };
            assert!(exception_pending(py));
            let error = crate::builtins::exceptions::exception_last_bits_noinc(py).unwrap();
            assert!(
                crate::builtins::exceptions::exception_matches_builtin_name(py, error, "TypeError"),
                "mapping error must not become a descriptor SystemError/AttributeError"
            );
            clear_exception(py);
            dec_ref_bits(py, invalid);
            crate::builtins::frames::frame_stack_pop(py);
            for bits in [globals, captured, getattr_name] {
                dec_ref_bits(py, bits);
            }

            let readonly = unsafe {
                native_descriptor_mutate(
                    py,
                    descriptor,
                    MoltObject::from_int(17).bits(),
                    Some(none),
                )
            };
            dec_ref_bits(py, readonly);
            assert!(exception_pending(py));
            clear_exception(py);

            dec_ref_bits(py, descriptor);
            dec_ref_bits(py, callback);
            dec_ref_bits(py, name);
        });
    }

    #[test]
    fn descriptor_protocol_rejects_wrong_receivers_and_corrupt_class_edges() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = builtin_classes(py).int;
            let name = string_bits(py, b"sample");
            let callback = callback_bits(py);
            let none = MoltObject::none().bits();
            let descriptor = alloc_native_descriptor(
                py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::GetSet,
                    operation: 0,
                    owner,
                    name,
                    doc: none,
                    getter: callback,
                    setter: none,
                    deleter: none,
                },
            );
            let wrong_receiver = string_bits(py, b"not-an-int");
            let result =
                unsafe { native_descriptor_get(py, descriptor, Some(wrong_receiver), None) };
            dec_ref_bits(py, result);
            assert!(exception_pending(py));
            clear_exception(py);

            let corrupt_ptr = crate::object::builders::alloc_native_descriptor_obj(
                py,
                member_descriptor_class(py),
                NativeDescriptorFlavor::GetSet,
                0,
                owner,
                name,
                none,
                callback,
                none,
                none,
            );
            assert!(!corrupt_ptr.is_null());
            let corrupt = MoltObject::from_ptr(corrupt_ptr).bits();
            let result = unsafe {
                native_descriptor_get(py, corrupt, Some(MoltObject::from_int(1).bits()), None)
            };
            dec_ref_bits(py, result);
            assert!(exception_pending(py));
            clear_exception(py);

            dec_ref_bits(py, corrupt);
            dec_ref_bits(py, wrong_receiver);
            dec_ref_bits(py, descriptor);
            dec_ref_bits(py, callback);
            dec_ref_bits(py, name);
        });
    }

    #[test]
    fn descriptor_flavors_have_sealed_identity_classes_and_no_public_constructor() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let member = member_descriptor_class(py);
            let getset = getset_descriptor_class(py);
            assert_ne!(member, getset);
            let name = string_bits(py, b"identity");
            let none = MoltObject::none().bits();
            for (flavor, expected_class) in [
                (NativeDescriptorFlavor::Member, member),
                (NativeDescriptorFlavor::GetSet, getset),
            ] {
                let descriptor = alloc_native_descriptor(
                    py,
                    NativeDescriptorSpec {
                        flavor,
                        operation: 0,
                        owner: builtin_classes(py).int,
                        name,
                        doc: none,
                        getter: none,
                        setter: none,
                        deleter: none,
                    },
                );
                assert_eq!(type_of_bits(py, descriptor), expected_class);
                dec_ref_bits(py, descriptor);
            }
            dec_ref_bits(py, name);
            for class in [member, getset] {
                let ptr = obj_from_bits(class).as_ptr().expect("descriptor class");
                assert!(unsafe { crate::object::class_is_not_base(py, ptr) });
                assert!(unsafe { crate::object::class_is_immutable(py, ptr) });

                let args_ptr = alloc_tuple(py, &[class]);
                let kwargs_ptr = alloc_dict_with_pairs(py, &[]);
                assert!(!args_ptr.is_null());
                assert!(!kwargs_ptr.is_null());
                let result = native_descriptor_method(
                    py,
                    NativeDescriptorOperation::New,
                    MoltObject::from_ptr(args_ptr).bits(),
                    MoltObject::from_ptr(kwargs_ptr).bits(),
                );
                dec_ref_bits(py, result);
                assert!(exception_pending(py));
                clear_exception(py);
                dec_ref_bits(py, MoltObject::from_ptr(args_ptr).bits());
                dec_ref_bits(py, MoltObject::from_ptr(kwargs_ptr).bits());
            }
        });
    }

    #[test]
    fn descriptor_gc_projection_retains_class_and_all_payload_edges() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = getset_descriptor_class(py);
            let owner = builtin_classes(py).int;
            let name = string_bits(py, b"sample");
            let doc = string_bits(py, b"documentation");
            let getter = callback_bits(py);
            let setter = callback_bits(py);
            let deleter = callback_bits(py);
            let owned = [class, owner, name, doc, getter, setter, deleter];
            let before = owned.map(refcount);
            let immortal = owned.map(is_immortal);

            let descriptor = alloc_native_descriptor(
                py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::GetSet,
                    operation: u32::MAX,
                    owner,
                    name,
                    doc,
                    getter,
                    setter,
                    deleter,
                },
            );
            assert!(!exception_pending(py));
            for ((bits, count), immortal) in owned.into_iter().zip(before).zip(immortal) {
                if immortal {
                    assert_eq!(count, molt_codegen_abi::IMMORTAL_REFCOUNT);
                    assert_eq!(
                        refcount(bits),
                        count,
                        "immortal owned edges retain their sentinel refcount",
                    );
                } else {
                    assert_eq!(refcount(bits), count + 1);
                }
            }

            let ptr = obj_from_bits(descriptor)
                .as_ptr()
                .expect("native descriptor");
            let mut visited = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_edges(py, ptr, &mut |child| {
                    visited.push(MoltObject::from_ptr(child).bits());
                });
            }
            assert_eq!(visited, owned);

            dec_ref_bits(py, descriptor);
            for (bits, count) in owned.into_iter().zip(before) {
                assert_eq!(refcount(bits), count);
            }
            for bits in [name, doc, getter, setter, deleter] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn descriptor_construction_rejects_non_callable_callbacks() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = string_bits(py, b"sample");
            let none = MoltObject::none().bits();
            let descriptor = alloc_native_descriptor(
                py,
                NativeDescriptorSpec {
                    flavor: NativeDescriptorFlavor::Member,
                    operation: 0,
                    owner: builtin_classes(py).int,
                    name,
                    doc: none,
                    getter: MoltObject::from_int(7).bits(),
                    setter: none,
                    deleter: none,
                },
            );
            dec_ref_bits(py, descriptor);
            assert!(exception_pending(py));
            clear_exception(py);
            dec_ref_bits(py, name);
        });
    }
}
