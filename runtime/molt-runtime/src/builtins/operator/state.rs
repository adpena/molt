use std::sync::atomic::AtomicU64;

use crate::PyToken;

// Each class namespace owns its declared __call__ and __init__ descriptors.
// The runtime retains only the three canonical class anchors.
pub(crate) struct OperatorRuntimeState {
    pub(crate) itemgetter_class: AtomicU64,
    pub(crate) attrgetter_class: AtomicU64,
    pub(crate) methodcaller_class: AtomicU64,
}

impl OperatorRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            itemgetter_class: AtomicU64::new(0),
            attrgetter_class: AtomicU64::new(0),
            methodcaller_class: AtomicU64::new(0),
        }
    }

    pub(crate) fn slots(&self) -> [&AtomicU64; 3] {
        [
            &self.itemgetter_class,
            &self.attrgetter_class,
            &self.methodcaller_class,
        ]
    }
}

pub(crate) fn operator_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let slots = state.operator.slots();
    crate::state::cache::clear_atomic_slots(_py, &slots)
}

pub(crate) fn operator_runtime_class_roots(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> Vec<u64> {
    crate::state::cache::cached_runtime_class_roots(py, &state.operator.slots())
}

pub(crate) fn operator_clear_runtime_callbacks(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::state::cache::clear_cached_runtime_callbacks(py, &state.operator.slots())
}
