/// Backend request paths are Unicode strings. An unrepresentable environment
/// path is an admission error, never an absent extraction request.
#[cfg(any(
    feature = "native-backend",
    all(any(unix, test), feature = "wasm-backend")
))]
pub(crate) fn shared_stdlib_archive_path_from_env() -> std::io::Result<Option<String>> {
    let output = decode_shared_stdlib_archive_path(std::env::var_os("MOLT_STDLIB_OBJ"))?;
    let cache = shared_stdlib_cache_entry_path_from_env()?;
    validate_shared_stdlib_cache_output(output.as_deref(), cache.as_deref())?;
    Ok(output)
}

#[cfg(any(test, feature = "native-backend", all(unix, feature = "wasm-backend")))]
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
    #[cfg(any(test, feature = "native-backend", all(unix, feature = "wasm-backend")))]
    pub(crate) fn validate_shared_stdlib(self, enabled: bool) -> Result<(), &'static str> {
        if self == Self::Object && enabled {
            return Err(
                "native object output requires the complete graph; disable shared stdlib extraction or request archive output",
            );
        }
        Ok(())
    }

    // Daemon cache-key consumers require a compiled backend on Unix; their
    // protocol tests compile on every host, including feature-empty builds.
    #[cfg(any(
        test,
        all(unix, any(feature = "native-backend", feature = "wasm-backend"))
    ))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Archive => "archive",
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_cache_requires_distinct_required_output_before_admission() {
        for output in [None, Some("cache/stdlib.a")] {
            assert_eq!(
                validate_shared_stdlib_cache_output(output, Some("cache/stdlib.a"))
                    .expect_err("cache cannot be required output")
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
        validate_shared_stdlib_cache_output(
            Some("build/stdlib.a"),
            Some(&format!("cache/shared-stdlib/{}/stdlib.a", "a".repeat(64))),
        )
        .unwrap();
        validate_shared_stdlib_cache_output(Some("standalone.a"), None).unwrap();
    }

    #[test]
    fn native_transport_protocol_labels_roundtrip_without_aliasing() {
        for (kind, label) in [
            (NativeArtifactKind::Object, "object"),
            (NativeArtifactKind::Archive, "archive"),
        ] {
            assert_eq!(kind.as_str(), label);
            assert_eq!(NativeArtifactKind::parse(kind.as_str()), Ok(kind));
        }
        assert_ne!(
            NativeArtifactKind::Object.as_str(),
            NativeArtifactKind::Archive.as_str()
        );
        assert!(NativeArtifactKind::parse("wasm").is_err());
    }

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

/// Optional cache destination is independent of the required archive output.
pub(crate) fn shared_stdlib_cache_entry_path_from_env() -> std::io::Result<Option<String>> {
    match std::env::var("MOLT_STDLIB_CACHE_ENTRY") {
        Ok(path) if path.is_empty() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MOLT_STDLIB_CACHE_ENTRY must not be empty",
        )),
        Ok(path) => {
            #[cfg(feature = "native-backend")]
            let path =
                super::shared_stdlib_cache::resolved_archive_path(std::path::Path::new(&path))?
                    .into_os_string()
                    .into_string()
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "resolved stdlib cache path requires Unicode",
                        )
                    })?;
            Ok(Some(path))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MOLT_STDLIB_CACHE_ENTRY requires a Unicode path",
        )),
    }
}

/// Validate before daemon cache admission as well as direct typed preparation.
pub(crate) fn validate_shared_stdlib_cache_output(
    output: Option<&str>,
    cache: Option<&str>,
) -> std::io::Result<()> {
    #[cfg(feature = "native-backend")]
    if let Some(cache) = cache {
        let cache = super::shared_stdlib_cache::resolved_archive_path(std::path::Path::new(cache))?;
        if super::shared_stdlib_cache::canonical_stdlib_cache_root(&cache)?.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shared stdlib cache entry must use the canonical shared-stdlib archive path",
            ));
        }
    }
    #[cfg(feature = "native-backend")]
    if let Some(output) = output {
        super::shared_stdlib_cache::validate_required_stdlib_output(
            std::path::Path::new(output),
            cache.map(std::path::Path::new),
        )?;
    }
    if let Some(cache) = cache {
        if output.is_none_or(|output| std::path::Path::new(output) == std::path::Path::new(cache)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "shared stdlib cache entry requires a distinct build-owned archive output",
            ));
        }
    }
    Ok(())
}
