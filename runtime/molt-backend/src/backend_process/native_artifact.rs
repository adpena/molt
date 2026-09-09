/// Backend request paths are Unicode strings. An unrepresentable environment
/// path is an admission error, never an absent extraction request.
pub(crate) fn shared_stdlib_archive_path_from_env() -> std::io::Result<Option<String>> {
    decode_shared_stdlib_archive_path(std::env::var_os("MOLT_STDLIB_OBJ"))
}

fn decode_shared_stdlib_archive_path(
    value: Option<std::ffi::OsString>,
) -> std::io::Result<Option<String>> {
    value.map(|value| {
        value.into_string().map_err(|_| std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MOLT_STDLIB_OBJ is present but not valid Unicode; backend request paths require Unicode",
        ))
    }).transpose()
}

/// Native transport kind is explicit: an archive is never a relocatable object.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeArtifactKind {
    #[default]
    Object,
    Archive,
}

impl NativeArtifactKind {
    /// Object transport owns the complete graph; shared extraction is an
    /// archive-only contract, including warm-cache and probe-only requests.
    pub(crate) fn validate_shared_stdlib(self, enabled: bool) -> Result<(), &'static str> {
        if self == Self::Object && enabled {
            return Err(
                "native object output requires the complete graph; disable shared stdlib extraction or request archive output",
            );
        }
        Ok(())
    }

    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "object" => Ok(Self::Object),
            "archive" => Ok(Self::Archive),
            _ => Err(format!(
                "native_output_kind must be object or archive, got {value:?}"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Archive => "archive",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_stdlib_admission_is_owned_by_native_transport_kind() {
        for kind in [NativeArtifactKind::Object, NativeArtifactKind::Archive] {
            for enabled in [false, true] {
                assert_eq!(
                    kind.validate_shared_stdlib(enabled).is_ok(),
                    kind == NativeArtifactKind::Archive || !enabled,
                );
            }
        }
    }

    #[test]
    fn shared_stdlib_environment_distinguishes_absent_empty_and_unicode_paths() {
        assert_eq!(
            decode_shared_stdlib_archive_path(None).expect("absent"),
            None
        );
        for path in ["", "cache/stdlib.a", "cache/stdlib-\u{03bb}.a"] {
            assert_eq!(
                decode_shared_stdlib_archive_path(Some(path.into())).expect("Unicode path"),
                Some(path.to_string()),
            );
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn non_unicode_shared_stdlib_path_is_never_absent() {
        #[cfg(unix)]
        let value = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff])
        };
        #[cfg(windows)]
        let value = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xd800])
        };
        let error = decode_shared_stdlib_archive_path(Some(value))
            .expect_err("present non-Unicode request is invalid, not disabled");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("MOLT_STDLIB_OBJ"));
        assert!(error.to_string().contains("Unicode"));
    }
}
