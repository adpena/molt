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
    memory_backend_vec_ref, prepend_read_buffer, rewind_read_buffer, unread_bytes,
};
#[path = "io/handle.rs"]
mod handle;
use handle::{
    VfsWritebackEntry, alloc_file_handle_with_state, resolve_file_handle_ptr,
    vfs_writeback_register, vfs_writeback_take,
};
pub(crate) use handle::{
    close_payload, file_handle_close_ptr, file_handle_detached_message, file_handle_enter,
    file_handle_exit, file_handle_is_closed, file_handle_require_attached,
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

pub(crate) fn io_clear_runtime_state(_py: &PyToken<'_>, state: &crate::state::RuntimeState) {
    crate::gil_assert();
    for slot in state.io.stdio_slots() {
        let bits = slot.swap(0, Ordering::AcqRel);
        if bits != 0 && !obj_from_bits(bits).is_none() {
            let _ = molt_file_flush(bits);
            if exception_pending(_py) {
                clear_exception(_py);
            }
            dec_ref_bits(_py, bits);
        }
    }
    state.io.vfs_writebacks.lock().unwrap().clear();
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
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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
}
