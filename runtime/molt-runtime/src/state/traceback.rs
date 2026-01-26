use crate::TRACEBACK_SUPPRESS;

pub(crate) fn traceback_suppress_enter() {
    TRACEBACK_SUPPRESS.with(|cell| {
        cell.set(cell.get().saturating_add(1));
    });
}

pub(crate) fn traceback_suppress_exit() {
    TRACEBACK_SUPPRESS.with(|cell| {
        let current = cell.get();
        if current > 0 {
            cell.set(current - 1);
        }
    });
}

pub(crate) fn traceback_suppressed() -> bool {
    TRACEBACK_SUPPRESS.with(|cell| cell.get() > 0)
}
