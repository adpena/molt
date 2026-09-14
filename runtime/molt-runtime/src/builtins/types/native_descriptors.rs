//! Shared builtin member/getset descriptor representation and protocol.
//!
//! Descriptor payloads retain ordinary runtime callable objects. The payload
//! therefore has one portable representation on native and WASM: no host
//! function pointer, callback registry, or attribute-family classifier is
//! embedded in the object. Callbacks receive the descriptor itself as typed
//! context before the ordinary receiver arguments.

use super::*;
pub(crate) use crate::object::layout::{
    NativeDescriptorFlavor, native_descriptor_deleter_bits, native_descriptor_doc_bits,
    native_descriptor_flavor, native_descriptor_getter_bits, native_descriptor_name_bits,
    native_descriptor_owner_bits, native_descriptor_setter_bits,
};
use crate::{call_callable3, isinstance_bits};

#[derive(Clone, Copy)]
pub(crate) struct NativeDescriptorSpec {
    pub(crate) flavor: NativeDescriptorFlavor,
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
            Self::Member => "member_descriptor",
            Self::GetSet => "getset_descriptor",
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
        &types_state(py).native_descriptor_new_fn,
        molt_native_descriptor_new as *const () as usize as u64,
    );
    let get = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        &types_state(py).native_descriptor_get_fn,
        molt_native_descriptor_get as *const () as usize as u64,
    );
    let set = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        &types_state(py).native_descriptor_set_fn,
        molt_native_descriptor_set as *const () as usize as u64,
    );
    let delete = crate::builtins::methods::builtin_variadic_func_bits(
        py,
        &types_state(py).native_descriptor_delete_fn,
        molt_native_descriptor_delete as *const () as usize as u64,
    );
    let repr = crate::builtins::methods::builtin_func_bits(
        py,
        &types_state(py).native_descriptor_repr_fn,
        molt_native_descriptor_repr as *const () as usize as u64,
        1,
    );
    let reduce = crate::builtins::methods::builtin_func_bits(
        py,
        &types_state(py).native_descriptor_reduce_fn,
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
        || !unsafe { crate::object::class_set_not_base(py, class_ptr) }
        || unsafe { crate::object::class_finish_definition(py, class_ptr) }.is_err()
        || !unsafe { crate::object::class_set_immutable(py, class_ptr) }
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
        NativeDescriptorFlavor::Member => member_descriptor_class(py),
        NativeDescriptorFlavor::GetSet => getset_descriptor_class(py),
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
    if isinstance_bits(py, instance, owner) {
        return true;
    }
    if exception_pending(py) {
        return false;
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
    if !unsafe { validate_receiver(py, ptr, instance) } {
        return MoltObject::none().bits();
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
    if !unsafe { validate_receiver(py, ptr, instance) } {
        return MoltObject::none().bits();
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
        NativeDescriptorFlavor::Member => "member",
        NativeDescriptorFlavor::GetSet => "attribute",
    };
    let text = format!("<{label} '{name}' of '{owner}' objects>");
    let value = alloc_string(py, text.as_bytes());
    if value.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(value).bits()
    }
}

/// Reconstruct a descriptor through the canonical builtin `getattr(owner, name)`
/// callable. The returned two-tuple is directly consumable by pickle/copyreg.
pub(crate) unsafe fn native_descriptor_reduce(py: &PyToken<'_>, descriptor: u64) -> u64 {
    let Some((ptr, _)) = descriptor_ptr(py, descriptor, "__reduce__") else {
        return MoltObject::none().bits();
    };
    let owner = unsafe { native_descriptor_owner_bits(ptr) };
    let name = unsafe { native_descriptor_name_bits(ptr) };
    let _inputs = DescriptorInputs::new(py, [descriptor, owner, name]);
    let Some(getattr) = crate::builtins::functions::python_builtin_function_bits(py, "getattr")
    else {
        return raise_exception::<_>(
            py,
            "SystemError",
            "builtin getattr is unavailable for native descriptor reduction",
        );
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
    fn fixed_descriptor_owns_class_metadata_and_callback_protocol() {
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
            let getattr = crate::builtins::functions::python_builtin_function_bits(py, "getattr")
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
