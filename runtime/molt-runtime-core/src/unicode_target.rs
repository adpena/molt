//! Runtime-owned target selection over pure Unicode facts.
use molt_obj_model::unicode::UnicodeData;

/// Select a satellite's exact admitted target through the existing runtime
/// comparison ABI. The registry owns both coordinates and future admission;
/// an unregistered gap/future version has no descriptor.
pub fn for_runtime_target() -> Option<&'static UnicodeData> {
    UnicodeData::for_target_comparison(crate::rt_target_at_least)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn satellite_selection_uses_the_registered_coordinates_and_rejects_gaps() {
        use molt_obj_model::python_version_generated::SUPPORTED_TARGET_PYTHON_VERSIONS;
        for version in SUPPORTED_TARGET_PYTHON_VERSIONS {
            let (major, minor) = version.split_once('.').unwrap();
            let (major, minor) = (major.parse::<i64>().unwrap(), minor.parse::<i64>().unwrap());
            let selected =
                UnicodeData::for_target_comparison(|a, b| (major, minor) >= (a, b)).unwrap();
            assert!(std::ptr::eq(
                selected,
                UnicodeData::for_version((major, minor)).unwrap()
            ));
        }
        let future = SUPPORTED_TARGET_PYTHON_VERSIONS
            .iter()
            .map(|version| {
                let (a, b) = version.split_once('.').unwrap();
                (a.parse::<i64>().unwrap(), b.parse::<i64>().unwrap())
            })
            .max()
            .unwrap();
        let future = (future.0, future.1 + 1);
        assert!(UnicodeData::for_target_comparison(|a, b| future >= (a, b)).is_none());
        assert!(UnicodeData::for_target_comparison(|_, _| false).is_none());
    }
}
