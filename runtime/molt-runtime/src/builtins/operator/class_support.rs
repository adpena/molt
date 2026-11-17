use crate::PyToken;
use crate::builtins::functions::native_callable::NativeCallableKind;
use crate::builtins::types::{
    ClassSemanticPolicy, NO_RUNTIME_ARGUMENT_NAMES, RuntimeClassMethodSpec, RuntimeMethodSignature,
    init_cached_runtime_class,
};

pub(super) fn itemgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            crate::NameId::DunderCall,
            NativeCallableKind::WrapperDescriptor,
            fn_ref!(molt_operator_itemgetter_call),
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            fn_ref!(molt_operator_itemgetter_new),
            2,
            RuntimeMethodSignature::new(NO_RUNTIME_ARGUMENT_NAMES, true, true),
        )
        .with_owner_context(),
    ];
    init_cached_runtime_class(
        _py,
        &operator.itemgetter_class,
        "itemgetter",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            native_module: Some("operator"),
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
            crate::NameId::DunderCall,
            NativeCallableKind::WrapperDescriptor,
            fn_ref!(molt_operator_attrgetter_call),
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            fn_ref!(molt_operator_attrgetter_new),
            2,
            RuntimeMethodSignature::new(NO_RUNTIME_ARGUMENT_NAMES, true, true),
        )
        .with_owner_context(),
    ];
    init_cached_runtime_class(
        _py,
        &operator.attrgetter_class,
        "attrgetter",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            native_module: Some("operator"),
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
            crate::NameId::DunderCall,
            NativeCallableKind::WrapperDescriptor,
            fn_ref!(molt_operator_methodcaller_call),
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            crate::NameId::DunderNew,
            NativeCallableKind::Constructor,
            fn_ref!(molt_operator_methodcaller_new),
            2,
            RuntimeMethodSignature::new(NO_RUNTIME_ARGUMENT_NAMES, true, true),
        )
        .with_owner_context(),
    ];
    init_cached_runtime_class(
        _py,
        &operator.methodcaller_class,
        "methodcaller",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(true, false),
            native_module: Some("operator"),
            layout_size: 32,
            instance_shape: Some(crate::object::ObjectShapeId::OperatorMethodCaller),
            native_slots: Some(crate::object::class_storage::ClassSlotPolicy::default()),
        },
        &methods,
    )
}
