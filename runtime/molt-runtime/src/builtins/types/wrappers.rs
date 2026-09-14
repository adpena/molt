//! Construction and descriptor protocol for native-prefix wrapper objects.
//!
//! Compiler intrinsics, ordinary type calls, explicit dunder calls and accessor
//! cloning share this authority. Physical storage comes from `WrapperKind`;
//! subclasses keep ordinary Python method lookup and construction semantics.

use super::*;
use crate::builtins::attr::clear_attribute_error_if_pending;
use crate::object::layout::{
    WrapperKind, class_qualname_bits, classmethod_func_bits, classmethod_replace_func_bits,
    property_doc_bits, property_getter_doc, property_name_bits, property_replace_del_bits,
    property_replace_doc_bits, property_replace_get_bits, property_replace_name_bits,
    property_replace_set_bits, property_set_getter_doc, staticmethod_func_bits,
    staticmethod_replace_func_bits,
};

impl WrapperKind {
    fn name(self) -> &'static str {
        match self {
            Self::Classmethod => "classmethod",
            Self::Staticmethod => "staticmethod",
            Self::Property => "property",
        }
    }

    fn class(self, py: &PyToken<'_>) -> u64 {
        let classes = builtin_classes(py);
        match self {
            Self::Classmethod => classes.classmethod,
            Self::Staticmethod => classes.staticmethod,
            Self::Property => classes.property,
        }
    }

    pub(crate) fn exact_class(py: &PyToken<'_>, class: u64) -> Option<Self> {
        let classes = builtin_classes(py);
        if class == classes.classmethod {
            Some(Self::Classmethod)
        } else if class == classes.staticmethod {
            Some(Self::Staticmethod)
        } else if class == classes.property {
            Some(Self::Property)
        } else {
            None
        }
    }
}

/// Reinitialization and metadata callbacks may release the original owner of
/// any operand. Pin the fixed-size input set without allocating a scratch Vec.
struct WrapperInputs<'a, 'py, const N: usize> {
    py: &'a PyToken<'py>,
    bits: [u64; N],
}

impl<'a, 'py, const N: usize> WrapperInputs<'a, 'py, N> {
    fn new(py: &'a PyToken<'py>, bits: [u64; N]) -> Self {
        for value in bits {
            inc_ref_bits(py, value);
        }
        Self { py, bits }
    }
}

impl<const N: usize> Drop for WrapperInputs<'_, '_, N> {
    fn drop(&mut self) {
        for value in self.bits {
            dec_ref_bits(self.py, value);
        }
    }
}

fn wrapper_receiver(
    py: &PyToken<'_>,
    kind: WrapperKind,
    bits: u64,
    method: &str,
) -> Option<*mut u8> {
    let ptr = obj_from_bits(bits).as_ptr();
    if let Some(ptr) = ptr
        && unsafe { object_type_id(ptr) } == kind.type_id()
    {
        return Some(ptr);
    }
    let message = format!(
        "descriptor '{}' requires a '{}' object but received a '{}'",
        method,
        kind.name(),
        type_name(py, obj_from_bits(bits)),
    );
    raise_exception::<Option<*mut u8>>(py, "TypeError", &message)
}

fn wrapper_allocate(py: &PyToken<'_>, kind: WrapperKind, class: u64) -> u64 {
    let Some(ptr) = obj_from_bits(class)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TYPE })
    else {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "{}.__new__(X): X is not a type object ({})",
                kind.name(),
                type_name(py, obj_from_bits(class)),
            ),
        );
    };
    if !issubclass_bits(class, kind.class(py)) {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "{}.__new__({}): {} is not a subtype of {}",
                kind.name(),
                class_name_for_error(class),
                class_name_for_error(class),
                kind.name(),
            ),
        );
    }
    unsafe { alloc_instance_for_class(py, ptr) }
}

fn property_arguments(
    py: &PyToken<'_>,
    args: &[u64],
    keywords: &[(String, u64)],
) -> Option<[u64; 4]> {
    if args.len() + keywords.len() > 4 {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!(
                "property() takes at most 4 arguments ({} given)",
                args.len() + keywords.len(),
            ),
        );
    }
    const NAMES: [&str; 4] = ["fget", "fset", "fdel", "doc"];
    let mut values = [MoltObject::none().bits(); 4];
    values[..args.len()].copy_from_slice(args);
    for (name, value) in keywords {
        let Some(index) = NAMES.iter().position(|known| *known == name) else {
            let message = if crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                format!("property() got an unexpected keyword argument '{name}'")
            } else {
                format!("'{name}' is an invalid keyword argument for property()")
            };
            return raise_exception::<_>(py, "TypeError", &message);
        };
        if index < args.len() {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "argument for property() given by name ('{}') and position ({})",
                    name,
                    index + 1,
                ),
            );
        }
        values[index] = *value;
    }
    Some(values)
}

fn property_initialize(py: &PyToken<'_>, self_bits: u64, values: [u64; 4]) -> u64 {
    let [get, set, delete, doc] = values;
    let _inputs = WrapperInputs::new(py, [self_bits, get, set, delete, doc]);
    let ptr = obj_from_bits(self_bits)
        .as_ptr()
        .expect("validated property receiver");
    unsafe {
        // Publish in CPython order: displaced edges can finalize and re-enter.
        // Later assignments observe, rather than roll back, those callbacks.
        if !property_replace_get_bits(py, ptr, get)
            || !property_replace_set_bits(py, ptr, set)
            || !property_replace_del_bits(py, ptr, delete)
            || !property_replace_doc_bits(py, ptr, MoltObject::none().bits())
            || !property_replace_name_bits(py, ptr, missing_bits(py))
        {
            return wrapper_storage_failure(py);
        }
        property_set_getter_doc(ptr, false);
        let effective_doc = if !obj_from_bits(doc).is_none() {
            inc_ref_bits(py, doc);
            doc
        } else if !obj_from_bits(get).is_none() {
            let Some(name) = attr_name_bits_from_bytes(py, b"__doc__") else {
                return MoltObject::none().bits();
            };
            let value = molt_getattr_builtin(get, name, MoltObject::none().bits());
            dec_ref_bits(py, name);
            if exception_pending(py) {
                dec_ref_bits(py, value);
                return MoltObject::none().bits();
            }
            if !obj_from_bits(value).is_none() {
                property_set_getter_doc(ptr, true);
            }
            value
        } else {
            MoltObject::none().bits()
        };
        let exact = type_of_bits(py, self_bits) == builtin_classes(py).property;
        if exact {
            let replaced = property_replace_doc_bits(py, ptr, effective_doc);
            dec_ref_bits(py, effective_doc);
            if !replaced {
                return wrapper_storage_failure(py);
            }
        } else {
            let Some(name) = attr_name_bits_from_bytes(py, b"__doc__") else {
                dec_ref_bits(py, effective_doc);
                return MoltObject::none().bits();
            };
            let result = crate::molt_set_attr_name(self_bits, name, effective_doc);
            dec_ref_bits(py, name);
            dec_ref_bits(py, effective_doc);
            dec_ref_bits(py, result);
            // Consult live getter_doc after __setattr__, which may reinitialize.
            if exception_pending(py) && !property_getter_doc(ptr) {
                clear_attribute_error_if_pending(py);
            }
        }
    }
    MoltObject::none().bits()
}

fn wrapper_initialize(
    py: &PyToken<'_>,
    kind: WrapperKind,
    self_bits: u64,
    args: &[u64],
    keywords: &[(String, u64)],
) -> u64 {
    let Some(ptr) = wrapper_receiver(py, kind, self_bits, "__init__") else {
        return MoltObject::none().bits();
    };
    if kind == WrapperKind::Property {
        let Some(values) = property_arguments(py, args, keywords) else {
            return MoltObject::none().bits();
        };
        return property_initialize(py, self_bits, values);
    }
    if !keywords.is_empty() {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("{}() takes no keyword arguments", kind.name()),
        );
    }
    if args.len() != 1 {
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("{} expected 1 argument, got {}", kind.name(), args.len()),
        );
    }
    let target = args[0];
    let _inputs = WrapperInputs::new(py, [self_bits, target]);
    let replaced = unsafe {
        match kind {
            WrapperKind::Staticmethod => staticmethod_replace_func_bits(py, ptr, target),
            WrapperKind::Classmethod => classmethod_replace_func_bits(py, ptr, target),
            WrapperKind::Property => unreachable!(),
        }
    };
    if !replaced {
        return wrapper_storage_failure(py);
    }
    if !unsafe { crate::builtins::attributes::wrapper_copy_metadata(py, self_bits, target) } {
        return MoltObject::none().bits();
    }
    MoltObject::none().bits()
}

fn wrapper_storage_failure(py: &PyToken<'_>) -> u64 {
    if !exception_pending(py) {
        return raise_exception::<_>(
            py,
            "SystemError",
            "invalid native wrapper storage or raw-zero reference",
        );
    }
    MoltObject::none().bits()
}

/// Fast exact-type construction is only a transport optimization. The exposed
/// initializer and compiler intrinsic use exactly the same semantic operation.
pub(crate) fn wrapper_construct(
    py: &PyToken<'_>,
    kind: WrapperKind,
    args: &[u64],
    keywords: &[(String, u64)],
) -> u64 {
    let instance = wrapper_allocate(py, kind, kind.class(py));
    if exception_pending(py) || obj_from_bits(instance).as_ptr().is_none() {
        return instance;
    }
    let result = wrapper_initialize(py, kind, instance, args, keywords);
    dec_ref_bits(py, result);
    if exception_pending(py) {
        dec_ref_bits(py, instance);
        MoltObject::none().bits()
    } else {
        instance
    }
}

pub(crate) fn try_construct_exact_wrapper(
    py: &PyToken<'_>,
    class: u64,
    args: &[u64],
    names: &[u64],
    values: &[u64],
) -> Option<u64> {
    let kind = WrapperKind::exact_class(py, class)?;
    let mut keywords = Vec::with_capacity(names.len());
    for (&name, &value) in names.iter().zip(values) {
        let Some(name) = string_obj_to_owned(obj_from_bits(name)) else {
            return Some(raise_exception::<_>(
                py,
                "TypeError",
                "keywords must be strings",
            ));
        };
        keywords.push((name, value));
    }
    Some(wrapper_construct(py, kind, args, &keywords))
}

/// Special-method fast paths are admitted by the resolved method, not merely
/// the physical heap kind. A subclass override always returns to normal dispatch.
pub(crate) unsafe fn wrapper_uses_default_method(
    py: &PyToken<'_>,
    ptr: *mut u8,
    name: &[u8],
    symbol: u64,
) -> bool {
    unsafe {
        let Some(kind) = WrapperKind::from_type_id(object_type_id(ptr)) else {
            return false;
        };
        let class = type_of_bits(py, MoltObject::from_ptr(ptr).bits());
        if class == kind.class(py) {
            return true;
        }
        let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
            return false;
        };
        let Some(name) = attr_name_bits_from_bytes(py, name) else {
            return false;
        };
        let raw = crate::builtins::attr::class_attr_lookup_raw_mro(py, class_ptr, name);
        dec_ref_bits(py, name);
        !exception_pending(py)
            && crate::call::type_policy::callable_matches_runtime_symbol(raw, symbol)
    }
}

/// Builtin __get__ only. Callers must resolve subclass overrides before using
/// this physical-prefix implementation. An absent instance differs from tagged
/// None for general descriptors, but property itself treats either as class access.
pub(crate) unsafe fn wrapper_get(
    py: &PyToken<'_>,
    wrapper: u64,
    instance: Option<u64>,
    owner: Option<u64>,
) -> u64 {
    unsafe {
        let ptr = obj_from_bits(wrapper)
            .as_ptr()
            .expect("validated wrapper receiver");
        let kind = WrapperKind::from_type_id(object_type_id(ptr)).expect("wrapper prefix");
        let owner = owner.unwrap_or_else(|| {
            type_of_bits(py, instance.unwrap_or_else(|| MoltObject::none().bits()))
        });
        let _inputs = WrapperInputs::new(
            py,
            [
                wrapper,
                instance.unwrap_or_else(|| MoltObject::none().bits()),
                owner,
            ],
        );
        match kind {
            WrapperKind::Property => {
                if instance.is_none_or(|value| obj_from_bits(value).is_none()) {
                    inc_ref_bits(py, wrapper);
                    return wrapper;
                }
                let getter = property_get_bits(ptr);
                if obj_from_bits(getter).is_none() {
                    return property_missing_accessor(py, ptr, instance.unwrap(), "getter");
                }
                let _getter = WrapperInputs::new(py, [getter]);
                call_callable1(py, getter, instance.unwrap())
            }
            WrapperKind::Classmethod | WrapperKind::Staticmethod => {
                let target = match kind {
                    WrapperKind::Classmethod => classmethod_func_bits(ptr),
                    _ => staticmethod_func_bits(ptr),
                };
                if crate::builtins::methods::is_missing_bits(py, target) {
                    return raise_exception::<_>(
                        py,
                        "RuntimeError",
                        &format!("uninitialized {} object", kind.name()),
                    );
                }
                if kind == WrapperKind::Staticmethod {
                    inc_ref_bits(py, target);
                    return target;
                }
                let _target = WrapperInputs::new(py, [target]);
                if !crate::object::ops_sys::runtime_target_at_least(py, 3, 13) {
                    let descriptor =
                        crate::builtins::attr::has_special_method(py, target, b"__get__");
                    if exception_pending(py) {
                        return MoltObject::none().bits();
                    }
                    if descriptor {
                        return crate::builtins::attr::descriptor_bind(
                            py,
                            target,
                            Some(owner),
                            Some(owner),
                        )
                        .unwrap_or_else(|| MoltObject::none().bits());
                    }
                }
                crate::molt_bound_method_new(target, owner)
            }
        }
    }
}

unsafe fn property_missing_accessor(
    py: &PyToken<'_>,
    ptr: *mut u8,
    instance: u64,
    accessor: &str,
) -> u64 {
    unsafe {
        let name = crate::builtins::attributes::property_name_value(py, ptr);
        if exception_pending(py) {
            if let Some(name) = name {
                dec_ref_bits(py, name);
            }
            return MoltObject::none().bits();
        }
        let class = type_of_bits(py, instance);
        let class_ptr = obj_from_bits(class).as_ptr().expect("instance type");
        let qualname = class_qualname_bits(class_ptr);
        let _class = WrapperInputs::new(py, [class, qualname]);
        let qualname_repr = molt_repr_from_obj(qualname);
        let qualname_text = string_obj_to_owned(obj_from_bits(qualname_repr));
        dec_ref_bits(py, qualname_repr);
        if exception_pending(py) {
            if let Some(name) = name {
                dec_ref_bits(py, name);
            }
            return MoltObject::none().bits();
        }
        let name_text = if let Some(name) = name {
            let repr = molt_repr_from_obj(name);
            dec_ref_bits(py, name);
            let text = string_obj_to_owned(obj_from_bits(repr));
            dec_ref_bits(py, repr);
            text
        } else {
            None
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let message = match (name_text, qualname_text) {
            (Some(name), Some(class)) => {
                format!("property {name} of {class} object has no {accessor}")
            }
            (_, Some(class)) => format!("property of {class} object has no {accessor}"),
            _ => format!("property has no {accessor}"),
        };
        raise_exception::<_>(py, "AttributeError", &message)
    }
}

pub(crate) unsafe fn property_mutate(
    py: &PyToken<'_>,
    wrapper: u64,
    instance: u64,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let ptr = obj_from_bits(wrapper)
            .as_ptr()
            .expect("validated property receiver");
        let _inputs = WrapperInputs::new(
            py,
            [
                wrapper,
                instance,
                value.unwrap_or_else(|| MoltObject::none().bits()),
            ],
        );
        let accessor = if value.is_some() {
            property_set_bits(ptr)
        } else {
            property_del_bits(ptr)
        };
        if obj_from_bits(accessor).is_none() {
            return property_missing_accessor(
                py,
                ptr,
                instance,
                if value.is_some() { "setter" } else { "deleter" },
            );
        }
        let _accessor = WrapperInputs::new(py, [accessor]);
        let result = match value {
            Some(value) => call_callable2(py, accessor, instance, value),
            None => call_callable1(py, accessor, instance),
        };
        dec_ref_bits(py, result);
        MoltObject::none().bits()
    }
}

fn property_copy(py: &PyToken<'_>, wrapper: u64, replacement: u64, index: usize) -> u64 {
    let Some(ptr) = wrapper_receiver(
        py,
        WrapperKind::Property,
        wrapper,
        ["getter", "setter", "deleter"][index],
    ) else {
        return MoltObject::none().bits();
    };
    unsafe {
        let mut accessors = [
            property_get_bits(ptr),
            property_set_bits(ptr),
            property_del_bits(ptr),
        ];
        if !obj_from_bits(replacement).is_none() {
            accessors[index] = replacement;
        }
        let doc = if property_getter_doc(ptr) && !obj_from_bits(accessors[0]).is_none() {
            MoltObject::none().bits()
        } else {
            property_doc_bits(ptr)
        };
        let class = type_of_bits(py, wrapper);
        let _inputs = WrapperInputs::new(
            py,
            [
                wrapper,
                class,
                accessors[0],
                accessors[1],
                accessors[2],
                doc,
            ],
        );
        let result = call_with_kwargs(
            py,
            class,
            &[accessors[0], accessors[1], accessors[2], doc],
            MoltObject::none().bits(),
        );
        if !exception_pending(py)
            && let Some(new_ptr) = obj_from_bits(result).as_ptr()
            && object_type_id(new_ptr) == WrapperKind::Property.type_id()
        {
            // The constructor may reinitialize the source. Copy its current
            // name only after that callback, just as CPython property_copy does.
            if !property_replace_name_bits(py, new_ptr, property_name_bits(ptr)) {
                dec_ref_bits(py, result);
                return wrapper_storage_failure(py);
            }
        }
        result
    }
}

#[derive(Clone, Copy)]
enum WrapperOperation {
    New,
    Init,
    Get,
    Call,
    Set,
    Delete,
    SetName,
}

fn wrapper_method(
    py: &PyToken<'_>,
    kind: WrapperKind,
    operation: WrapperOperation,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    let method = match operation {
        WrapperOperation::New => "__new__",
        WrapperOperation::Init => "__init__",
        WrapperOperation::Get => "__get__",
        WrapperOperation::Call => "__call__",
        WrapperOperation::Set => "__set__",
        WrapperOperation::Delete => "__delete__",
        WrapperOperation::SetName => "__set_name__",
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
            &format!(
                "descriptor '{method}' of '{}' object needs an argument",
                kind.name()
            ),
        );
    };
    if matches!(operation, WrapperOperation::New) {
        return wrapper_allocate(py, kind, receiver);
    }
    let Some(ptr) = wrapper_receiver(py, kind, receiver, method) else {
        return MoltObject::none().bits();
    };
    if matches!(operation, WrapperOperation::Init) {
        return wrapper_initialize(py, kind, receiver, args, &keywords);
    }
    if matches!(operation, WrapperOperation::Call) {
        let _receiver = WrapperInputs::new(py, [receiver]);
        let target = unsafe { staticmethod_func_bits(ptr) };
        if crate::builtins::methods::is_missing_bits(py, target) {
            // CPython 3.12 dereferences NULL here. Molt's verified domain
            // excludes that crash; never call the missing sentinel as code.
            return raise_exception::<_>(py, "RuntimeError", "uninitialized staticmethod object");
        }
        let _target = WrapperInputs::new(py, [target]);
        return call_with_kwargs(py, target, args, kwargs_bits);
    }
    if !keywords.is_empty() {
        let prefix = if matches!(operation, WrapperOperation::SetName) {
            ""
        } else {
            "wrapper "
        };
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("{prefix}{method}() takes no keyword arguments"),
        );
    }
    let (min, max) = match operation {
        WrapperOperation::Get => (1, 2),
        WrapperOperation::Set | WrapperOperation::SetName => (2, 2),
        WrapperOperation::Delete => (1, 1),
        _ => unreachable!(),
    };
    if args.len() < min || args.len() > max {
        let message = if matches!(operation, WrapperOperation::SetName) {
            format!(
                "__set_name__() takes 2 positional arguments but {} were given",
                args.len()
            )
        } else if min != max {
            let name = if crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
                method
            } else {
                ""
            };
            let (bound, count) = if args.len() < min {
                ("least", min)
            } else {
                ("most", max)
            };
            format!(
                "{name} expected at {bound} {count} argument{}, got {}",
                if count == 1 { "" } else { "s" },
                args.len()
            )
        } else {
            format!(
                "expected {min} argument{}, got {}",
                if min == 1 { "" } else { "s" },
                args.len()
            )
        };
        return raise_exception::<_>(py, "TypeError", &message);
    }
    unsafe {
        match operation {
            WrapperOperation::Get => {
                let instance = (!obj_from_bits(args[0]).is_none()).then_some(args[0]);
                let owner = args
                    .get(1)
                    .copied()
                    .filter(|value| !obj_from_bits(*value).is_none());
                if instance.is_none() && owner.is_none() {
                    return raise_exception::<_>(py, "TypeError", "__get__(None, None) is invalid");
                }
                wrapper_get(py, receiver, instance, owner)
            }
            WrapperOperation::Set => property_mutate(py, receiver, args[0], Some(args[1])),
            WrapperOperation::Delete => property_mutate(py, receiver, args[0], None),
            WrapperOperation::SetName => {
                let _inputs = WrapperInputs::new(py, [receiver, args[1]]);
                if !property_replace_name_bits(py, ptr, args[1]) {
                    return wrapper_storage_failure(py);
                }
                MoltObject::none().bits()
            }
            _ => unreachable!(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_new(target: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_construct(py, WrapperKind::Staticmethod, &[target], &[])
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_new(target: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_construct(py, WrapperKind::Classmethod, &[target], &[])
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_new(get: u64, set: u64, delete: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_construct(py, WrapperKind::Property, &[get, set, delete], &[])
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_getter(wrapper: u64, get: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { property_copy(py, wrapper, get, 0) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_setter(wrapper: u64, set: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { property_copy(py, wrapper, set, 1) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_deleter(wrapper: u64, delete: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { property_copy(py, wrapper, delete, 2) })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_type_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Staticmethod,
            WrapperOperation::New,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Staticmethod,
            WrapperOperation::Init,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_get(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Staticmethod,
            WrapperOperation::Get,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_call(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Staticmethod,
            WrapperOperation::Call,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_type_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Classmethod,
            WrapperOperation::New,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Classmethod,
            WrapperOperation::Init,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_get(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Classmethod,
            WrapperOperation::Get,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_type_new(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::New,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_init(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::Init,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_get(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::Get,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_set(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::Set,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_delete(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::Delete,
            args,
            kwargs,
        )
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_set_name(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        wrapper_method(
            py,
            WrapperKind::Property,
            WrapperOperation::SetName,
            args,
            kwargs,
        )
    })
}
