use crate::{RECURSION_DEPTH, RECURSION_LIMIT};

pub(crate) fn recursion_limit_get() -> usize {
    RECURSION_LIMIT.with(|limit| limit.get())
}

pub(crate) fn recursion_limit_set(limit: usize) {
    RECURSION_LIMIT.with(|cell| cell.set(limit));
}

pub(crate) fn recursion_guard_enter() -> bool {
    let limit = recursion_limit_get();
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

pub(crate) fn recursion_guard_exit() {
    RECURSION_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
        }
    });
}
