use super::*;
use crate::test_support::RuntimeTestTransaction;

const RETIRED_SELECTORS: [&str; 7] = [
    "MOLT_PYTHON_VERSION",
    "MOLT_SYS_VERSION_INFO",
    "MOLT_SYS_VERSION",
    "MOLT_SYS_API_VERSION",
    "MOLT_SYS_ABIFLAGS",
    "MOLT_SYS_IMPLEMENTATION_NAME",
    "MOLT_SYS_CACHE_TAG",
];

struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl RestoreEnvironment {
    fn capture() -> Self {
        Self(
            RETIRED_SELECTORS
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        )
    }
}

impl Drop for RestoreEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn publish(py: &PyToken<'_>, major: i64, minor: i64, micro: i64, release: &str, serial: i64) {
    let release = ExceptionValue::adopt(
        py,
        MoltObject::from_ptr(alloc_string(py, release.as_bytes())).bits(),
    );
    assert!(obj_from_bits(release.bits()).as_ptr().is_some());
    let result = molt_sys_set_version_info(
        MoltObject::from_int(major).bits(),
        MoltObject::from_int(minor).bits(),
        MoltObject::from_int(micro).bits(),
        release.bits(),
        MoltObject::from_int(serial).bits(),
    );
    dec_ref_bits(py, result);
}

fn assert_display(py: &PyToken<'_>, expected: &str) {
    let version = ExceptionValue::adopt(py, molt_sys_version());
    assert!(!exception_pending(py));
    assert_eq!(
        string_obj_to_owned(obj_from_bits(version.bits())).as_deref(),
        Some(expected)
    );
}

#[test]
fn compiled_target_ignores_retired_environment_before_and_after_publication() {
    // These are independent public CPython-minor controls, not a generated
    // table iterated against itself. The application setter is the producer.
    for minor in [12, 13, 14] {
        RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
            let _environment = RestoreEnvironment::capture();
            unsafe {
                std::env::set_var("MOLT_PYTHON_VERSION", "4.99");
                std::env::set_var("MOLT_SYS_VERSION_INFO", "4,99,7,final,0");
                std::env::set_var("MOLT_SYS_VERSION", "foreign interpreter");
                std::env::set_var("MOLT_SYS_API_VERSION", "9999");
                std::env::set_var("MOLT_SYS_ABIFLAGS", "foreign");
                std::env::set_var("MOLT_SYS_IMPLEMENTATION_NAME", "foreign");
                std::env::set_var("MOLT_SYS_CACHE_TAG", "foreign-499");
            }
            assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
            crate::with_gil_entry_nopanic!(py, {
                let state = runtime_state(py);
                assert!(state.sys_version_info.lock().unwrap().is_none());
                assert_eq!(runtime_target_minor(py), 12);
                assert_display(py, &format_sys_version(default_sys_version_info()));
                assert!(state.sys_version_info.lock().unwrap().is_none());
                publish(py, 3, minor, 0, "final", 0);
                assert!(!exception_pending(py));

                // Replay absent, conflicting, malformed and unsupported selectors
                // without rebuilding or resetting the committed target.
                for selector in [None, Some("3.12"), Some("malformed"), Some("4.99")] {
                    for key in RETIRED_SELECTORS {
                        unsafe {
                            match selector {
                                Some(value) => std::env::set_var(key, value),
                                None => std::env::remove_var(key),
                            }
                        }
                    }
                    let (info, provisional) = current_sys_version_info(state);
                    assert!(!provisional);
                    assert_eq!((info.major, info.minor, info.micro), (3, minor, 0));
                    assert_eq!((info.releaselevel.as_str(), info.serial), ("final", 0));
                    assert_display(py, &format!("3.{minor}.0 (molt)"));
                    let hexversion = molt_sys_hexversion();
                    assert_eq!(
                        to_i64(obj_from_bits(hexversion)),
                        Some((3 << 24) | (minor << 16) | 0xf0)
                    );
                    dec_ref_bits(py, hexversion);
                    assert_eq!(sys_api_version(), 1013);
                    assert_eq!(sys_abiflags(), "");
                    assert_eq!(sys_implementation_name(), "molt");
                    assert_eq!(
                        sys_cache_tag(sys_implementation_name(), &info),
                        format!("molt-3{minor}")
                    );
                    publish(py, 3, minor, 0, "final", 0);
                    assert!(!exception_pending(py));
                }
            });
        });
    }
}

#[test]
fn first_default_target_publication_is_final_before_sys_import() {
    RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            assert!(state.sys_version_info.lock().unwrap().is_none());
            assert!(crate::builtins::modules::interpreter_sys_module(py).is_none());
            // Reads do not seal the bootstrap default.
            assert_eq!(runtime_target_minor(py), 12);
            assert!(runtime_target_at_least(py, 3, 12));
            assert_display(py, &format_sys_version(default_sys_version_info()));
            assert!(state.sys_version_info.lock().unwrap().is_none());
            let default = default_sys_version_info();
            publish(
                py,
                default.major,
                default.minor,
                default.micro,
                &default.releaselevel,
                default.serial,
            );
            assert!(!exception_pending(py));
            assert!(state.sys_version_info.lock().unwrap().is_some());
            publish(py, 3, 13, 0, "final", 0);
            assert!(exception_pending(py));
            clear_exception(py);
            assert_eq!(runtime_target_minor(py), 12);
            assert_display(py, &format_sys_version(default));
        });
    });
}

#[test]
fn rejected_target_or_release_never_commits_bootstrap_state() {
    RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
        crate::with_gil_entry_nopanic!(py, {
            let state = runtime_state(py);
            for (major, minor, release) in [
                (4, 12, "final"),
                (3, 11, "final"),
                (3, 15, "final"),
                (3, 12, "unknown"),
                (-1, 12, "final"),
            ] {
                publish(py, major, minor, 0, release, 0);
                assert!(exception_pending(py));
                clear_exception(py);
                assert!(state.sys_version_info.lock().unwrap().is_none());
                assert_display(py, &format_sys_version(default_sys_version_info()));
            }
            publish(py, 3, 14, 0, "final", 0);
            assert!(!exception_pending(py));
            assert_eq!(runtime_target_minor(py), 14);
        });
    });
}
