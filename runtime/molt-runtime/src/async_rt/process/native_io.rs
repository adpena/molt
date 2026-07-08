use super::stdio::NativeProcessStdio;
use crate::*;
use std::io::{Read, Write};
use std::sync::OnceLock;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::thread;
use std::time::Duration;

pub(super) fn trace_process_io() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_PROCESS_IO").ok().as_deref(),
            Some("1")
        )
    })
}

pub(super) fn ignore_sigpipe() {
    static IGNORE: OnceLock<()> = OnceLock::new();
    IGNORE.get_or_init(|| {
        #[cfg(unix)]
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
    });
}

fn spawn_process_reader(mut reader: impl Read + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = buf[..n].to_vec();
                    if trace_process_io() {
                        let limit = 256usize;
                        let preview = bytes
                            .iter()
                            .take(limit)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if bytes.len() > limit {
                            eprintln!(
                                "molt_process_reader read {} bytes [{} ...]",
                                bytes.len(),
                                preview
                            );
                        } else {
                            eprintln!(
                                "molt_process_reader read {} bytes [{}]",
                                bytes.len(),
                                preview
                            );
                        }
                    }
                    if !super::super::channels::stream_enqueue_bytes_blocking(stream, bytes) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        super::super::channels::stream_close_local(stream);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

fn spawn_process_writer(mut writer: impl Write + Send + 'static, stream_bits: u64) {
    unsafe {
        let _ = molt_stream_clone(stream_bits);
    }
    thread::spawn(move || {
        ignore_sigpipe();
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        let receiver = stream.receiver.clone();
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => {
                    super::super::channels::stream_release_queued_bytes(stream, bytes.len());
                    if bytes.is_empty() {
                        continue;
                    }
                    if trace_process_io() {
                        let limit = 64usize;
                        let preview = bytes
                            .iter()
                            .take(limit)
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if bytes.len() > limit {
                            eprintln!(
                                "molt_process_writer write {} bytes [{} ...]",
                                bytes.len(),
                                preview
                            );
                        } else {
                            eprintln!(
                                "molt_process_writer write {} bytes [{}]",
                                bytes.len(),
                                preview
                            );
                        }
                    }
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                    if writer.flush().is_err() {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if stream.closed.load(AtomicOrdering::Acquire) {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        stream.closed.store(true, AtomicOrdering::Release);
        unsafe {
            molt_stream_drop(stream_bits);
        }
    });
}

pub(super) fn attach_process_stdio(
    child: &mut std::process::Child,
    stdio: &mut NativeProcessStdio,
) {
    if stdio.stdin_stream != 0
        && let Some(stdin) = child.stdin.take()
    {
        spawn_process_writer(stdin, stdio.stdin_stream);
    }
    if stdio.stdout_stream != 0 {
        if let Some(reader) = stdio.merged_stdout_reader.take() {
            spawn_process_reader(reader, stdio.stdout_stream);
        } else if let Some(stdout) = child.stdout.take() {
            spawn_process_reader(stdout, stdio.stdout_stream);
        }
    }
    if stdio.stderr_stream != 0
        && let Some(stderr) = child.stderr.take()
    {
        spawn_process_reader(stderr, stdio.stderr_stream);
    }
}
