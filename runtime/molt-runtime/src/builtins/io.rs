use crate::PyToken;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(windows)]
use crate::windows_abi::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FILE_NAME_NORMALIZED, FILE_TYPE_CHAR,
    GetConsoleMode, GetCurrentProcess, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};

// Re-export path/glob/os functions so that `io::*` includes them
#[allow(unused_imports)]
pub use super::io_path::*;
pub(crate) use super::io_path_utils::*;
use crate::object::ops_encoding::DecodeFailure;
use crate::object::{
    MoltFileBackend, MoltMemoryBackend, MoltTextBackend, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF,
    NEWLINE_KIND_LF,
};
use crate::*;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[path = "io/buffer.rs"]
mod buffer;
pub(crate) use buffer::collect_bytes_like;
#[cfg(test)]
use buffer::file_remaining_bytes_hint;
use buffer::{
    backend_flush, backend_read_bytes, backend_seek, backend_tell, backend_truncate,
    backend_write_bytes, buffered_read_bytes, buffered_read_into, clear_read_buffer,
    clear_write_buffer, file_read1_bytes, flush_write_buffer, handle_read_byte,
    memory_backend_bits, memory_backend_vec_ref, prepend_read_buffer, rewind_read_buffer,
    unread_bytes,
};
#[path = "io/handle.rs"]
mod handle;
use handle::{
    VfsWritebackEntry, alloc_file_handle_with_state, resolve_file_handle_ptr,
    vfs_writeback_register, vfs_writeback_take,
};
pub(crate) use handle::{
    file_handle_close_detached, file_handle_close_ptr, file_handle_detached_message,
    file_handle_enter, file_handle_exit, file_handle_is_closed, file_handle_require_attached,
};
#[path = "io/text.rs"]
mod text;
use text::*;
#[path = "io/open.rs"]
mod open;
pub(crate) use open::dup_fd;
#[cfg(windows)]
use open::windows_handle_isatty;
#[cfg(windows)]
pub(crate) use open::windows_path_from_handle;
pub use open::{
    molt_file_io_init, molt_file_io_new, molt_file_open, molt_file_open_ex, molt_open_builtin,
    molt_sys_stderr, molt_sys_stdin, molt_sys_stdout,
};
use open::{open_arg_newline, reconfigure_arg_newline, reconfigure_arg_type};
#[path = "io/construct.rs"]
mod construct;
pub use construct::{
    molt_buffered_init, molt_buffered_new, molt_bytesio_init, molt_bytesio_new, molt_io_class,
    molt_stringio_init, molt_stringio_new, molt_text_io_wrapper_init, molt_text_io_wrapper_new,
};
#[path = "io/read.rs"]
mod read;
pub use read::{
    molt_file_getbuffer, molt_file_getvalue, molt_file_peek, molt_file_read, molt_file_read1,
    molt_file_readall, molt_file_readinto, molt_file_readinto1, molt_file_readline,
    molt_file_readlines,
};
#[path = "io/write.rs"]
mod write;
pub use write::{molt_file_close, molt_file_flush, molt_file_write, molt_file_writelines};
#[path = "io/control.rs"]
mod control;
pub use control::{
    molt_file_detach, molt_file_enter, molt_file_exit, molt_file_exit_method, molt_file_fileno,
    molt_file_isatty, molt_file_iter, molt_file_next, molt_file_readable, molt_file_reconfigure,
    molt_file_seek, molt_file_seekable, molt_file_tell, molt_file_truncate, molt_file_writable,
};

#[path = "io/attributes.rs"]
mod attributes;
pub(crate) use attributes::io_publish_members;
pub use attributes::molt_io_member_get;

const DEFAULT_BUFFER_SIZE: i64 = 8192;

pub(crate) struct IoRuntimeState {
    pub(crate) sys_stdin_handle_bits: AtomicU64,
    pub(crate) sys_stdout_handle_bits: AtomicU64,
    pub(crate) sys_stderr_handle_bits: AtomicU64,
    vfs_writebacks: Mutex<HashMap<usize, VfsWritebackEntry>>,
}

impl IoRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            sys_stdin_handle_bits: AtomicU64::new(0),
            sys_stdout_handle_bits: AtomicU64::new(0),
            sys_stderr_handle_bits: AtomicU64::new(0),
            vfs_writebacks: Mutex::new(HashMap::new()),
        }
    }

    fn stdio_slots(&self) -> [&AtomicU64; 3] {
        [
            &self.sys_stdin_handle_bits,
            &self.sys_stdout_handle_bits,
            &self.sys_stderr_handle_bits,
        ]
    }
}

pub(crate) fn io_clear_runtime_state(
    _py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    crate::gil_assert();
    let handles = state
        .io
        .stdio_slots()
        .map(|slot| slot.swap(0, Ordering::AcqRel));
    let writebacks = {
        let mut registry = state.io.vfs_writebacks.lock().unwrap();
        std::mem::take(&mut *registry)
    };
    let changed = handles.iter().any(|&bits| bits != 0) || !writebacks.is_empty();
    // Detach the entire owner cohort before flush/decref or backend destruction
    // can publish a replacement for the next shutdown fixed-point pass.
    for bits in handles {
        if bits != 0 && !obj_from_bits(bits).is_none() {
            let _ = molt_file_flush(bits);
            if exception_pending(_py) {
                clear_exception(_py);
            }
            dec_ref_bits(_py, bits);
        }
    }
    drop(writebacks);
    changed
}

fn raise_vfs_error(py: &PyToken<'_>, error: crate::vfs::VfsError, path: &str) -> u64 {
    let message = format!("{error}: '{path}'");
    let kind = match error {
        crate::vfs::VfsError::NotFound => "FileNotFoundError",
        crate::vfs::VfsError::AlreadyExists => "FileExistsError",
        crate::vfs::VfsError::PermissionDenied | crate::vfs::VfsError::ReadOnly => {
            "PermissionError"
        }
        crate::vfs::VfsError::IsDirectory => "IsADirectoryError",
        _ => "OSError",
    };
    raise_exception::<u64>(py, kind, &message)
}

#[cfg(test)]
mod tests {
    use super::{file_remaining_bytes_hint, io_clear_runtime_state, molt_sys_stdout};
    use crate::{clear_exception, dec_ref_bits, obj_from_bits, runtime_state};
    use std::fs::{File, remove_file};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    fn safe_temp_component(value: &str) -> String {
        value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let current_thread = std::thread::current();
        let thread_name = current_thread.name().unwrap_or("t");
        path.push(format!(
            "molt_io_{name}_{}_{}.bin",
            std::process::id(),
            safe_temp_component(thread_name)
        ));
        path
    }

    fn io_string(py: &crate::PyToken<'_>, text: &str) -> u64 {
        let ptr = crate::alloc_string(py, text.as_bytes());
        assert!(!ptr.is_null());
        crate::MoltObject::from_ptr(ptr).bits()
    }

    fn io_error(py: &crate::PyToken<'_>) -> (String, String) {
        assert!(crate::exception_pending(py));
        let error = crate::molt_exception_last();
        crate::molt_exception_clear();
        let kind = crate::type_name(py, obj_from_bits(error)).to_string();
        let message = crate::molt_exception_message(error);
        let text = crate::string_obj_to_owned(obj_from_bits(message)).unwrap();
        dec_ref_bits(py, message);
        dec_ref_bits(py, error);
        (kind, text)
    }

    fn selected_grants(required: &[&str], mask: usize) -> String {
        required
            .iter()
            .enumerate()
            .filter_map(|(index, name)| (mask & (1 << index) != 0).then_some(*name))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn assert_missing(py: &crate::PyToken<'_>, operation: &str, missing: &[&str]) {
        assert_eq!(
            io_error(py),
            (
                "PermissionError".to_string(),
                format!(
                    "missing [{}] capabilities for '{operation}'",
                    missing.join(", ")
                )
            )
        );
    }

    fn open_entry(entry: usize, path: u64, mode: u64, opener: u64) -> u64 {
        let none = crate::MoltObject::none().bits();
        let yes = crate::MoltObject::from_bool(true).bits();
        let buffering = crate::MoltObject::from_int(0).bits();
        match entry {
            0 => super::molt_file_open(path, mode),
            1 => super::molt_file_open_ex(path, mode, buffering, none, none, none, yes, opener),
            2 => super::molt_file_io_new(none, path, mode, yes, opener),
            3 => super::molt_open_builtin(path, mode, buffering, none, none, none, yes, opener),
            _ => unreachable!(),
        }
    }

    #[test]
    fn open_entrypoints_report_selected_mode_missing_set_without_mutation() {
        let required = ["fs.read", "fs.write"];
        for mask in 0..4 {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &selected_grants(&required, mask),
                |py| {
                    assert!(runtime_state(py).vfs_state.set(None).is_ok());
                    for entry in 0..4 {
                        for (mode, selected, operation) in [
                            ("rb", 1, "io.open.read"),
                            ("wb", 2, "io.open.write"),
                            ("ab", 2, "io.open.write"),
                            ("xb", 2, "io.open.write"),
                            ("r+b", 3, "io.open.readwrite"),
                            ("w+b", 3, "io.open.readwrite"),
                            ("a+b", 3, "io.open.readwrite"),
                            ("x+b", 3, "io.open.readwrite"),
                        ] {
                            let path = temp_path(&format!("admission_{mask}_{entry}_{mode}"));
                            let exclusive = mode.starts_with('x');
                            if exclusive {
                                assert!(!path.exists());
                            } else {
                                std::fs::write(&path, b"retained").unwrap();
                            }
                            let path_bits = io_string(py, path.to_str().unwrap());
                            let mode_bits = io_string(py, mode);
                            let handle = open_entry(
                                entry,
                                path_bits,
                                mode_bits,
                                crate::MoltObject::none().bits(),
                            );
                            let missing = required
                                .iter()
                                .enumerate()
                                .filter_map(|(index, name)| {
                                    (selected & (1 << index) != 0 && mask & (1 << index) == 0)
                                        .then_some(*name)
                                })
                                .collect::<Vec<_>>();
                            if missing.is_empty() {
                                assert!(!crate::exception_pending(py), "{mode} entry {entry}");
                                assert!(!obj_from_bits(handle).is_none());
                                super::molt_file_close(handle);
                                dec_ref_bits(py, handle);
                            } else {
                                assert_missing(py, operation, &missing);
                                if exclusive {
                                    assert!(!path.exists());
                                } else {
                                    assert_eq!(std::fs::read(&path).unwrap(), b"retained");
                                }
                            }
                            dec_ref_bits(py, path_bits);
                            dec_ref_bits(py, mode_bits);
                            if path.exists() {
                                remove_file(path).unwrap();
                            }
                        }
                    }
                },
            );
        }
    }

    #[test]
    fn path_makedirs_reports_all_missing_grants_before_creating_parents() {
        let required = ["fs.read", "fs.write"];
        for mask in 0..4 {
            let root = temp_path(&format!("makedirs_{mask}"));
            assert!(!root.exists());
            let target = root.join("nested/leaf");
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &selected_grants(&required, mask),
                |py| {
                    let path = io_string(py, target.to_str().unwrap());
                    super::molt_path_makedirs(
                        path,
                        crate::MoltObject::from_int(0o777).bits(),
                        crate::MoltObject::from_bool(false).bits(),
                    );
                    let missing = required
                        .iter()
                        .enumerate()
                        .filter_map(|(index, name)| (mask & (1 << index) == 0).then_some(*name))
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        assert!(!crate::exception_pending(py));
                        assert!(target.is_dir());
                    } else {
                        assert_missing(py, "path.makedirs", &missing);
                        assert!(!root.exists());
                    }
                    dec_ref_bits(py, path);
                },
            );
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    static FSPATH_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static OPENER_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn selected_tmp_path(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            FSPATH_CALLS.fetch_add(1, Ordering::Relaxed);
            io_string(py, "/tmp/selected")
        })
    }

    extern "C" fn forbidden_opener(_path: u64, _flags: u64) -> u64 {
        OPENER_CALLS.fetch_add(1, Ordering::Relaxed);
        crate::MoltObject::from_int(-1).bits()
    }

    fn test_callable(py: &crate::PyToken<'_>, address: *const (), arity: u64) -> u64 {
        let ptr = crate::alloc_function_obj(
            py,
            crate::provenance::abi::expose_function_address(address),
            arity,
        );
        assert!(!ptr.is_null());
        unsafe {
            crate::object::layout::function_set_call_target_ptr(ptr, address);
        }
        crate::MoltObject::from_ptr(ptr).bits()
    }

    fn fspath_instance(py: &crate::PyToken<'_>) -> u64 {
        let function = test_callable(py, selected_tmp_path as *const (), 1);
        let name = io_string(py, "SelectedTmpPath");
        let method = io_string(py, "__fspath__");
        let namespace =
            crate::MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[method, function]))
                .bits();
        let bases = crate::MoltObject::from_ptr(crate::alloc_tuple(
            py,
            &[crate::builtin_classes(py).object],
        ))
        .bits();
        let class = crate::builtins::types::molt_type_new(
            crate::builtin_classes(py).type_obj,
            name,
            bases,
            namespace,
            crate::MoltObject::none().bits(),
        );
        assert!(!crate::exception_pending(py));
        let instance =
            unsafe { crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
        for bits in [function, name, method, namespace, bases, class] {
            dec_ref_bits(py, bits);
        }
        instance
    }

    fn tmp_mount(py: &crate::PyToken<'_>) -> std::sync::Arc<crate::vfs::tmp::TmpFs> {
        let backend = std::sync::Arc::new(crate::vfs::tmp::TmpFs::new(1));
        let mut table = crate::vfs::MountTable::new();
        table.add_mount("/tmp", backend.clone());
        assert!(
            runtime_state(py)
                .vfs_state
                .set(Some(crate::vfs::VfsState::from_table(table)))
                .is_ok()
        );
        backend
    }

    #[test]
    fn selected_vfs_readwrite_aggregates_all_grants_after_fspath_without_mutation() {
        use crate::vfs::VfsBackend;
        let required = ["fs.read", "fs.write", "fs.tmp.read", "fs.tmp.write"];
        for mask in 0..16 {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &selected_grants(&required, mask),
                |py| {
                    let backend = tmp_mount(py);
                    for pathlike in [false, true] {
                        backend.open_write("selected", b"retained").unwrap();
                        FSPATH_CALLS.store(0, Ordering::Relaxed);
                        let path = if pathlike {
                            fspath_instance(py)
                        } else {
                            io_string(py, "/tmp/selected")
                        };
                        let mode = io_string(py, "r+b");
                        let handle = open_entry(3, path, mode, crate::MoltObject::none().bits());
                        assert_eq!(FSPATH_CALLS.load(Ordering::Relaxed), usize::from(pathlike));
                        let missing = required
                            .iter()
                            .enumerate()
                            .filter_map(|(index, name)| (mask & (1 << index) == 0).then_some(*name))
                            .collect::<Vec<_>>();
                        if missing.is_empty() {
                            assert!(!crate::exception_pending(py));
                            let data = super::molt_file_read(
                                handle,
                                crate::MoltObject::from_int(-1).bits(),
                            );
                            assert_eq!(
                                unsafe { super::collect_bytes_like(py, data) }.unwrap(),
                                b"retained"
                            );
                            dec_ref_bits(py, data);
                            let suffix =
                                crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"!")).bits();
                            super::molt_file_write(handle, suffix);
                            dec_ref_bits(py, suffix);
                            super::molt_file_close(handle);
                            assert!(!crate::exception_pending(py));
                            dec_ref_bits(py, handle);
                            assert_eq!(backend.open_read("selected").unwrap(), b"retained!");
                        } else {
                            assert_missing(py, "io.open.readwrite", &missing);
                            assert_eq!(backend.open_read("selected").unwrap(), b"retained");
                        }
                        for bits in [path, mode] {
                            dec_ref_bits(py, bits);
                        }
                    }
                },
            );
        }
    }

    #[test]
    fn denied_custom_opener_is_never_called_and_requires_no_mount_grants() {
        for mask in 0..3 {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &selected_grants(&["fs.read", "fs.write"], mask),
                |py| {
                    let _backend = tmp_mount(py);
                    let path = io_string(py, "/tmp/selected");
                    let mode = io_string(py, "r+b");
                    let opener = test_callable(py, forbidden_opener as *const (), 2);
                    OPENER_CALLS.store(0, Ordering::Relaxed);
                    open_entry(3, path, mode, opener);
                    let required = ["fs.read", "fs.write"];
                    let missing = required
                        .iter()
                        .enumerate()
                        .filter_map(|(index, name)| (mask & (1 << index) == 0).then_some(*name))
                        .collect::<Vec<_>>();
                    assert_missing(py, "io.open.readwrite", &missing);
                    assert_eq!(OPENER_CALLS.load(Ordering::Relaxed), 0);
                    for bits in [path, mode, opener] {
                        dec_ref_bits(py, bits);
                    }
                },
            );
        }
    }

    #[test]
    fn bundle_missing_set_precedes_readonly_and_invalid_mode_precedes_admission() {
        let required = ["fs.read", "fs.write", "fs.bundle.read"];
        for mask in 0..8 {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                &selected_grants(&required, mask),
                |py| {
                    let mut table = crate::vfs::MountTable::new();
                    table.add_mount(
                        "/bundle",
                        std::sync::Arc::new(crate::vfs::bundle::BundleFs::from_entries(vec![(
                            "selected".to_string(),
                            b"retained".to_vec(),
                        )])),
                    );
                    assert!(
                        runtime_state(py)
                            .vfs_state
                            .set(Some(crate::vfs::VfsState::from_table(table)))
                            .is_ok()
                    );
                    let path = io_string(py, "/bundle/selected");
                    let mode = io_string(py, "r+b");
                    open_entry(3, path, mode, crate::MoltObject::none().bits());
                    let missing = required
                        .iter()
                        .enumerate()
                        .filter_map(|(index, name)| (mask & (1 << index) == 0).then_some(*name))
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        assert_eq!(
                            io_error(py),
                            (
                                "PermissionError".to_string(),
                                "Read-only file system".to_string()
                            )
                        );
                    } else {
                        assert_missing(py, "io.open.readwrite", &missing);
                    }
                    let invalid = io_string(py, "rw");
                    open_entry(3, path, invalid, crate::MoltObject::none().bits());
                    assert_eq!(io_error(py).0, "ValueError");
                    for bits in [path, mode, invalid] {
                        dec_ref_bits(py, bits);
                    }
                },
            );
        }
    }

    #[test]
    fn dev_append_only_uses_write_adapter_but_readwrite_still_refuses_reads() {
        for grants in ["fs.write", "fs.read,fs.write"] {
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
                let backend = std::sync::Arc::new(crate::vfs::dev::DevFs::new());
                let mut table = crate::vfs::MountTable::new();
                table.add_mount("/dev", backend.clone());
                assert!(
                    runtime_state(py)
                        .vfs_state
                        .set(Some(crate::vfs::VfsState::from_table(table)))
                        .is_ok()
                );
                for (path_name, mode_name, expected_error) in [
                    ("/dev/stdout", "ab", None),
                    ("/dev/stderr", "ab", None),
                    ("/dev/stdin", "ab", Some("PermissionError")),
                    ("/dev", "ab", Some("IsADirectoryError")),
                    ("/dev/missing", "ab", Some("FileNotFoundError")),
                ] {
                    let path = io_string(py, path_name);
                    let mode = io_string(py, mode_name);
                    let handle = open_entry(3, path, mode, crate::MoltObject::none().bits());
                    if let Some(kind) = expected_error {
                        assert_eq!(io_error(py).0, kind);
                    } else {
                        assert!(!crate::exception_pending(py));
                        let data =
                            crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"stream")).bits();
                        super::molt_file_write(handle, data);
                        super::molt_file_close(handle);
                        assert!(!crate::exception_pending(py));
                        for bits in [handle, data] {
                            dec_ref_bits(py, bits);
                        }
                    }
                    for bits in [path, mode] {
                        dec_ref_bits(py, bits);
                    }
                }
                assert_eq!(backend.take_stdout(), b"stream");
                assert_eq!(backend.take_stderr(), b"stream");
                for mode_name in ["r+b", "a+b"] {
                    let path = io_string(py, "/dev/stdout");
                    let mode = io_string(py, mode_name);
                    open_entry(3, path, mode, crate::MoltObject::none().bits());
                    if grants == "fs.write" {
                        assert_missing(py, "io.open.readwrite", &["fs.read"]);
                    } else {
                        assert_eq!(
                            io_error(py),
                            (
                                "PermissionError".to_string(),
                                "Permission denied: '/dev/stdout'".to_string()
                            )
                        );
                    }
                    for bits in [path, mode] {
                        dec_ref_bits(py, bits);
                    }
                }
                assert!(backend.take_stdout().is_empty());
            });
        }
    }

    #[test]
    fn vfs_modes_retain_append_truncate_and_exclusive_semantics() {
        use crate::vfs::VfsBackend;
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
            "fs.write,fs.tmp.write",
            |py| {
                let backend = tmp_mount(py);
                for (mode, initial, expected) in [
                    ("wb", Some(&b"old"[..]), &b"new"[..]),
                    ("ab", Some(&b"old"[..]), &b"oldnew"[..]),
                    ("ab", None, &b"new"[..]),
                    ("xb", None, &b"new"[..]),
                ] {
                    if backend.exists("selected") {
                        backend.unlink("selected").unwrap();
                    }
                    if let Some(bytes) = initial {
                        backend.open_write("selected", bytes).unwrap();
                    }
                    let path = io_string(py, "/tmp/selected");
                    let mode_bits = io_string(py, mode);
                    let handle = open_entry(3, path, mode_bits, crate::MoltObject::none().bits());
                    assert!(!crate::exception_pending(py), "{mode}");
                    assert_eq!(
                        super::molt_file_readable(handle),
                        crate::MoltObject::from_bool(false).bits()
                    );
                    let data = crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"new")).bits();
                    super::molt_file_write(handle, data);
                    super::molt_file_close(handle);
                    assert!(!crate::exception_pending(py));
                    assert_eq!(backend.open_read("selected").unwrap(), expected);
                    for bits in [handle, data, path, mode_bits] {
                        dec_ref_bits(py, bits);
                    }
                }
                let path = io_string(py, "/tmp/selected");
                let mode = io_string(py, "xb");
                open_entry(3, path, mode, crate::MoltObject::none().bits());
                assert_eq!(io_error(py).0, "FileExistsError");
                assert_eq!(backend.open_read("selected").unwrap(), b"new");
                for bits in [path, mode] {
                    dec_ref_bits(py, bits);
                }
            },
        );
    }

    #[test]
    fn vfs_close_reports_quota_failure_and_retires_writeback_once() {
        use crate::vfs::VfsBackend;
        for device in [false, true] {
            for text in [false, true] {
                crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
                    "fs.write,fs.tmp.write",
                    |py| {
                        let tmp = std::sync::Arc::new(crate::vfs::tmp::TmpFs::new(0));
                        let dev = std::sync::Arc::new(crate::vfs::dev::DevFs::new());
                        let mut table = crate::vfs::MountTable::new();
                        if device {
                            // Actual device capacity: existing bytes must survive
                            // the rejected close, not merely an injected error.
                            dev.open_write("stdout", &vec![b'x'; 16 * 1024 * 1024])
                                .unwrap();
                            table.add_mount("/dev", dev.clone());
                        } else {
                            table.add_mount("/tmp", tmp.clone());
                        }
                        assert!(
                            runtime_state(py)
                                .vfs_state
                                .set(Some(crate::vfs::VfsState::from_table(table)))
                                .is_ok()
                        );
                        let path = io_string(
                            py,
                            if device {
                                "/dev/stdout"
                            } else {
                                "/tmp/selected"
                            },
                        );
                        let mode = io_string(py, if text { "w" } else { "wb" });
                        let handle = super::molt_file_open(path, mode);
                        assert!(!crate::exception_pending(py));
                        let ptr = obj_from_bits(handle).as_ptr().unwrap();
                        let (key, child) = unsafe {
                            let raw = &*crate::file_handle_ptr(ptr);
                            (std::sync::Arc::as_ptr(&raw.state) as usize, raw.buffer_bits)
                        };
                        assert!(
                            runtime_state(py)
                                .io
                                .vfs_writebacks
                                .lock()
                                .unwrap()
                                .contains_key(&key)
                        );
                        let data = if text {
                            io_string(py, "new")
                        } else {
                            crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"new")).bits()
                        };
                        super::molt_file_write(handle, data);
                        assert!(!crate::exception_pending(py));
                        super::molt_file_close(handle);
                        assert_eq!(
                            io_error(py),
                            (
                                "OSError".to_string(),
                                format!(
                                    "Quota exceeded: '{}'",
                                    if device { "stdout" } else { "selected" }
                                )
                            )
                        );
                        assert!(unsafe {
                            super::file_handle_is_closed(&*crate::file_handle_ptr(ptr))
                        });
                        assert!(
                            !runtime_state(py)
                                .io
                                .vfs_writebacks
                                .lock()
                                .unwrap()
                                .contains_key(&key)
                        );
                        if device {
                            let original = dev.take_stdout();
                            assert_eq!(original.len(), 16 * 1024 * 1024);
                            assert!(original.iter().all(|&byte| byte == b'x'));
                        } else {
                            assert!(!tmp.exists("selected"));
                        }
                        // Draining the device frees capacity: replay would now
                        // succeed and become observable, even if errors vanished.
                        super::molt_file_close(handle);
                        if child != 0 {
                            super::molt_file_close(child);
                        }
                        assert!(!crate::exception_pending(py));
                        assert!(dev.take_stdout().is_empty());
                        assert!(!tmp.exists("selected"));
                        super::molt_file_write(handle, data);
                        assert_eq!(io_error(py).0, "ValueError");
                        for bits in [data, handle, mode, path] {
                            dec_ref_bits(py, bits);
                        }
                    },
                );
            }
        }
    }

    #[test]
    fn file_remaining_bytes_hint_tracks_stream_position() {
        let path = temp_path("reserve_hint");
        let mut writer = File::create(&path).expect("create temp file");
        writer.write_all(&[1u8; 16]).expect("write temp file");
        drop(writer);

        let mut file = File::open(&path).expect("open temp file");
        assert_eq!(file_remaining_bytes_hint(&mut file), Some(16));
        file.seek(SeekFrom::Start(5)).expect("seek temp file");
        assert_eq!(file_remaining_bytes_hint(&mut file), Some(11));

        let _ = remove_file(path);
    }

    #[test]
    fn cached_stdio_handles_are_runtime_owned_and_clearable() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            io_clear_runtime_state(_py, state);
            clear_exception(_py);

            let stdout_bits = molt_sys_stdout();
            assert!(!obj_from_bits(stdout_bits).is_none());
            assert_eq!(
                state.io.sys_stdout_handle_bits.load(Ordering::Acquire),
                stdout_bits
            );

            dec_ref_bits(_py, stdout_bits);
            io_clear_runtime_state(_py, state);
            assert_eq!(state.io.sys_stdout_handle_bits.load(Ordering::Acquire), 0);
        });
    }

    #[cfg(unix)]
    #[test]
    fn host_descriptor_state_releases_without_closing_the_descriptor() {
        use crate::object::{MoltFileBackend, MoltFileState};
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::sync::Mutex;

        let path = temp_path("host_descriptor");
        let mut owner = File::create(&path).expect("create temp file");
        let fd = owner.as_raw_fd();
        // A second wrapper of one descriptor, as WASI standard streams hold.
        let borrowed = unsafe { File::from_raw_fd(fd) };
        drop(MoltFileState {
            backend: Mutex::new(Some(MoltFileBackend::File(borrowed))),
            host_descriptor: true,
        });
        assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
        owner
            .write_all(b"still open")
            .expect("the host descriptor stays usable");
        drop(owner);
        let _ = remove_file(path);
    }
}
