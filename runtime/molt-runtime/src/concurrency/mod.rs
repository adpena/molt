pub(crate) mod gil;

pub(crate) use gil::{gil_assert, gil_held, with_gil, GilGuard, PyToken};

#[macro_export]
macro_rules! with_gil_entry {
    ($py:ident, $body:block) => {{
        let _gil_guard = $crate::concurrency::GilGuard::new();
        let $py = _gil_guard.token();
        let $py = &$py;
        $body
    }};
}
