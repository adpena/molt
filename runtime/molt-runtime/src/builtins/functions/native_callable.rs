//! Native callable identity is declared before publication, independently of its
//! portable function payload and private argument-binding metadata.
use crate::call::class_init::function_set_attr_name;
use crate::object::object_replace_class_edge;
use crate::*;
use std::sync::atomic::AtomicU64;

pub(crate) use crate::object::function_metadata::FunctionMetadataField as CallableMetadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeCallableKind {
    Constructor,
    Function,
    CMethod,
    MethodDescriptor,
    WrapperDescriptor,
    ClassMethodDescriptor,
    MethodWrapper,
}

impl NativeCallableKind {
    pub(crate) fn class(self, py: &PyToken<'_>) -> u64 {
        let classes = builtin_classes(py);
        match self {
            Self::Constructor | Self::Function => classes.builtin_function_or_method,
            Self::CMethod => classes.builtin_method,
            Self::MethodDescriptor => classes.method_descriptor,
            Self::WrapperDescriptor => classes.wrapper_descriptor,
            Self::ClassMethodDescriptor => classes.classmethod_descriptor,
            Self::MethodWrapper => classes.method_wrapper,
        }
    }

    pub(crate) fn from_class(py: &PyToken<'_>, class: u64) -> Option<Self> {
        let classes = builtin_classes(py);
        [
            Self::Function,
            Self::CMethod,
            Self::MethodDescriptor,
            Self::WrapperDescriptor,
            Self::ClassMethodDescriptor,
            Self::MethodWrapper,
        ]
        .into_iter()
        .find(|kind| match kind {
            Self::Constructor => false,
            Self::Function => class == classes.builtin_function_or_method,
            Self::CMethod => class == classes.builtin_method,
            Self::MethodDescriptor => class == classes.method_descriptor,
            Self::WrapperDescriptor => class == classes.wrapper_descriptor,
            Self::ClassMethodDescriptor => class == classes.classmethod_descriptor,
            Self::MethodWrapper => class == classes.method_wrapper,
        })
    }

    pub(crate) fn is_descriptor(self) -> bool {
        matches!(
            self,
            Self::MethodDescriptor | Self::WrapperDescriptor | Self::ClassMethodDescriptor
        )
    }

    pub(crate) fn bound_class(self, py: &PyToken<'_>) -> u64 {
        match self {
            Self::WrapperDescriptor | Self::MethodWrapper => Self::MethodWrapper.class(py),
            Self::CMethod => Self::CMethod.class(py),
            _ => Self::Function.class(py),
        }
    }

    /// Physical metadata declarations for each native callable class.
    pub(crate) fn metadata_fields(self) -> &'static [CallableMetadata] {
        match self {
            Self::Constructor | Self::Function | Self::CMethod => &[
                CallableMetadata::Name,
                CallableMetadata::QualName,
                CallableMetadata::Doc,
                CallableMetadata::TextSignature,
                CallableMetadata::Module,
                CallableMetadata::SelfValue,
            ],
            Self::MethodDescriptor | Self::WrapperDescriptor | Self::ClassMethodDescriptor => &[
                CallableMetadata::Name,
                CallableMetadata::QualName,
                CallableMetadata::Doc,
                CallableMetadata::TextSignature,
                CallableMetadata::Owner,
            ],
            Self::MethodWrapper => &[
                CallableMetadata::Name,
                CallableMetadata::QualName,
                CallableMetadata::Doc,
                CallableMetadata::TextSignature,
                CallableMetadata::Owner,
                CallableMetadata::SelfValue,
            ],
        }
    }

    pub(crate) fn has_module(self) -> bool {
        matches!(self, Self::Function | Self::CMethod)
    }
}

/// Passed by the owning method declaration; never inferred from names or native
/// code addresses. The cached callable retains its owner and metadata once.
#[derive(Clone, Copy)]
pub(crate) struct NativeCallableSpec<'a> {
    pub(crate) kind: NativeCallableKind,
    pub(crate) owner: Option<u64>,
    pub(crate) name: Option<&'a str>,
    pub(crate) self_bits: Option<u64>,
    pub(crate) cache: Option<&'a AtomicU64>,
    pub(crate) text_signature: Option<&'a str>,
}

impl<'a> NativeCallableSpec<'a> {
    pub(crate) const fn function(cache: &'a AtomicU64) -> Self {
        Self {
            cache: Some(cache),
            ..Self::uncached_function()
        }
    }

    pub(crate) const fn uncached_function() -> Self {
        Self {
            kind: NativeCallableKind::Function,
            owner: None,
            name: None,
            self_bits: None,
            cache: None,
            text_signature: None,
        }
    }

    pub(crate) const fn declared(kind: NativeCallableKind, owner: u64, name: &'a str) -> Self {
        Self {
            kind,
            owner: Some(owner),
            name: Some(name),
            self_bits: None,
            cache: None,
            text_signature: None,
        }
    }

    pub(crate) const fn constructor(owner: u64) -> Self {
        Self {
            kind: NativeCallableKind::Function,
            owner: Some(owner),
            name: Some("__new__"),
            self_bits: Some(owner),
            cache: None,
            text_signature: None,
        }
    }

    pub(crate) const fn with_text_signature(self, signature: &'a str) -> Self {
        Self {
            text_signature: Some(signature),
            ..self
        }
    }
}

/// Read canonical class metadata without treating a foreign wrapper as the
/// managed class payload. Preserve Python string bytes, including surrogates.
unsafe fn owner_qualname_bytes(py: &PyToken<'_>, owner: u64) -> Result<Vec<u8>, ()> {
    unsafe {
        let pointer = obj_from_bits(owner).as_ptr().ok_or(())?;
        if object_type_id(pointer) == TYPE_ID_TYPE {
            let qualified = class_qualname_bits(pointer);
            let name = if qualified == 0 {
                class_name_bits(pointer)
            } else {
                qualified
            };
            return crate::object::ops_format::string_obj_bytes(obj_from_bits(name)).ok_or_else(
                || {
                    raise_exception::<()>(
                        py,
                        "SystemError",
                        "native callable owner has no qualified name",
                    );
                },
            );
        }
        let view = match crate::object::class_layout::real_class_view(owner) {
            Ok(Some(view)) => view,
            Ok(None) => {
                raise_exception::<()>(py, "SystemError", "native callable owner is not a type");
                return Err(());
            }
            Err(()) => {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native callable owner");
                return Err(());
            }
        };
        let name = molt_cpython_abi::api::typeobj::PyType_GetQualName(view);
        if name.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "native callable owner qualified name",
            );
            return Err(());
        }
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(name);
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            molt_cpython_abi::api::refcount::Py_DECREF(name);
        });
        let Some(bits) = bits else {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "native callable owner qualified name",
            );
            return Err(());
        };
        let bytes = crate::object::ops_format::string_obj_bytes(obj_from_bits(bits));
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
        bytes.ok_or_else(|| {
            raise_exception::<()>(
                py,
                "SystemError",
                "native callable owner qualified name is not a string",
            );
        })
    }
}

pub(crate) unsafe fn configure_native_callable(
    py: &PyToken<'_>,
    pointer: *mut u8,
    spec: NativeCallableSpec<'_>,
) -> bool {
    unsafe {
        let class = spec.kind.class(py);
        if object_class_bits(pointer) != class
            && !object_replace_class_edge(py, pointer, class, ClassEdgeOwnership::Owned)
        {
            raise_exception::<u64>(
                py,
                "SystemError",
                "native callable identity is already published",
            );
            return false;
        }
        if let Some(owner) = spec.owner
            && spec.kind.is_descriptor()
            && !function_set_attr_name(py, pointer, b"__objclass__", owner)
        {
            return false;
        }
        if let Some(name) = spec.name {
            let Some(name_bits) = attr_name_bits_from_bytes(py, name.as_bytes()) else {
                return false;
            };
            let named = function_set_attr_name(py, pointer, b"__name__", name_bits);
            dec_ref_bits(py, name_bits);
            if !named {
                return false;
            }
            let mut qualified = if let Some(owner) = spec.owner {
                let Ok(mut qualified) = owner_qualname_bytes(py, owner) else {
                    return false;
                };
                qualified.push(b'.');
                qualified
            } else {
                Vec::new()
            };
            qualified.extend_from_slice(name.as_bytes());
            let Some(qualified_bits) = attr_name_bits_from_bytes(py, &qualified) else {
                return false;
            };
            let named = function_set_attr_name(py, pointer, b"__qualname__", qualified_bits);
            dec_ref_bits(py, qualified_bits);
            if !named {
                return false;
            }
        }
        if let Some(signature) = spec.text_signature {
            let Some(bits) = attr_name_bits_from_bytes(py, signature.as_bytes()) else {
                return false;
            };
            let written = function_set_attr_name(py, pointer, b"__text_signature__", bits);
            dec_ref_bits(py, bits);
            if !written {
                return false;
            }
        }
        if let Some(value) = spec.self_bits
            && !function_set_attr_name(py, pointer, b"__self__", value)
        {
            return false;
        }
        !exception_pending(py)
    }
}

/// A native classmethod may create a foreign wrapper for the live native type.
/// Hold that owner until the bound method or immediate call acquires custody.
pub(crate) struct NativeDescriptorReceiver {
    bits: u64,
    owner: Option<crate::PtrDropGuard>,
}

impl NativeDescriptorReceiver {
    pub(crate) fn borrowed(bits: u64) -> Self {
        Self { bits, owner: None }
    }

    unsafe fn owned_type(bits: u64) -> Self {
        Self {
            bits,
            owner: Some(crate::PtrDropGuard::new(
                obj_from_bits(bits).as_ptr().expect("owned real type"),
            )),
        }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }
}

impl Drop for NativeDescriptorReceiver {
    fn drop(&mut self) {
        if self.owner.is_some() {
            molt_cpython_abi::api::errors::with_preserved_error(|| drop(self.owner.take()));
        }
    }
}

/// CPython distinguishes invoking a slot wrapper from binding its descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeDescriptorContext {
    Binding,
    Call,
}

/// Public native identity is a typed field, not the qualified binder label.
unsafe fn native_callable_name(pointer: *mut u8) -> Option<Vec<u8>> {
    unsafe {
        CallableMetadata::Name
            .load(pointer)
            .and_then(|bits| crate::object::ops_format::string_obj_bytes(obj_from_bits(bits)))
    }
}

/// Error labels read actual class storage, never __name__/__class__ hooks.
/// A failed foreign class inquiry remains the original native failure.
unsafe fn descriptor_class_name(py: &PyToken<'_>, class: u64) -> Result<Vec<u8>, ()> {
    unsafe {
        let bytes = if let Some(pointer) = obj_from_bits(class).as_ptr()
            && object_type_id(pointer) == TYPE_ID_TYPE
        {
            crate::object::ops_format::string_obj_bytes(obj_from_bits(class_name_bits(pointer)))
        } else {
            let view = crate::object::class_layout::real_class_view(class).map_err(|()| {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native descriptor class name",
                );
            })?;
            view.and_then(|view| {
                let name = (*view).tp_name;
                (!name.is_null()).then(|| std::ffi::CStr::from_ptr(name).to_bytes().to_vec())
            })
        };
        let Some(bytes) = bytes else {
            raise_exception::<()>(py, "SystemError", "native descriptor class has no name");
            return Err(());
        };
        // CPython's diagnostic type labels use %.100s. Its UTF-8 C-string
        // conversion replaces an incomplete character at the precision edge.
        Ok(String::from_utf8_lossy(&bytes[..bytes.len().min(100)])
            .into_owned()
            .into_bytes())
    }
}

unsafe fn descriptor_received_type_name(py: &PyToken<'_>, value: u64) -> Result<Vec<u8>, ()> {
    unsafe {
        let class = crate::object::class_layout::real_type_bits(py, value).map_err(|()| {
            crate::cpython_abi_hooks::propagate_native_failure(py, "native receiver type");
        })?;
        let owner = NativeDescriptorReceiver::owned_type(class);
        descriptor_class_name(py, owner.bits())
    }
}

unsafe fn raise_descriptor_receiver_error(
    py: &PyToken<'_>,
    pointer: *mut u8,
    kind: NativeCallableKind,
    context: NativeDescriptorContext,
    declaring: u64,
    receiver: Option<u64>,
) -> Result<(), ()> {
    unsafe {
        let name = native_callable_name(pointer).unwrap_or_else(|| b"?".to_vec());
        let expected = descriptor_class_name(py, declaring)?;
        let message = if let Some(receiver) = receiver {
            if kind == NativeCallableKind::ClassMethodDescriptor {
                let is_type = if obj_from_bits(receiver)
                    .as_ptr()
                    .is_some_and(|pointer| object_type_id(pointer) == TYPE_ID_TYPE)
                {
                    true
                } else {
                    crate::object::class_layout::real_class_view(receiver)
                        .map_err(|()| {
                            crate::cpython_abi_hooks::propagate_native_failure(
                                py,
                                "native receiver type",
                            );
                        })?
                        .is_some()
                };
                if is_type {
                    let actual = descriptor_class_name(py, receiver)?;
                    [
                        b"descriptor '".as_slice(),
                        &name,
                        b"' requires a subtype of '",
                        &expected,
                        b"' but received '",
                        &actual,
                        b"'",
                    ]
                    .concat()
                } else {
                    let actual = descriptor_received_type_name(py, receiver)?;
                    [
                        b"descriptor '".as_slice(),
                        &name,
                        b"' for type '",
                        &expected,
                        b"' needs a type, not a '",
                        &actual,
                        b"' as arg 2",
                    ]
                    .concat()
                }
            } else {
                let actual = descriptor_received_type_name(py, receiver)?;
                if kind == NativeCallableKind::WrapperDescriptor
                    && context == NativeDescriptorContext::Call
                {
                    [
                        b"descriptor '".as_slice(),
                        &name,
                        b"' requires a '",
                        &expected,
                        b"' object but received a '",
                        &actual,
                        b"'",
                    ]
                    .concat()
                } else {
                    [
                        b"descriptor '".as_slice(),
                        &name,
                        b"' for '",
                        &expected,
                        b"' objects doesn't apply to a '",
                        &actual,
                        b"' object",
                    ]
                    .concat()
                }
            }
        } else if context == NativeDescriptorContext::Binding {
            [
                b"descriptor '".as_slice(),
                &name,
                b"' for type '",
                &expected,
                b"' needs either an object or a type",
            ]
            .concat()
        } else if kind == NativeCallableKind::MethodDescriptor {
            let qualified = CallableMetadata::QualName
                .load(pointer)
                .and_then(|bits| crate::object::ops_format::string_obj_bytes(obj_from_bits(bits)))
                .unwrap_or(name);
            [
                b"unbound method ".as_slice(),
                &qualified,
                b"() needs an argument",
            ]
            .concat()
        } else {
            [
                b"descriptor '".as_slice(),
                &name,
                b"' of '",
                &expected,
                b"' object needs an argument",
            ]
            .concat()
        };
        crate::builtins::exceptions::raise_exception_bytes::<()>(py, "TypeError", &message);
        Ok(())
    }
}

/// Binding and direct calls share receiver admission and public metadata.
pub(crate) unsafe fn native_descriptor_receiver(
    py: &PyToken<'_>,
    pointer: *mut u8,
    kind: NativeCallableKind,
    context: NativeDescriptorContext,
    owner: Option<u64>,
    instance: Option<u64>,
) -> Result<Option<NativeDescriptorReceiver>, ()> {
    unsafe {
        if !kind.is_descriptor() {
            return Ok(None);
        }
        let receiver = if kind == NativeCallableKind::ClassMethodDescriptor {
            if let Some(owner) = owner {
                Some(NativeDescriptorReceiver::borrowed(owner))
            } else if let Some(instance) = instance {
                let class =
                    crate::object::class_layout::real_type_bits(py, instance).map_err(|()| {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "native receiver type",
                        );
                    })?;
                Some(NativeDescriptorReceiver::owned_type(class))
            } else {
                None
            }
        } else {
            instance.map(NativeDescriptorReceiver::borrowed)
        };
        let declaring = CallableMetadata::Owner
            .load(pointer)
            .unwrap_or(MoltObject::none().bits());
        let Some(receiver) = receiver else {
            if context == NativeDescriptorContext::Call
                || kind == NativeCallableKind::ClassMethodDescriptor
            {
                raise_descriptor_receiver_error(py, pointer, kind, context, declaring, None)?;
                return Err(());
            }
            return Ok(None);
        };
        let valid = if !obj_from_bits(declaring).is_none() {
            if kind == NativeCallableKind::ClassMethodDescriptor {
                crate::object::class_layout::try_is_real_subtype(py, receiver.bits(), declaring)
            } else {
                crate::object::class_layout::try_is_real_instance(py, receiver.bits(), declaring)
            }
            .map_err(|()| {
                crate::cpython_abi_hooks::propagate_native_failure(py, "native receiver admission");
            })?
        } else {
            false
        };
        if !valid {
            raise_descriptor_receiver_error(
                py,
                pointer,
                kind,
                context,
                declaring,
                Some(receiver.bits()),
            )?;
            return Err(());
        }
        Ok(Some(receiver))
    }
}

/// Repr reads public identity without exposing private binder storage.
pub(crate) unsafe fn native_callable_repr(py: &PyToken<'_>, public: *mut u8) -> Option<Vec<u8>> {
    use crate::object::ops_format::format_class_name_bytes;
    unsafe {
        let kind = NativeCallableKind::from_class(py, object_class_bits(public))?;
        let bound = object_type_id(public) == TYPE_ID_BOUND_METHOD;
        let function = if bound {
            obj_from_bits(bound_method_func_bits(public)).as_ptr()?
        } else {
            public
        };
        let name = native_callable_name(function).unwrap_or_default();
        if kind.is_descriptor() {
            let declaring = CallableMetadata::Owner
                .load(function)
                .unwrap_or(MoltObject::none().bits());
            let owner = if obj_from_bits(declaring).is_none() {
                Vec::new()
            } else {
                format_class_name_bytes(declaring)
            };
            let label = if kind == NativeCallableKind::WrapperDescriptor {
                b"<slot wrapper '".as_slice()
            } else {
                b"<method '".as_slice()
            };
            let mut out = label.to_vec();
            out.extend_from_slice(&name);
            out.extend_from_slice(b"' of '");
            out.extend_from_slice(&owner);
            out.extend_from_slice(b"' objects>");
            return Some(out);
        }
        if bound {
            let receiver = bound_method_self_bits(public);
            let class = match crate::object::class_layout::real_type_bits(py, receiver) {
                Ok(class) => class,
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "native receiver type");
                    return None;
                }
            };
            let receiver_type = format_class_name_bytes(class);
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, class));
            let address = obj_from_bits(receiver)
                .as_ptr()
                .map_or(receiver as usize, |ptr| ptr as usize);
            let mut out = if kind == NativeCallableKind::MethodWrapper {
                let mut out = b"<method-wrapper '".to_vec();
                out.extend_from_slice(&name);
                out.extend_from_slice(b"' of ");
                out
            } else {
                let mut out = b"<built-in method ".to_vec();
                out.extend_from_slice(&name);
                out.extend_from_slice(b" of ");
                out
            };
            out.extend_from_slice(&receiver_type);
            out.extend_from_slice(format!(" object at 0x{address:x}>").as_bytes());
            return Some(out);
        }
        Some(if name.is_empty() {
            b"<built-in function>".to_vec()
        } else {
            let mut out = b"<built-in function ".to_vec();
            out.extend_from_slice(&name);
            out.push(b'>');
            out
        })
    }
}
/// Receiver validation for direct Python calls, before variadic ABI packing.
pub(crate) unsafe fn admit_native_call(
    py: &PyToken<'_>,
    function: *mut u8,
    receiver: Option<u64>,
) -> bool {
    unsafe {
        let Some(kind) = NativeCallableKind::from_class(py, object_class_bits(function)) else {
            return true;
        };
        if !kind.is_descriptor() {
            return true;
        }
        let (owner, instance) = if kind == NativeCallableKind::ClassMethodDescriptor {
            (receiver, None)
        } else {
            (None, receiver)
        };
        // Option marks an absent C operand. Python None is a real receiver;
        // only the Python-visible __get__ wrapper normalizes its sentinel.
        match native_descriptor_receiver(
            py,
            function,
            kind,
            NativeDescriptorContext::Call,
            owner,
            instance,
        ) {
            Ok(Some(_)) => true,
            Err(()) => false,
            Ok(None) => {
                raise_exception::<u64>(
                    py,
                    "SystemError",
                    "native descriptor call admitted no receiver",
                );
                false
            }
        }
    }
}

pub(crate) unsafe fn admit_native_callable(
    py: &PyToken<'_>,
    callable: u64,
    positional: &[u64],
) -> bool {
    unsafe {
        let Some(pointer) = obj_from_bits(callable).as_ptr() else {
            return true;
        };
        match object_type_id(pointer) {
            TYPE_ID_FUNCTION => admit_native_call(py, pointer, positional.first().copied()),
            TYPE_ID_BOUND_METHOD => {
                let Some(function) = obj_from_bits(bound_method_func_bits(pointer)).as_ptr() else {
                    return true;
                };
                object_type_id(function) != TYPE_ID_FUNCTION
                    || admit_native_call(py, function, Some(bound_method_self_bits(pointer)))
            }
            _ => true,
        }
    }
}
