use crate::PyToken;
use crate::builtins::types::{
    RuntimeClassMethodSpec, RuntimeMethodSignature, SELF_NAME_RUNTIME_ARGUMENT_NAMES,
    SELF_RUNTIME_ARGUMENT_NAMES, init_cached_runtime_class,
};

pub(super) fn itemgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            &operator.itemgetter_call,
            crate::molt_operator_itemgetter_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            &operator.itemgetter_init,
            crate::molt_operator_itemgetter_init as *const () as usize as u64,
            2,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, false),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.itemgetter_class,
        "itemgetter",
        16,
        Some(crate::object::ObjectShapeId::OperatorItemGetter),
        &methods,
    )
}

pub(super) fn attrgetter_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            &operator.attrgetter_call,
            crate::molt_operator_attrgetter_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            &operator.attrgetter_init,
            crate::molt_operator_attrgetter_init as *const () as usize as u64,
            2,
            RuntimeMethodSignature::new(SELF_RUNTIME_ARGUMENT_NAMES, true, false),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.attrgetter_class,
        "attrgetter",
        16,
        Some(crate::object::ObjectShapeId::OperatorAttrGetter),
        &methods,
    )
}

pub(super) fn methodcaller_class(_py: &PyToken<'_>) -> u64 {
    let operator = &crate::runtime_state(_py).operator;
    let methods = [
        RuntimeClassMethodSpec::fixed(
            "__call__",
            &operator.methodcaller_call,
            crate::molt_operator_methodcaller_call as *const () as usize as u64,
            2,
        ),
        RuntimeClassMethodSpec::with_signature(
            "__init__",
            &operator.methodcaller_init,
            crate::molt_operator_methodcaller_init as *const () as usize as u64,
            4,
            RuntimeMethodSignature::new(SELF_NAME_RUNTIME_ARGUMENT_NAMES, true, true),
        ),
    ];
    init_cached_runtime_class(
        _py,
        &operator.methodcaller_class,
        "methodcaller",
        32,
        Some(crate::object::ObjectShapeId::OperatorMethodCaller),
        &methods,
    )
}
