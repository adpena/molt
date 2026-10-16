use super::*;
use crate::builtins::attr::{
    class_namespace_lookup_mro_pure, descriptor_call1, descriptor_has_get,
};
use crate::builtins::exceptions::ExceptionValue;
use crate::runtime_flags::{self, RuntimeFlag};
use std::cell::Cell;

/// The builtin `__getattribute__` a receiver kind uses when its class defines
/// none of its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DefaultGetattribute {
    Object,
    Module,
    Type,
}

impl DefaultGetattribute {
    fn bits(self, py: &PyToken<'_>) -> Result<Option<u64>, ()> {
        match self {
            Self::Object => object_method_bits(py, "__getattribute__"),
            Self::Module => {
                builtin_class_method_bits(py, builtin_classes(py).module, "__getattribute__")
            }
            Self::Type => type_method_bits(py, "__getattribute__"),
        }
    }
}

/// A class's attribute protocol is plain when its MRO has no `__getattr__`
/// and its `__getattribute__` is the receiver kind's default. The fact is
/// keyed like the descriptor cache: class identity, the class layout version
/// and the global type version, which advances on every class mutation and
/// on every type release, so a reused class address never matches.
#[derive(Clone, Copy)]
struct PlainProtocolEntry {
    class_bits: u64,
    layout_version: u64,
    type_version: u64,
    default: DefaultGetattribute,
    plain: bool,
}

const PLAIN_PROTOCOL_SLOTS: usize = 64;

thread_local! {
    static PLAIN_PROTOCOL: [Cell<Option<PlainProtocolEntry>>; PLAIN_PROTOCOL_SLOTS] =
        const { [const { Cell::new(None) }; PLAIN_PROTOCOL_SLOTS] };
}

/// Whether `owner` needs neither hook resolution: true lets a lookup skip the
/// two MRO walks. Only callback-free, published namespace facts are cached;
/// an undecided namespace answers false without caching, so the full
/// transaction decides.
pub(super) unsafe fn plain_attribute_protocol(
    py: &PyToken<'_>,
    owner: *mut u8,
    default: DefaultGetattribute,
) -> Result<bool, ()> {
    unsafe {
        let class_bits = MoltObject::from_ptr(owner).bits();
        let layout_version = crate::class_layout_version_bits(owner);
        let type_version = crate::object::global_type_version();
        let slot = ((class_bits >> 4) ^ (class_bits >> 12)) as usize % PLAIN_PROTOCOL_SLOTS;
        let cached = PLAIN_PROTOCOL.with(|slots| slots[slot].get());
        if let Some(entry) = cached
            && entry.class_bits == class_bits
            && entry.layout_version == layout_version
            && entry.type_version == type_version
            && entry.default == default
        {
            return Ok(entry.plain);
        }
        let (Some(getattr), Some(Some(getattribute))) = (
            class_namespace_lookup_mro_pure(py, owner, b"__getattr__", None),
            class_namespace_lookup_mro_pure(py, owner, b"__getattribute__", None),
        ) else {
            return Ok(false);
        };
        let plain = getattr.is_none() && Some(getattribute) == default.bits(py)?;
        // Resolving the default may publish a namespace; store only a fact
        // that still describes the current class graph.
        if crate::class_layout_version_bits(owner) == layout_version
            && crate::object::global_type_version() == type_version
        {
            PLAIN_PROTOCOL.with(|slots| {
                slots[slot].set(Some(PlainProtocolEntry {
                    class_bits,
                    layout_version,
                    type_version,
                    default,
                    plain,
                }));
            });
        }
        Ok(plain)
    }
}

/// One normal attribute transaction for custom and default lookup, on ordinary
/// objects, dataclasses, dictless instances and metaclass receivers alike.
/// Capture __getattr__ before any getattribute/descriptor callback can replace
/// it. Explicit object/type.__getattribute__ entrypoints bypass this transaction.
pub(super) unsafe fn attribute_lookup_transaction<F>(
    py: &PyToken<'_>,
    object: *mut u8,
    owner: *mut u8,
    name: u64,
    default: DefaultGetattribute,
    default_lookup: F,
) -> Result<Option<u64>, ()>
where
    F: FnOnce() -> Result<Option<u64>, ()>,
{
    unsafe {
        let receiver = MoltObject::from_ptr(object).bits();
        let owner_bits = MoltObject::from_ptr(owner).bits();
        if plain_attribute_protocol(py, owner, default)? {
            // The same pins and exception framing as the full transaction,
            // without resolving hooks the class does not have.
            inc_ref_bits(py, receiver);
            inc_ref_bits(py, owner_bits);
            inc_ref_bits(py, name);
            exception_stack_push();
            let result = default_lookup();
            exception_stack_pop(py);
            dec_ref_bits(py, name);
            dec_ref_bits(py, owner_bits);
            dec_ref_bits(py, receiver);
            return result;
        }
        let default = default.bits(py)?;
        inc_ref_bits(py, receiver);
        inc_ref_bits(py, owner_bits);
        inc_ref_bits(py, name);
        let mut fallback = None;
        let mut raw = None;
        let result = (|| {
            let fallback_name =
                intern_static_name(py, &runtime_state(py).interned.getattr_name, b"__getattr__");
            fallback = class_attr_lookup_raw_mro(py, owner, fallback_name)?;
            if let Some(bits) = fallback {
                inc_ref_bits(py, bits);
            }
            let getattribute_name = intern_static_name(
                py,
                &runtime_state(py).interned.getattribute_name,
                b"__getattribute__",
            );
            raw = class_attr_lookup_raw_mro(py, owner, getattribute_name)?;
            if let Some(bits) = raw {
                inc_ref_bits(py, bits);
            }
            if fallback.is_some() {
                traceback_suppress_enter();
            }
            exception_stack_push();
            let result = match raw {
                Some(raw) if Some(raw) != default => {
                    descriptor_call1(py, raw, owner, Some(receiver), name)
                }
                _ => default_lookup(),
            };
            // The selected getattribute callable retires with its own failure
            // still selected, before AttributeError suppression or fallback.
            if let Some(bits) = raw.take() {
                dec_ref_bits(py, bits);
            }
            if fallback.is_some() {
                traceback_suppress_exit();
            }
            let result = match result {
                Err(()) if fallback.is_some() && clear_attribute_error_if_pending(py) => Ok(None),
                result => result,
            };
            exception_stack_pop(py);
            match (result, fallback) {
                (Ok(None), Some(fallback)) => {
                    exception_stack_push();
                    let result = descriptor_call1(py, fallback, owner, Some(receiver), name);
                    exception_stack_pop(py);
                    result
                }
                (result, _) => result,
            }
        })();
        if let Some(bits) = raw {
            dec_ref_bits(py, bits);
        }
        if let Some(bits) = fallback {
            dec_ref_bits(py, bits);
        }
        dec_ref_bits(py, name);
        dec_ref_bits(py, owner_bits);
        dec_ref_bits(py, receiver);
        result
    }
}

pub(crate) unsafe fn type_attr_lookup_ptr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Result<Option<u64>, ()> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, true) }
}

pub(crate) unsafe fn type_attr_lookup_ptr_default(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Result<Option<u64>, ()> {
    unsafe { type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, false) }
}

unsafe fn type_attr_lookup_ptr_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    allow_meta_custom_getattribute: bool,
) -> Result<Option<u64>, ()> {
    unsafe {
        let class_bits = MoltObject::from_ptr(obj_ptr).bits();
        inc_ref_bits(_py, class_bits);
        let _class_guard = crate::PtrDropGuard::new(obj_ptr);
        let meta_bits = object_class_bits(obj_ptr);
        let meta_ptr = if meta_bits != 0 {
            obj_from_bits(meta_bits).as_ptr()
        } else {
            obj_from_bits(builtin_classes(_py).type_obj).as_ptr()
        };
        let meta_ptr = match meta_ptr {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => Some(ptr),
            _ => None,
        };
        let _meta_guard = meta_ptr.map(|ptr| {
            inc_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            crate::PtrDropGuard::new(ptr)
        });
        if let Some(meta_ptr) = meta_ptr
            && allow_meta_custom_getattribute
        {
            return attribute_lookup_transaction(
                _py,
                obj_ptr,
                meta_ptr,
                attr_bits,
                DefaultGetattribute::Type,
                || type_attr_lookup_ptr_inner(_py, obj_ptr, attr_bits, false),
            );
        }
        // Capture one metaclass selection before descriptor classification or
        // class-namespace equality can replace its only namespace reference.
        let mut meta_attribute = match meta_ptr {
            Some(meta) => class_attr_lookup_raw_mro(_py, meta, attr_bits)?
                .map(|bits| ExceptionValue::pin(_py, bits)),
            None => None,
        };
        if let Some(selected) = meta_attribute.as_ref()
            && descriptor_has_get(_py, selected.bits())?
            && descriptor_is_data(_py, selected.bits())?
        {
            return descriptor_bind(
                _py,
                selected.bits(),
                meta_ptr.map(|ptr| MoltObject::from_ptr(ptr).bits()),
                Some(class_bits),
            );
        }
        // Look through the class MRO once. Pin the local selection before
        // retiring the unused metaclass candidate, then bind it.
        if let Some(bits) = class_attr_lookup_raw_mro(_py, obj_ptr, attr_bits)? {
            let local = ExceptionValue::pin(_py, bits);
            drop(meta_attribute.take());
            return descriptor_bind(_py, local.bits(), Some(class_bits), None);
        }

        if let Some(name) = string_obj_to_owned(obj_from_bits(attr_bits)) {
            let builtins = builtin_classes(_py);
            if class_bits == builtins.object
                && (name == "__getattribute__" || name == "__setattr__" || name == "__delattr__")
                && let Some(func_bits) = object_method_bits(_py, name.as_str())?
            {
                inc_ref_bits(_py, func_bits);
                drop(meta_attribute.take());
                return Ok(Some(func_bits));
            }
            if name == "__init_subclass__"
                && runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_INIT_SUBCLASS)
            {
                let builtins = builtin_classes(_py);
                eprintln!(
                    "molt init_subclass lookup class_bits=0x{:x} builtins.object=0x{:x} is_builtin={}",
                    class_bits,
                    builtins.object,
                    is_builtin_class_bits(_py, class_bits),
                );
            }
            if class_bits == builtins.tuple
                && name == "__new__"
                && let Some(func_bits) = builtin_class_method_bits(_py, class_bits, "__new__")?
            {
                inc_ref_bits(_py, func_bits);
                drop(meta_attribute.take());
                return Ok(Some(func_bits));
            }

            let class_bits = MoltObject::from_ptr(obj_ptr).bits();
            if name == "fromkeys" {
                let builtins = builtin_classes(_py);
                if issubclass_bits(class_bits, builtins.dict)
                    && let Some(func_bits) = dict_method_bits(_py, name.as_str())?
                {
                    let function = ExceptionValue::pin(_py, func_bits);
                    drop(meta_attribute.take());
                    let bound_bits = molt_bound_method_new(function.bits(), class_bits);
                    return if obj_from_bits(bound_bits).is_none() {
                        Err(())
                    } else {
                        Ok(Some(bound_bits))
                    };
                }
            }
        }
        if let Some(selected) = meta_attribute {
            return descriptor_bind(
                _py,
                selected.bits(),
                meta_ptr.map(|ptr| MoltObject::from_ptr(ptr).bits()),
                Some(class_bits),
            );
        }
        Ok(None)
    }
}
