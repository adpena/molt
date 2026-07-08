use super::*;

pub(crate) struct OperatorRuntimeState {
    pub(crate) itemgetter_class: AtomicU64,
    pub(crate) attrgetter_class: AtomicU64,
    pub(crate) methodcaller_class: AtomicU64,
    pub(crate) itemgetter_call: AtomicU64,
    pub(crate) attrgetter_call: AtomicU64,
    pub(crate) methodcaller_call: AtomicU64,
    pub(crate) itemgetter_init: AtomicU64,
    pub(crate) attrgetter_init: AtomicU64,
    pub(crate) methodcaller_init: AtomicU64,
}

impl OperatorRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            itemgetter_class: AtomicU64::new(0),
            attrgetter_class: AtomicU64::new(0),
            methodcaller_class: AtomicU64::new(0),
            itemgetter_call: AtomicU64::new(0),
            attrgetter_call: AtomicU64::new(0),
            methodcaller_call: AtomicU64::new(0),
            itemgetter_init: AtomicU64::new(0),
            attrgetter_init: AtomicU64::new(0),
            methodcaller_init: AtomicU64::new(0),
        }
    }

    pub(crate) fn slots(&self) -> [&AtomicU64; 9] {
        [
            &self.itemgetter_class,
            &self.attrgetter_class,
            &self.methodcaller_class,
            &self.itemgetter_call,
            &self.attrgetter_call,
            &self.methodcaller_call,
            &self.itemgetter_init,
            &self.attrgetter_init,
            &self.methodcaller_init,
        ]
    }
}

pub(crate) fn operator_clear_runtime_state(_py: &PyToken<'_>, state: &crate::state::RuntimeState) {
    crate::gil_assert();
    let slots = state.operator.slots();
    crate::state::cache::clear_atomic_slots(_py, &slots);
}
