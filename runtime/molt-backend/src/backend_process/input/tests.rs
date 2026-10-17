use super::*;
use crate::backend_process::io_limits::RequestBoundedRead;
use std::io::{BufReader, Cursor};

// Literal wire bytes, independent of Molt's encoders: map {"functions": []}.
const JSON: &[u8] = br#"{"functions":[]}"#;
const MSGPACK: &[u8] = b"\x81\xa9functions\x90";
#[cfg(feature = "cbor")]
const CBOR: &[u8] = b"\xa1\x69functions\x80";
const NDJSON: &[u8] = b"{\"kind\":\"ir_stream_start\"}\n{\"kind\":\"ir_stream_end\"}";

struct InputFile(std::path::PathBuf);
impl InputFile {
    fn new(bytes: &[u8]) -> Self {
        let path = molt_ir::debug_artifacts::prepare_unique_debug_artifact_path("input-frame-test")
            .expect("prepare unique input fixture");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let input = Self(path);
        std::io::Write::write_all(&mut file, bytes).unwrap();
        input
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for InputFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).expect("remove input fixture");
    }
}

#[test]
fn file_receivers_reject_a_second_frame_and_accept_one_complete_frame() {
    for (format, bytes) in [
        ("json", JSON),
        ("msgpack", MSGPACK),
        ("ndjson", NDJSON),
        #[cfg(feature = "cbor")]
        ("cbor", CBOR),
    ] {
        let valid = InputFile::new(bytes);
        assert!(
            read_backend_ir_document(format, Some(valid.path()))
                .unwrap()
                .ir
                .functions
                .is_empty()
        );
        let mut repeated = bytes.to_vec();
        if format == "ndjson" {
            repeated.push(b'\n');
        }
        repeated.extend_from_slice(bytes);
        let invalid = InputFile::new(&repeated);
        assert_eq!(
            read_backend_ir_document(format, Some(invalid.path()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData,
            "{format}"
        );
        let mut truncated = bytes.to_vec();
        truncated.pop();
        let invalid = InputFile::new(&truncated);
        assert!(
            read_backend_ir_document(format, Some(invalid.path())).is_err(),
            "{format}"
        );
    }
}

#[test]
fn json_whitespace_is_not_a_second_frame() {
    let mut bytes = JSON.to_vec();
    bytes.extend_from_slice(b" \n\t\r");
    let input = InputFile::new(&bytes);
    assert!(read_backend_ir_document("json", Some(input.path())).is_ok());
}

#[test]
fn unknown_explicit_format_is_rejected_before_opening_input() {
    for format in ["", "JSON", "unknown"] {
        let error = read_backend_ir_document(format, Some("missing-input-that-must-not-be-opened"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("unknown IR format"));
    }
}

#[cfg(not(feature = "cbor"))]
#[test]
fn disabled_cbor_is_rejected_before_opening_input() {
    let error = read_backend_ir_document("cbor", Some("missing-input-that-must-not-be-opened"))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(error.to_string().contains("'cbor' feature"));
}

#[test]
fn binary_eof_checks_observe_buffered_suffixes_and_input_limit() {
    for format in [
        IrFormat::MessagePack,
        #[cfg(feature = "cbor")]
        IrFormat::Cbor,
    ] {
        let bytes = match format {
            IrFormat::MessagePack => MSGPACK,
            #[cfg(feature = "cbor")]
            IrFormat::Cbor => CBOR,
            _ => unreachable!(),
        };
        for capacity in [1, 8, 1 << 20] {
            for suffix in [None, Some(0), Some(b' '), Some(0xc1)] {
                let mut input = bytes.to_vec();
                if let Some(byte) = suffix {
                    input.push(byte);
                }
                for limit in [bytes.len(), bytes.len() + 1] {
                    let bounded = RequestBoundedRead::new(Cursor::new(&input), limit, "test frame");
                    let mut reader = BufReader::with_capacity(capacity, bounded);
                    let result = match format {
                        IrFormat::MessagePack => msgpack::decode(&mut reader),
                        #[cfg(feature = "cbor")]
                        IrFormat::Cbor => cbor::decode(&mut reader),
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        result.is_ok(),
                        suffix.is_none(),
                        "{format:?}, capacity={capacity}, limit={limit}, suffix={suffix:?}"
                    );
                    if suffix.is_some() && limit == bytes.len() {
                        assert!(result.unwrap_err().to_string().contains("byte limit"));
                    }
                }
            }
        }
    }
}

#[test]
fn binary_decoders_handle_short_reads_and_do_not_mask_eof_errors() {
    struct TailReader {
        bytes: Cursor<&'static [u8]>,
        interrupted: bool,
        fail: bool,
    }
    impl io::Read for TailReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            // Force real one-byte reads even if BufReader bypasses its small buffer.
            let count = io::Read::read(&mut self.bytes, &mut buffer[..1])?;
            if count != 0 {
                return Ok(count);
            }
            if std::mem::take(&mut self.interrupted) {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            if self.fail {
                return Err(io::Error::other("independent EOF failure"));
            }
            Ok(0)
        }
    }
    for format in [
        IrFormat::MessagePack,
        #[cfg(feature = "cbor")]
        IrFormat::Cbor,
    ] {
        let bytes = match format {
            IrFormat::MessagePack => MSGPACK,
            #[cfg(feature = "cbor")]
            IrFormat::Cbor => CBOR,
            _ => unreachable!(),
        };
        for fail in [false, true] {
            let mut reader = BufReader::new(TailReader {
                bytes: Cursor::new(bytes),
                interrupted: true,
                fail,
            });
            let result = match format {
                IrFormat::MessagePack => msgpack::decode(&mut reader),
                #[cfg(feature = "cbor")]
                IrFormat::Cbor => cbor::decode(&mut reader),
                _ => unreachable!(),
            };
            if fail {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("independent EOF failure")
                );
            } else {
                assert!(result.unwrap().ir.functions.is_empty());
            }
        }
    }
}
