use crate::PyToken;
use crate::object::ops_sys::runtime_target_at_least;

pub(crate) fn pep649_enabled(_py: &PyToken<'_>) -> bool {
    runtime_target_at_least(_py, 3, 14)
}
