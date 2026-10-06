use crate::PyToken;
use crate::builtins::functions::native_callable::NativeCallableKind;
use crate::builtins::types::{
    ClassSemanticPolicy, RuntimeClassMethodSpec, RuntimeMethodSignature,
    SELF_NAME_RUNTIME_ARGUMENT_NAMES, SELF_RUNTIME_ARGUMENT_NAMES, init_cached_runtime_class,
};

pub(super) fn itemgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_itemgetter_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_itemgetter_init as *const () as usize as u64,
            2,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, false),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.itemgetter_class,
        "itemgetter",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            layout_size: 16,
            instance_shape: Some(crate::object::ObjectShapeId::OperatorItemGetter),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}

pub(super) fn attrgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_attrgetter_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_attrgetter_init as *const () as usize as u64,
            2,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, false),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.attrgetter_class,
        "attrgetter",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            layout_size: 16,
            instance_shape: Some(crate::object::ObjectShapeId::OperatorAttrGetter),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}

pub(super) fn methodcaller_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_methodcaller_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            NativeCallableKind::WrapperDescriptor,
            crate::molt_operator_methodcaller_init as *const () as usize as u64,
            4,
            RuntimeMethodSignature::new(SELF_NAME_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.methodcaller_class,
        "methodcaller",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            layout_size: 32,
            instance_shape: Some(crate::object::ObjectShapeId::OperatorMethodCaller),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}
