use molt_ir::backend_environment::{self as backend_env, BackendEnv};
/// Backend request paths are Unicode strings. An unrepresentable environment
/// path is an admission error, never an absent extraction request.
pub(crate) fn shared_stdlib_archive_path_from_env() -> std::io::Result<Option<String>> {
    let output =
        decode_shared_stdlib_archive_path(backend_env::var_os(BackendEnv::MOLT_STDLIB_OBJ))?;
    validate_shared_stdlib_cache_output(output.as_deref())?;
    Ok(output)
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
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum NativeArtifactKind {
    #[default]
    Object,
    Archive,
}

impl NativeArtifactKind {
    /// A native object owns the complete graph; shared extraction is an
    /// archive-only contract, including warm-cache and probe-only requests.
    /// A wasm32 application object is no user artifact: the WASM link takes
    /// it together with the shared stdlib archive it was partitioned from.
    #[cfg(feature = "native-backend")]
    pub(crate) fn validate_shared_stdlib(
        self,
        enabled: bool,
        target_triple: Option<&str>,
    ) -> Result<(), &'static str> {
        let linked_with_archive =
            molt_backend::CodegenTarget::from_target_triple(target_triple).is_wasm();
        if self == Self::Object && enabled && !linked_with_archive {
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
}

/// The required archive obeys the one rule in
/// `shared_stdlib_cache::validate_required_stdlib_output`.
pub(crate) fn validate_shared_stdlib_cache_output(output: Option<&str>) -> std::io::Result<()> {
    #[cfg(feature = "native-backend")]
    if let Some(output) = output {
        super::shared_stdlib_cache::validate_required_stdlib_output(
            std::path::Path::new(output),
            None,
        )?;
    }
    #[cfg(not(feature = "native-backend"))]
    let _ = output;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_archive_is_a_store_entry_or_a_standalone_path() {
        validate_shared_stdlib_cache_output(Some(&format!(
            "cache/shared-stdlib/{}/stdlib.a",
            "a".repeat(64)
        )))
        .unwrap();
        validate_shared_stdlib_cache_output(Some("standalone.a")).unwrap();
        validate_shared_stdlib_cache_output(None).unwrap();
    }

    #[test]
    fn native_transport_protocol_labels_parse_without_aliasing() {
        for (kind, label) in [
            (NativeArtifactKind::Object, "object"),
            (NativeArtifactKind::Archive, "archive"),
        ] {
            assert_eq!(NativeArtifactKind::parse(label), Ok(kind));
        }
        assert!(NativeArtifactKind::parse("wasm").is_err());
    }

    #[test]
    fn shared_stdlib_admission_is_owned_by_native_transport_kind() {
        for kind in [NativeArtifactKind::Object, NativeArtifactKind::Archive] {
            for enabled in [false, true] {
                assert_eq!(
                    kind.validate_shared_stdlib(enabled, None).is_ok(),
                    kind == NativeArtifactKind::Archive || !enabled,
                );
                assert!(
                    kind.validate_shared_stdlib(enabled, Some("wasm32-wasip1"))
                        .is_ok()
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
