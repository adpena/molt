use super::{file_remaining_bytes_hint, io_clear_runtime_state, molt_sys_stdout};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{clear_exception, dec_ref_bits, obj_from_bits, runtime_state};
use std::fs::{File, remove_file};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

struct MetadataBackend {
    inner: crate::vfs::tmp::TmpFs,
    stats: std::sync::atomic::AtomicUsize,
    reads: std::sync::atomic::AtomicUsize,
}
impl MetadataBackend {
    fn new() -> std::sync::Arc<Self> {
        use crate::vfs::VfsBackend;
        let backend = std::sync::Arc::new(Self {
            inner: crate::vfs::tmp::TmpFs::new(1),
            stats: std::sync::atomic::AtomicUsize::new(0),
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        backend.inner.open_write("value", b"fixture").unwrap();
        backend.inner.open_write("other", b"second").unwrap();
        backend
            .inner
            .open_write("valid-\u{fffd}", b"unicode")
            .unwrap();
        backend
    }
}
impl crate::vfs::VfsBackend for MetadataBackend {
    fn open_read(&self, path: &str) -> Result<Vec<u8>, crate::vfs::VfsError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.open_read(path)
    }
    fn open_write(&self, path: &str, data: &[u8]) -> Result<(), crate::vfs::VfsError> {
        self.inner.open_write(path, data)
    }
    fn open_append(&self, path: &str, data: &[u8]) -> Result<(), crate::vfs::VfsError> {
        self.inner.open_append(path, data)
    }
    fn stat(&self, path: &str) -> Result<crate::vfs::VfsStat, crate::vfs::VfsError> {
        self.stats.fetch_add(1, Ordering::Relaxed);
        self.inner.stat(path)
    }
    fn readdir(&self, path: &str) -> Result<Vec<String>, crate::vfs::VfsError> {
        self.inner.readdir(path)
    }
    fn mkdir(&self, path: &str) -> Result<(), crate::vfs::VfsError> {
        self.inner.mkdir(path)
    }
    fn unlink(&self, path: &str) -> Result<(), crate::vfs::VfsError> {
        self.inner.unlink(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), crate::vfs::VfsError> {
        self.inner.rename(from, to)
    }
    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }
    fn is_readonly(&self) -> bool {
        self.inner.is_readonly()
    }
}

fn metadata_mount(backend: std::sync::Arc<MetadataBackend>) -> crate::vfs::VfsState {
    let mut table = crate::vfs::MountTable::new();
    table.add_mount("/tmp", backend);
    crate::vfs::VfsState::from_table(table)
}

fn assert_metadata_error(py: &crate::PyToken<'_>, class: &str, errno: i32) {
    let error = crate::molt_exception_last();
    assert_eq!(crate::type_name(py, obj_from_bits(error)), class);
    dec_ref_bits(py, error);
    assert_eq!(io_error_errno(py), i64::from(errno));
}

#[test]
fn vfs_metadata_admission_and_call_budget() {
    enum Observation {
        Stat,
        Predicate(bool),
        ReadlinkError,
    }
    let required = ["fs.read", "fs.tmp.read"];
    for mask in 0..4 {
        let backend = MetadataBackend::new();
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&required, mask),
            Some(metadata_mount(backend.clone())),
            |py| {
                let path = io_string(py, "/tmp/value");
                let operations: [(fn(u64) -> u64, Observation); 13] = [
                    (|p| crate::molt_os_stat(p), Observation::Stat),
                    (|p| crate::molt_os_lstat(p), Observation::Stat),
                    (|p| crate::molt_path_exists(p), Observation::Predicate(true)),
                    (|p| crate::molt_path_isfile(p), Observation::Predicate(true)),
                    (|p| crate::molt_path_isdir(p), Observation::Predicate(false)),
                    (
                        |p| crate::molt_path_islink(p),
                        Observation::Predicate(false),
                    ),
                    (|p| crate::molt_path_readlink(p), Observation::ReadlinkError),
                    (
                        |p| {
                            crate::molt_os_stat_at(
                                p,
                                crate::MoltObject::none().bits(),
                                crate::MoltObject::from_bool(true).bits(),
                            )
                        },
                        Observation::Stat,
                    ),
                    (
                        |p| {
                            crate::molt_os_stat_at(
                                p,
                                crate::MoltObject::from_int(-1).bits(),
                                crate::MoltObject::from_bool(false).bits(),
                            )
                        },
                        Observation::Stat,
                    ),
                    (
                        |p| crate::molt_os_lstat_at(p, crate::MoltObject::none().bits()),
                        Observation::Stat,
                    ),
                    (
                        |p| crate::molt_os_lstat_at(p, crate::MoltObject::from_int(-1).bits()),
                        Observation::Stat,
                    ),
                    (
                        |p| crate::molt_os_readlink_at(p, crate::MoltObject::none().bits()),
                        Observation::ReadlinkError,
                    ),
                    (
                        |p| crate::molt_os_readlink_at(p, crate::MoltObject::from_int(-1).bits()),
                        Observation::ReadlinkError,
                    ),
                ];
                for (operation, expected) in operations {
                    let before = backend.stats.load(Ordering::Relaxed);
                    let result = operation(path);
                    if mask == 3 {
                        assert_eq!(backend.stats.load(Ordering::Relaxed) - before, 1);
                        match expected {
                            Observation::ReadlinkError => {
                                assert_metadata_error(py, "OSError", libc::EINVAL)
                            }
                            Observation::Stat => {
                                assert!(!crate::exception_pending(py));
                                let pointer = obj_from_bits(result).as_ptr().unwrap();
                                assert_eq!(
                                    unsafe { crate::object::layout::tuple_storage_len(pointer) },
                                    10
                                );
                                let fields = unsafe {
                                    std::slice::from_raw_parts(
                                        crate::object::layout::tuple_storage_items(pointer),
                                        10,
                                    )
                                };
                                assert_eq!(crate::to_i64(obj_from_bits(fields[6])), Some(7));
                                assert_eq!(
                                    crate::to_i64(obj_from_bits(fields[0])).unwrap() & 0o170000,
                                    0o100000
                                );
                                dec_ref_bits(py, result);
                            }
                            Observation::Predicate(value) => {
                                assert!(!crate::exception_pending(py));
                                assert_eq!(obj_from_bits(result).as_bool(), Some(value));
                                dec_ref_bits(py, result);
                            }
                        }
                    } else {
                        assert_eq!(backend.stats.load(Ordering::Relaxed), before);
                        let (class, message) = io_error(py);
                        assert_eq!(class, "PermissionError");
                        for (bit, name) in required.iter().enumerate() {
                            assert_eq!(message.contains(name), mask & (1 << bit) == 0, "{message}");
                        }
                    }
                    assert_eq!(backend.reads.load(Ordering::Relaxed), 0);
                }
                dec_ref_bits(py, path);
            },
        );
    }
}

#[test]
fn vfs_metadata_aliases_and_errno_domain() {
    let backend = MetadataBackend::new();
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
        "fs.read,fs.tmp.read",
        Some(metadata_mount(backend.clone())),
        |py| {
            for (name, operation, class, errno) in [
                (
                    "/tmp/missing",
                    crate::molt_os_stat as extern "C" fn(u64) -> u64,
                    "FileNotFoundError",
                    libc::ENOENT,
                ),
                (
                    "/tmp/missing",
                    crate::molt_path_readlink,
                    "FileNotFoundError",
                    libc::ENOENT,
                ),
                (
                    "/tmp/value",
                    crate::molt_path_readlink,
                    "OSError",
                    libc::EINVAL,
                ),
            ] {
                let path = io_string(py, name);
                let before = backend.stats.load(Ordering::Relaxed);
                operation(path);
                assert_metadata_error(py, class, errno);
                assert_eq!(backend.stats.load(Ordering::Relaxed) - before, 1);
                dec_ref_bits(py, path);
            }
            // A real U+FFFD name exists. Raw non-UTF8 bytes must not alias it.
            let invalid =
                crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"/tmp/valid-\xff")).bits();
            let before = backend.stats.load(Ordering::Relaxed);
            crate::molt_os_stat(invalid);
            assert_eq!(io_error(py).0, "UnicodeError");
            assert_eq!(backend.stats.load(Ordering::Relaxed), before);
            dec_ref_bits(py, invalid);
            let valid = io_string(py, "/tmp/valid-\u{fffd}");
            let result = crate::molt_os_stat(valid);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, result);
            dec_ref_bits(py, valid);
            assert_eq!(backend.reads.load(Ordering::Relaxed), 0);
        },
    );
}

#[test]
fn vfs_metadata_samefile_owner_identity() {
    let required = ["fs.read", "fs.tmp.read", "fs.state.read", "fs.bundle.read"];
    for mask in 0..16 {
        let shared = MetadataBackend::new();
        let distinct = MetadataBackend::new();
        let mut table = crate::vfs::MountTable::new();
        table.add_mount("/tmp", shared.clone());
        table.add_mount("/state", shared.clone());
        table.add_mount("/bundle", distinct.clone());
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&required, mask),
            Some(crate::vfs::VfsState::from_table(table)),
            |py| {
                let left = io_string(py, "/tmp/value");
                // Independent policy oracle: both paths need generic read and
                // their own mount grants, even when the backend owner is shared.
                for (name, expected, needed) in [
                    ("/state/value", true, 0b0111),
                    ("/tmp/other", false, 0b0011),
                    ("/bundle/value", false, 0b1011),
                ] {
                    let right = io_string(py, name);
                    for operation in [
                        crate::molt_os_path_samefile as extern "C" fn(u64, u64) -> u64,
                        crate::molt_pathlib_samefile,
                    ] {
                        let before = shared.stats.load(Ordering::Relaxed)
                            + distinct.stats.load(Ordering::Relaxed);
                        let result = operation(left, right);
                        let calls = shared.stats.load(Ordering::Relaxed)
                            + distinct.stats.load(Ordering::Relaxed)
                            - before;
                        if mask & needed == needed {
                            assert_eq!(calls, 2);
                            assert!(!crate::exception_pending(py));
                            assert_eq!(obj_from_bits(result).as_bool(), Some(expected));
                            dec_ref_bits(py, result);
                        } else {
                            assert_eq!(calls, 0);
                            let (class, message) = io_error(py);
                            assert_eq!(class, "PermissionError");
                            for (bit, name) in required.iter().enumerate() {
                                assert_eq!(
                                    message.contains(name),
                                    needed & (1 << bit) != 0 && mask & (1 << bit) == 0,
                                    "{message}"
                                );
                            }
                        }
                    }
                    dec_ref_bits(py, right);
                }
                dec_ref_bits(py, left);
                assert_eq!(shared.reads.load(Ordering::Relaxed), 0);
                assert_eq!(distinct.reads.load(Ordering::Relaxed), 0);
            },
        );
    }
}

#[test]
fn iobase_finalization_uses_published_base_and_preserves_pending_error() {
    use std::sync::atomic::AtomicUsize;
    static CLOSES: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn close() -> u64 {
        CLOSES.fetch_add(1, Ordering::SeqCst);
        crate::MoltObject::none().bits()
    }
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        CLOSES.store(0, Ordering::SeqCst);
        // Do not fetch __del__: retirement must discover the declaration
        // even when no caller has requested that method or class dict.
        let class = crate::builtin_classes(py).io_base;
        let stream = unsafe { crate::call_callable0(py, class) };
        assert!(!crate::exception_pending(py));
        let method = crate::builtins::methods::alloc_builtin_function(py, fn_addr!(close), 0);
        let name = io_string(py, "close");
        crate::molt_setattr_builtin(stream, name, method);
        assert!(!crate::exception_pending(py));
        dec_ref_bits(py, method);
        dec_ref_bits(py, name);
        let pending = crate::alloc_exception(py, "ValueError", "incoming IO finalization");
        assert!(!pending.is_null());
        crate::record_exception(py, pending);
        dec_ref_bits(py, stream);
        assert_eq!(CLOSES.load(Ordering::SeqCst), 1);
        let observed = crate::molt_exception_last_pending();
        assert_eq!(observed, crate::MoltObject::from_ptr(pending).bits());
        dec_ref_bits(py, observed);
        clear_exception(py);
        dec_ref_bits(py, crate::MoltObject::from_ptr(pending).bits());
    });
}

#[test]
fn buffered_close_observes_borrowed_backend_child_without_retiring_host_owner() {
    use crate::{MoltObject, object::MoltFileState};
    use std::sync::{Arc, Mutex};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = crate::builtin_classes(py);
        for close_child in [false, true] {
            let path = temp_path("borrowed_close");
            let file = File::create(&path).unwrap();
            #[cfg(windows)]
            let crt_fd = {
                use std::os::windows::io::AsRawHandle;
                super::windows_crt_fd_from_handle(file.as_raw_handle(), false, true, false)
                    .expect("fixture file owns its CRT descriptor")
            };
            let state = Arc::new(MoltFileState {
                backend: Mutex::new(Some(crate::object::MoltFileBackend::File(file))),
                #[cfg(windows)]
                crt_fd: Mutex::new(Some(crt_fd)),
                host_descriptor: false,
            });
            // The enclosing test owns the backend, just as the WASI host
            // owns stdio. Neither Python handle may retire that resource.
            let raw = super::alloc_file_handle_with_state(
                py,
                Some(Arc::clone(&state)),
                true,
                false,
                false,
                false,
                false,
                false,
                false,
                0,
                classes.file_io,
                MoltObject::none().bits(),
                "rb".to_string(),
                None,
                None,
                None,
                None,
                0,
                0,
            );
            assert!(!raw.is_null());
            let raw = MoltObject::from_ptr(raw).bits();
            let buffered = super::molt_buffered_new(
                classes.buffered_reader,
                raw,
                MoltObject::from_int(8).bits(),
            );
            assert!(!crate::exception_pending(py));
            super::molt_buffered_init(buffered, raw, MoltObject::from_int(8).bits());
            assert!(!crate::exception_pending(py));
            super::molt_file_close(if close_child { raw } else { buffered });
            assert!(!crate::exception_pending(py));
            assert!(state.backend.lock().unwrap().is_some());
            super::molt_file_read(buffered, MoltObject::from_int(1).bits());
            assert_eq!(io_error(py).0, "ValueError");
            super::molt_file_close(buffered);
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, buffered);
            dec_ref_bits(py, raw);
            assert!(state.backend.lock().unwrap().is_some());
            drop(state);
            remove_file(path).unwrap();
        }
    });
}

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

#[test]
fn buffered_reader_native_cursor_and_readinto1() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("", |py| unsafe {
        use crate::*;
        let source = MoltObject::from_ptr(alloc_bytes(py, b"abc\ndefghij")).bits();
        let raw = super::molt_bytesio_new(builtin_classes(py).bytes_io, source);
        dec_ref_bits(py, source);
        let size = MoltObject::from_int(4).bits();
        let reader = super::molt_buffered_new(builtin_classes(py).buffered_reader, raw, size);
        super::molt_buffered_init(reader, raw, size);
        assert!(!exception_pending(py));
        let assert_bytes = |value: u64, expected: &[u8]| {
            assert!(!exception_pending(py));
            let pointer = obj_from_bits(value).as_ptr().unwrap();
            assert_eq!(object_type_id(pointer), TYPE_ID_BYTES);
            assert_eq!(
                std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)),
                expected
            );
            dec_ref_bits(py, value);
        };
        // peek's size is a hint, not a result bound. All operations share this
        // read-ahead cursor, including a larger direct readinto1 destination.
        assert_bytes(
            super::molt_file_peek(reader, MoltObject::from_int(1).bits()),
            b"abc\n",
        );
        assert_bytes(
            super::molt_file_read1(reader, MoltObject::from_int(2).bits()),
            b"ab",
        );
        assert_eq!(
            to_i64(obj_from_bits(super::molt_file_tell(reader))),
            Some(2)
        );
        let destination = MoltObject::from_ptr(alloc_bytearray(py, b"--------")).bits();
        let count = super::molt_file_readinto1(reader, destination);
        assert!(!exception_pending(py));
        assert_eq!(to_i64(obj_from_bits(count)), Some(8));
        dec_ref_bits(py, count);
        assert_eq!(
            bytearray_vec_ref(obj_from_bits(destination).as_ptr().unwrap()),
            b"c\ndefghi"
        );
        dec_ref_bits(py, destination);
        assert_bytes(
            super::molt_file_read(reader, MoltObject::from_int(2).bits()),
            b"j",
        );
        let position = super::molt_file_seek(
            reader,
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(0).bits(),
        );
        assert!(!exception_pending(py));
        assert_eq!(to_i64(obj_from_bits(position)), Some(0));
        dec_ref_bits(py, position);
        assert_bytes(
            super::molt_file_readline(reader, MoltObject::from_int(-1).bits()),
            b"abc\n",
        );
        assert_bytes(super::molt_file_readall(reader), b"defghij");
        super::molt_file_close(reader);
        assert!(!exception_pending(py));
        dec_ref_bits(py, reader);
        dec_ref_bits(py, raw);
    });
}

fn bytes_output(bits: u64) -> Vec<u8> {
    let pointer = obj_from_bits(bits).as_ptr().unwrap();
    unsafe {
        assert_eq!(crate::object_type_id(pointer), crate::TYPE_ID_BYTES);
        std::slice::from_raw_parts(crate::bytes_data(pointer), crate::bytes_len(pointer)).to_vec()
    }
}

fn io_error(py: &crate::PyToken<'_>) -> (String, String) {
    assert!(crate::exception_pending(py));
    let error = crate::molt_exception_last();
    crate::molt_exception_clear();
    let kind = crate::type_name(py, obj_from_bits(error)).to_string();
    let message = crate::builtins::exceptions::molt_exception_message(error);
    let text = crate::string_obj_to_owned(obj_from_bits(message)).unwrap();
    dec_ref_bits(py, message);
    dec_ref_bits(py, error);
    (kind, text)
}

fn io_error_errno(py: &crate::PyToken<'_>) -> i64 {
    assert!(crate::exception_pending(py));
    let error = crate::molt_exception_last();
    crate::molt_exception_clear();
    let errno = crate::builtins::exceptions::exception_typed_field_get(
        py,
        obj_from_bits(error).as_ptr().unwrap(),
        molt_obj_model::ExceptionTypedField::OSErrorErrno,
    )
    .expect("host positioning raises OSError")
    .expect("host positioning preserves errno");
    let value = crate::to_i64(obj_from_bits(errno)).expect("errno must be an integer");
    dec_ref_bits(py, errno);
    dec_ref_bits(py, error);
    value
}

fn host_error_errno(error: &std::io::Error) -> i64 {
    let errno = error
        .raw_os_error()
        .expect("host operation has an OS error");
    #[cfg(windows)]
    let errno = crate::windows_abi::winerror_to_errno(errno);
    i64::from(errno)
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

fn open_entry(py: &crate::PyToken<'_>, entry: usize, path: u64, mode: u64, opener: u64) -> u64 {
    let none = crate::MoltObject::none().bits();
    let yes = crate::MoltObject::from_bool(true).bits();
    let buffering = crate::MoltObject::from_int(0).bits();
    match entry {
        0 => super::molt_file_open(path, mode),
        1 => super::molt_file_open_ex(path, mode, buffering, none, none, none, yes, opener),
        2 => super::molt_file_io_new(crate::builtin_classes(py).file_io, path, mode, yes, opener),
        3 => super::molt_open_builtin(path, mode, buffering, none, none, none, yes, opener),
        _ => unreachable!(),
    }
}

#[test]
fn text_positions_preserve_raw_offsets_and_disjoint_decoder_cookies() {
    use molt_stdlib_text::codec_registry::EncodingKind;
    use num_bigint::{BigInt, Sign};
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for offset in [0, 2, 3, 6, (1_u64 << 46) + 7, u64::MAX] {
            let bits =
                super::text_cookie_encode_bits(py, offset, None, &[], &[], None, false).unwrap();
            let integer = crate::index_bigint_from_obj(py, bits, "position must be int").unwrap();
            assert_eq!(
                integer,
                BigInt::from(offset),
                "stateless tell is a byte offset"
            );
            let restored = super::text_cookie_decode_value(&integer).unwrap();
            assert_eq!(restored.pos, offset);
            assert!(restored.pending_byte.is_none());
            assert!(restored.pending_bytes.is_empty());
            assert!(restored.pending_text.is_empty());
            assert!(restored.decoder.is_none());
            dec_ref_bits(py, bits);
        }
        for (offset, pending, bytes, text, decoder) in [
            (0, Some(0), &[][..], &[][..], None),
            (6, Some(b'\r'), &b"\xe2\x82"[..], &b"tail"[..], None),
            (10, None, &[][..], &[][..], Some(EncodingKind::Utf16BE)),
            (16, None, &[][..], &b"x"[..], Some(EncodingKind::Utf32LE)),
        ] {
            let bits =
                super::text_cookie_encode_bits(py, offset, pending, bytes, text, decoder, true)
                    .unwrap();
            let integer = crate::index_bigint_from_obj(py, bits, "position must be int").unwrap();
            assert!(
                integer > BigInt::from(u64::MAX),
                "state cannot alias any raw offset"
            );
            let restored = super::text_cookie_decode_value(&integer).unwrap();
            assert_eq!(restored.pos, offset);
            assert_eq!(restored.pending_byte, pending);
            assert_eq!(restored.pending_bytes, bytes);
            assert_eq!(restored.pending_text, text);
            assert_eq!(restored.decoder, decoder);
            assert_eq!(restored.bom_seen, Some(true));
            let (_, wire) = integer.to_bytes_le();
            let mut truncated = wire.clone();
            truncated.remove(0);
            assert!(
                super::text_cookie_decode_value(&BigInt::from_bytes_le(Sign::Plus, &truncated))
                    .is_err()
            );
            let mut unknown_flags = wire.clone();
            unknown_flags[8] |= 0x80;
            assert!(
                super::text_cookie_decode_value(&BigInt::from_bytes_le(Sign::Plus, &unknown_flags))
                    .is_err()
            );
            let mut unknown_decoder = wire.clone();
            unknown_decoder[11] = 255;
            assert!(
                super::text_cookie_decode_value(&BigInt::from_bytes_le(
                    Sign::Plus,
                    &unknown_decoder
                ))
                .is_err()
            );
            let mut oversized = wire.clone();
            oversized[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(
                super::text_cookie_decode_value(&BigInt::from_bytes_le(Sign::Plus, &oversized))
                    .is_err()
            );
            let mut extra = wire;
            extra.insert(16, 0);
            assert!(
                super::text_cookie_decode_value(&BigInt::from_bytes_le(Sign::Plus, &extra))
                    .is_err()
            );
            dec_ref_bits(py, bits);
        }
        assert!(super::text_cookie_decode_value(&BigInt::from(-1)).is_err());
        assert!(!crate::exception_pending(py));
    });
}

#[test]
fn memory_positions_use_target_ssize_bounds_and_preserve_failure_position() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let classes = crate::builtin_classes(py);
        let data = crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"abc")).bits();
        let text = io_string(py, "abc");
        let newline = io_string(py, "\n");
        let bytes_io = super::molt_bytesio_new(classes.bytes_io, data);
        let string_io = super::molt_stringio_new(classes.string_io, text, newline);
        assert!(!crate::exception_pending(py));
        let limit = isize::MAX as i128;
        for (handle, string) in [(bytes_io, false), (string_io, true)] {
            let offset = crate::int_bits_from_i128(py, limit);
            let result =
                super::molt_file_seek(handle, offset, crate::MoltObject::from_int(0).bits());
            assert!(!crate::exception_pending(py));
            assert_eq!(crate::to_i64(obj_from_bits(result)), Some(limit as i64));
            dec_ref_bits(py, result);
            for (delta, origin, kind) in [
                (limit + 1, 0, "OverflowError"),
                (-limit - 2, 0, "OverflowError"),
                (-1, 0, "ValueError"),
                (1, 1, if string { "OSError" } else { "OverflowError" }),
                (limit, 2, if string { "OSError" } else { "OverflowError" }),
            ] {
                let delta = crate::int_bits_from_i128(py, delta);
                super::molt_file_seek(handle, delta, crate::MoltObject::from_int(origin).bits());
                assert_eq!(io_error(py).0, kind);
                let current = super::molt_file_tell(handle);
                assert_eq!(crate::to_i64(obj_from_bits(current)), Some(limit as i64));
                dec_ref_bits(py, current);
                dec_ref_bits(py, delta);
            }
            if !string {
                let negative = crate::int_bits_from_i128(py, -limit - 1);
                let clamped =
                    super::molt_file_seek(handle, negative, crate::MoltObject::from_int(1).bits());
                assert!(!crate::exception_pending(py));
                assert_eq!(crate::to_i64(obj_from_bits(clamped)), Some(0));
                dec_ref_bits(py, negative);
                dec_ref_bits(py, clamped);
            }
            let end = super::molt_file_seek(
                handle,
                crate::MoltObject::from_int(0).bits(),
                crate::MoltObject::from_int(2).bits(),
            );
            assert!(!crate::exception_pending(py));
            assert_eq!(crate::to_i64(obj_from_bits(end)), Some(3));
            dec_ref_bits(py, end);
            dec_ref_bits(py, offset);
            super::molt_file_close(handle);
            dec_ref_bits(py, handle);
        }
        for bits in [data, text, newline] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn host_append_entrypoints_publish_eof_and_full_width_positions() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("fs.write", |py| {
        let path = temp_path("append_position");
        let path_bits = io_string(py, path.to_str().unwrap());
        let mode = io_string(py, "ab");
        for entry in 0..4 {
            std::fs::write(&path, b"prefix").unwrap();
            let handle = open_entry(py, entry, path_bits, mode, crate::MoltObject::none().bits());
            assert!(!crate::exception_pending(py));
            let position = super::molt_file_tell(handle);
            assert_eq!(crate::to_i64(obj_from_bits(position)), Some(6));
            dec_ref_bits(py, position);
            // A filesystem may reject even a seek-only offset above its
            // maximum file position. Compare against an independent host
            // descriptor on the same filesystem, including refusal state.
            let large = (1_i128 << 47) + 3;
            let mut reference = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            assert_eq!(reference.seek(SeekFrom::End(0)).unwrap(), 6);
            let expected = reference.seek(SeekFrom::Start(large as u64));
            let expected_position = reference.stream_position().unwrap();
            let offset = crate::int_bits_from_i128(py, large);
            let result =
                super::molt_file_seek(handle, offset, crate::MoltObject::from_int(0).bits());
            match expected {
                Ok(position) => {
                    assert_eq!(position, large as u64);
                    assert!(!crate::exception_pending(py), "entry {entry}");
                    assert_eq!(crate::to_i64(obj_from_bits(result)), Some(large as i64));
                }
                Err(error) => {
                    assert_eq!(expected_position, 6);
                    assert_eq!(io_error_errno(py), host_error_errno(&error));
                }
            }
            let position = super::molt_file_tell(handle);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                crate::to_i64(obj_from_bits(position)),
                Some(expected_position as i64)
            );
            super::molt_file_close(handle);
            assert!(!crate::exception_pending(py));
            for bits in [offset, result, position, handle] {
                dec_ref_bits(py, bits);
            }
            assert_eq!(std::fs::read(&path).unwrap(), b"prefix");
        }
        for bits in [path_bits, mode] {
            dec_ref_bits(py, bits);
        }
        remove_file(path).unwrap();
    });
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn descriptor_positions_preserve_host_results_and_checked_whence() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("fs.read", |py| {
        let path = temp_path("descriptor_position");
        std::fs::write(&path, b"prefix").unwrap();
        let path_bits = io_string(py, path.to_str().unwrap());
        let mode = io_string(py, "rb");
        let handle = open_entry(py, 2, path_bits, mode, crate::MoltObject::none().bits());
        assert!(!crate::exception_pending(py));
        let descriptor = super::molt_file_fileno(handle);
        let zero = crate::MoltObject::from_int(0).bits();
        let current = crate::MoltObject::from_int(1).bits();
        assert_eq!(
            crate::to_i64(obj_from_bits(crate::molt_os_lseek(
                descriptor,
                crate::MoltObject::from_int(6).bits(),
                zero,
            ))),
            Some(6),
        );
        assert!(!crate::exception_pending(py));
        let large = (1_i128 << 47) + 3;
        let mut reference = File::open(&path).unwrap();
        reference.seek(SeekFrom::Start(6)).unwrap();
        let expected = reference.seek(SeekFrom::Start(large as u64));
        let expected_position = reference.stream_position().unwrap();
        let offset = crate::int_bits_from_i128(py, large);
        let result = crate::molt_os_lseek(descriptor, offset, zero);
        match expected {
            Ok(position) => {
                assert_eq!(position, large as u64);
                assert!(!crate::exception_pending(py));
                assert_eq!(crate::to_i64(obj_from_bits(result)), Some(large as i64));
            }
            Err(error) => {
                assert_eq!(expected_position, 6);
                assert_eq!(io_error_errno(py), host_error_errno(&error));
            }
        }
        for invalid in [1_i128 << 40, -(1_i128 << 40)] {
            let invalid = crate::int_bits_from_i128(py, invalid);
            crate::molt_os_lseek(descriptor, zero, invalid);
            assert_eq!(io_error(py).0, "OverflowError");
            let position = crate::molt_os_lseek(descriptor, zero, current);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                crate::to_i64(obj_from_bits(position)),
                Some(expected_position as i64)
            );
            dec_ref_bits(py, position);
            dec_ref_bits(py, invalid);
        }
        for invalid in [i128::from(i64::MAX) + 1, i128::from(i64::MIN) - 1] {
            let invalid = crate::int_bits_from_i128(py, invalid);
            crate::molt_os_lseek(descriptor, invalid, zero);
            assert_eq!(io_error(py).0, "OverflowError");
            let position = crate::molt_os_lseek(descriptor, zero, current);
            assert!(!crate::exception_pending(py));
            assert_eq!(
                crate::to_i64(obj_from_bits(position)),
                Some(expected_position as i64)
            );
            dec_ref_bits(py, position);
            dec_ref_bits(py, invalid);
        }
        let invalid_descriptor = crate::int_bits_from_i128(py, 1_i128 << 40);
        crate::molt_os_lseek(invalid_descriptor, zero, zero);
        assert_eq!(io_error(py).0, "OverflowError");
        let position = super::molt_file_tell(handle);
        assert!(!crate::exception_pending(py));
        assert_eq!(
            crate::to_i64(obj_from_bits(position)),
            Some(expected_position as i64)
        );
        super::molt_file_close(handle);
        assert!(!crate::exception_pending(py));
        for bits in [
            position,
            invalid_descriptor,
            result,
            offset,
            descriptor,
            handle,
            mode,
            path_bits,
        ] {
            dec_ref_bits(py, bits);
        }
        drop(reference);
        assert_eq!(std::fs::read(&path).unwrap(), b"prefix");
        remove_file(path).unwrap();
    });
}

#[test]
fn open_entrypoints_report_selected_mode_missing_set_without_mutation() {
    let required = ["fs.read", "fs.write"];
    for mask in 0..4 {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&required, mask),
            None,
            |py| {
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
                            py,
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
        crate::MoltObject::from_ptr(crate::alloc_dict_with_pairs(py, &[method, function])).bits();
    let bases =
        crate::MoltObject::from_ptr(crate::alloc_tuple(py, &[crate::builtin_classes(py).object]))
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

fn tmp_mount(quota_mb: usize) -> (crate::vfs::VfsState, std::sync::Arc<crate::vfs::tmp::TmpFs>) {
    let backend = std::sync::Arc::new(crate::vfs::tmp::TmpFs::new(quota_mb));
    let mut table = crate::vfs::MountTable::new();
    table.add_mount("/tmp", backend.clone());
    (crate::vfs::VfsState::from_table(table), backend)
}

#[test]
fn selected_vfs_readwrite_aggregates_all_grants_after_fspath_without_mutation() {
    use crate::vfs::VfsBackend;
    let required = ["fs.read", "fs.write", "fs.tmp.read", "fs.tmp.write"];
    for mask in 0..16 {
        let (vfs, backend) = tmp_mount(1);
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&required, mask),
            Some(vfs),
            |py| {
                for pathlike in [false, true] {
                    backend.open_write("selected", b"retained").unwrap();
                    FSPATH_CALLS.store(0, Ordering::Relaxed);
                    let path = if pathlike {
                        fspath_instance(py)
                    } else {
                        io_string(py, "/tmp/selected")
                    };
                    let mode = io_string(py, "r+b");
                    let handle = open_entry(py, 3, path, mode, crate::MoltObject::none().bits());
                    assert_eq!(FSPATH_CALLS.load(Ordering::Relaxed), usize::from(pathlike));
                    let missing = required
                        .iter()
                        .enumerate()
                        .filter_map(|(index, name)| (mask & (1 << index) == 0).then_some(*name))
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        assert!(!crate::exception_pending(py));
                        let data =
                            super::molt_file_read(handle, crate::MoltObject::from_int(-1).bits());
                        assert_eq!(bytes_output(data), b"retained");
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

#[cfg(all(feature = "stdlib_fs_extra", not(target_arch = "wasm32")))]
static CODEC_OPENER_FD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

#[cfg(all(feature = "stdlib_fs_extra", not(target_arch = "wasm32")))]
extern "C" fn codec_opener(_path: u64, _flags: u64) -> u64 {
    crate::MoltObject::from_int(CODEC_OPENER_FD.load(Ordering::Relaxed)).bits()
}

#[cfg(all(feature = "stdlib_fs_extra", target_os = "linux"))]
#[test]
fn append_seek_failure_preserves_caller_fd_but_retires_opener_fd_and_accepts_pipe() {
    use std::os::fd::{AsRawFd, IntoRawFd};
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("fs.write", |py| {
        let path = temp_path("append_admission");
        std::fs::write(&path, b"prefix").unwrap();
        let path_bits = io_string(py, path.to_str().unwrap());
        let path_c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        let mode = io_string(py, "ab");
        let none = crate::MoltObject::none().bits();
        for (closefd, custom_opener) in [(true, false), (false, false), (true, true)] {
            let descriptor = unsafe { libc::open(path_c.as_ptr(), libc::O_PATH) };
            assert!(descriptor >= 0);
            CODEC_OPENER_FD.store(i64::from(descriptor), Ordering::Relaxed);
            let opener = if custom_opener {
                test_callable(py, codec_opener as *const (), 2)
            } else {
                none
            };
            super::molt_file_io_new(
                crate::builtin_classes(py).file_io,
                if custom_opener {
                    path_bits
                } else {
                    crate::MoltObject::from_int(i64::from(descriptor)).bits()
                },
                mode,
                crate::MoltObject::from_bool(closefd).bits(),
                opener,
            );
            assert_eq!(
                io_error(py).0,
                "OSError",
                "EBADF is not a nonseekable stream"
            );
            let live = unsafe { libc::fcntl(descriptor, libc::F_GETFD) } >= 0;
            assert_eq!(
                live, !custom_opener,
                "failed FileIO never owns the caller's descriptor"
            );
            if live {
                unsafe {
                    libc::close(descriptor);
                }
            }
            dec_ref_bits(py, opener);
        }
        // Supplied descriptors and real opener descriptors share the EOF
        // publication rule, even though O_APPEND itself is a write policy.
        for custom_opener in [false, true] {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let descriptor = file.into_raw_fd();
            CODEC_OPENER_FD.store(i64::from(descriptor), Ordering::Relaxed);
            let opener = if custom_opener {
                test_callable(py, codec_opener as *const (), 2)
            } else {
                none
            };
            let handle = super::molt_file_io_new(
                crate::builtin_classes(py).file_io,
                if custom_opener {
                    path_bits
                } else {
                    crate::MoltObject::from_int(i64::from(descriptor)).bits()
                },
                mode,
                crate::MoltObject::from_bool(true).bits(),
                opener,
            );
            assert!(!crate::exception_pending(py));
            let position = super::molt_file_tell(handle);
            assert_eq!(crate::to_i64(obj_from_bits(position)), Some(6));
            super::molt_file_close(handle);
            for bits in [handle, position, opener] {
                dec_ref_bits(py, bits);
            }
            assert!(unsafe { libc::fcntl(descriptor, libc::F_GETFD) } < 0);
        }
        let (mut reader, writer) = std::io::pipe().unwrap();
        let descriptor = unsafe { libc::dup(writer.as_raw_fd()) };
        assert!(descriptor >= 0);
        let handle = super::molt_file_io_new(
            crate::builtin_classes(py).file_io,
            crate::MoltObject::from_int(i64::from(descriptor)).bits(),
            mode,
            crate::MoltObject::from_bool(true).bits(),
            none,
        );
        assert!(
            !crate::exception_pending(py),
            "only ESPIPE is admitted on append initialization"
        );
        // Append construction suppresses ESPIPE, but public positioning
        // must retain the actual errno instead of a generic OSError.
        super::molt_file_tell(handle);
        assert_eq!(io_error_errno(py), i64::from(libc::ESPIPE));
        super::molt_file_seek(
            handle,
            crate::MoltObject::from_int(0).bits(),
            crate::MoltObject::from_int(0).bits(),
        );
        assert_eq!(io_error_errno(py), i64::from(libc::ESPIPE));
        let data = crate::MoltObject::from_ptr(crate::alloc_bytes(py, b"x")).bits();
        super::molt_file_write(handle, data);
        super::molt_file_close(handle);
        assert!(!crate::exception_pending(py));
        drop(writer);
        let mut observed = String::new();
        std::io::Read::read_to_string(&mut reader, &mut observed).unwrap();
        assert_eq!(observed, "x");
        for bits in [data, handle, path_bits, mode] {
            dec_ref_bits(py, bits);
        }
        remove_file(path).unwrap();
    });
}

#[cfg(all(feature = "stdlib_fs_extra", not(target_arch = "wasm32")))]
#[test]
fn codec_failure_preserves_open_order_and_retires_selected_descriptor_owner() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
        "fs.read,fs.write",
        |py| {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("selected");
            let none = crate::MoltObject::none().bits();
            let yes = crate::MoltObject::from_bool(true).bits();
            let buffering = crate::MoltObject::from_int(-1).bits();
            let path_bits = io_string(py, path.to_str().unwrap());
            let encoding = io_string(py, "molt-invalid-codec");
            let read = io_string(py, "r");
            let write = io_string(py, "w");
            super::molt_file_open_ex(path_bits, read, buffering, encoding, none, none, yes, none);
            assert_eq!(io_error(py).0, "FileNotFoundError");
            std::fs::write(&path, b"sentinel").unwrap();
            super::molt_file_open_ex(path_bits, write, buffering, encoding, none, none, yes, none);
            assert_eq!(io_error(py).0, "LookupError");
            assert_eq!(std::fs::read(&path).unwrap(), b"");

            for (closefd, custom_opener) in [(true, false), (false, false), (true, true)] {
                let dir = io_string(py, root.path().to_str().unwrap());
                let empty = io_string(py, "");
                let created = crate::molt_tempfile_mkstemp(
                    empty,
                    empty,
                    dir,
                    crate::MoltObject::from_bool(false).bits(),
                );
                assert!(!crate::exception_pending(py));
                let rows = unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        obj_from_bits(created).as_ptr().unwrap(),
                        "test tuple",
                    )
                }
                .unwrap();
                let fd = crate::to_i64(obj_from_bits(rows[0])).unwrap();
                CODEC_OPENER_FD.store(fd, Ordering::Relaxed);
                let opener = if custom_opener {
                    test_callable(py, codec_opener as *const (), 2)
                } else {
                    none
                };
                let input = if custom_opener { path_bits } else { rows[0] };
                super::molt_file_open_ex(
                    input,
                    read,
                    buffering,
                    encoding,
                    none,
                    none,
                    crate::MoltObject::from_bool(closefd).bits(),
                    opener,
                );
                assert_eq!(io_error(py).0, "LookupError");
                let duplicate = unsafe { libc::dup(fd as libc::c_int) };
                assert_eq!(duplicate >= 0, !closefd);
                if duplicate >= 0 {
                    unsafe {
                        libc::close(duplicate);
                        libc::close(fd as libc::c_int);
                    }
                }
                let selected_path = crate::string_obj_to_owned(obj_from_bits(rows[1])).unwrap();
                std::fs::remove_file(selected_path).unwrap();
                for bits in [opener, created, dir, empty] {
                    dec_ref_bits(py, bits);
                }
            }
            for bits in [path_bits, encoding, read, write] {
                dec_ref_bits(py, bits);
            }
        },
    );
}

#[cfg(all(feature = "stdlib_fs_extra", not(target_arch = "wasm32")))]
#[test]
fn lazy_raw_publication_failure_preserves_live_stream_and_detach_identity() {
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    struct RestoreTracker;
    impl Drop for RestoreTracker {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
        "fs.read,fs.write",
        |py| {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("raw-owner");
            std::fs::write(&path, b"abcdef").unwrap();
            let path = io_string(py, path.to_str().unwrap());
            let mode = io_string(py, "rb");
            let none = crate::MoltObject::none().bits();
            let yes = crate::MoltObject::from_bool(true).bits();
            let mut refused = false;
            let mut completed = false;
            for limit in 0..16 {
                let stream = super::molt_file_open_ex(
                    path,
                    mode,
                    crate::MoltObject::from_int(4).bits(),
                    none,
                    none,
                    none,
                    yes,
                    none,
                );
                assert!(!crate::exception_pending(py));
                let pointer = obj_from_bits(stream).as_ptr().unwrap();
                set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                    max_allocations: Some(limit),
                    ..Default::default()
                })));
                let restore = RestoreTracker;
                let raw = unsafe { super::file_handle_raw_bits(py, pointer) };
                drop(restore);
                let raw = if let Some(raw) = raw {
                    completed = true;
                    raw
                } else {
                    refused = true;
                    assert!(crate::exception_pending(py));
                    crate::molt_exception_clear();
                    // A refusal during raw/dictionary construction must not
                    // close the live parent's shared descriptor.
                    assert!(!unsafe {
                        super::file_handle_is_closed(&*crate::file_handle_ptr(pointer))
                    });
                    unsafe { super::file_handle_raw_bits(py, pointer) }.unwrap()
                };
                assert!(!crate::exception_pending(py));
                let repeated = unsafe { super::file_handle_raw_bits(py, pointer) }.unwrap();
                assert_eq!(raw, repeated);
                dec_ref_bits(py, repeated);
                let first = super::molt_file_read(stream, crate::MoltObject::from_int(1).bits());
                assert_eq!(bytes_output(first), b"a");
                dec_ref_bits(py, first);
                assert_eq!(
                    crate::to_i64(obj_from_bits(super::molt_file_tell(raw))),
                    Some(4)
                );
                let detached = super::molt_file_detach(stream);
                assert_eq!(detached, raw);
                let remaining = super::molt_file_read(raw, crate::MoltObject::from_int(-1).bits());
                assert_eq!(bytes_output(remaining), b"ef");
                dec_ref_bits(py, remaining);
                super::molt_file_close(raw);
                for value in [raw, detached, stream] {
                    dec_ref_bits(py, value);
                }
                assert!(!crate::exception_pending(py));
                if completed {
                    break;
                }
            }
            assert!(refused && completed);
            for value in [path, mode] {
                dec_ref_bits(py, value);
            }
        },
    );
}

#[test]
fn denied_custom_opener_is_never_called_and_requires_no_mount_grants() {
    for mask in 0..3 {
        let (vfs, _backend) = tmp_mount(1);
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&["fs.read", "fs.write"], mask),
            Some(vfs),
            |py| {
                let path = io_string(py, "/tmp/selected");
                let mode = io_string(py, "r+b");
                let opener = test_callable(py, forbidden_opener as *const (), 2);
                OPENER_CALLS.store(0, Ordering::Relaxed);
                open_entry(py, 3, path, mode, opener);
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
        let mut table = crate::vfs::MountTable::new();
        table.add_mount(
            "/bundle",
            std::sync::Arc::new(crate::vfs::bundle::BundleFs::from_entries(vec![(
                "selected".to_string(),
                b"retained".to_vec(),
            )])),
        );
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            &selected_grants(&required, mask),
            Some(crate::vfs::VfsState::from_table(table)),
            |py| {
                let path = io_string(py, "/bundle/selected");
                let mode = io_string(py, "r+b");
                open_entry(py, 3, path, mode, crate::MoltObject::none().bits());
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
                open_entry(py, 3, path, invalid, crate::MoltObject::none().bits());
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
        let backend = std::sync::Arc::new(crate::vfs::dev::DevFs::new());
        let mut table = crate::vfs::MountTable::new();
        table.add_mount("/dev", backend.clone());
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
            grants,
            Some(crate::vfs::VfsState::from_table(table)),
            |py| {
                for (path_name, mode_name, expected_error) in [
                    ("/dev/stdout", "ab", None),
                    ("/dev/stderr", "ab", None),
                    ("/dev/stdin", "ab", Some("PermissionError")),
                    ("/dev", "ab", Some("IsADirectoryError")),
                    ("/dev/missing", "ab", Some("FileNotFoundError")),
                ] {
                    let path = io_string(py, path_name);
                    let mode = io_string(py, mode_name);
                    let handle = open_entry(py, 3, path, mode, crate::MoltObject::none().bits());
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
                    open_entry(py, 3, path, mode, crate::MoltObject::none().bits());
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
            },
        );
    }
}

#[test]
fn vfs_modes_retain_append_truncate_and_exclusive_semantics() {
    use crate::vfs::VfsBackend;
    let (vfs, backend) = tmp_mount(1);
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
        "fs.write,fs.tmp.write",
        Some(vfs),
        |py| {
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
                let handle = open_entry(py, 3, path, mode_bits, crate::MoltObject::none().bits());
                assert!(!crate::exception_pending(py), "{mode}");
                let position = super::molt_file_tell(handle);
                assert_eq!(
                    crate::to_i64(obj_from_bits(position)),
                    Some(if mode == "ab" && initial.is_some() {
                        3
                    } else {
                        0
                    })
                );
                dec_ref_bits(py, position);
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
            open_entry(py, 3, path, mode, crate::MoltObject::none().bits());
            assert_eq!(io_error(py).0, "FileExistsError");
            assert_eq!(backend.open_read("selected").unwrap(), b"new");
            for bits in [path, mode] {
                dec_ref_bits(py, bits);
            }
        },
    );
}

#[test]
fn vfs_refused_construction_never_publishes_a_writeback_owner() {
    use crate::vfs::VfsBackend;
    let backend = std::sync::Arc::new(crate::vfs::tmp::TmpFs::new(1024));
    let mut table = crate::vfs::MountTable::new();
    table.add_mount("/tmp", backend.clone());
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
        "fs.write,fs.tmp.write",
        Some(crate::vfs::VfsState::from_table(table)),
        |py| {
            let path = io_string(py, "/tmp/selected");
            let mode = io_string(py, "w");
            let invalid = io_string(py, "molt-invalid-codec");
            let encoding = io_string(py, "utf-8");
            let none = crate::MoltObject::none().bits();
            let yes = crate::MoltObject::from_bool(true).bits();
            let buffering = crate::MoltObject::from_int(-1).bits();
            super::molt_file_open_ex(path, mode, buffering, invalid, none, none, yes, none);
            assert_eq!(io_error(py).0, "LookupError");
            assert!(
                runtime_state(py)
                    .io
                    .vfs_writebacks
                    .lock()
                    .unwrap()
                    .is_empty()
            );
            assert!(!backend.exists("selected"));
            let stream =
                super::molt_file_open_ex(path, mode, buffering, encoding, none, none, yes, none);
            assert!(!crate::exception_pending(py));
            assert_eq!(runtime_state(py).io.vfs_writebacks.lock().unwrap().len(), 1);
            let content = io_string(py, "retained");
            super::molt_file_write(stream, content);
            super::molt_file_close(stream);
            assert!(!crate::exception_pending(py));
            assert_eq!(backend.open_read("selected").unwrap(), b"retained");
            assert!(
                runtime_state(py)
                    .io
                    .vfs_writebacks
                    .lock()
                    .unwrap()
                    .is_empty()
            );
            for bits in [path, mode, invalid, encoding, stream, content] {
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
            crate::test_support::RuntimeTestTransaction::with_explicit_capabilities_and_vfs(
                "fs.write,fs.tmp.write",
                Some(crate::vfs::VfsState::from_table(table)),
                |py| {
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
                        (
                            std::sync::Arc::as_ptr(raw.state.as_ref().unwrap()) as usize,
                            raw.buffer_bits,
                        )
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
                    assert!(unsafe { super::file_handle_is_closed(&*crate::file_handle_ptr(ptr)) });
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
