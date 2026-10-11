//! Bounded stdin request reads, which every backend build uses.

use super::*;

#[test]
fn read_bounded_request_bytes_allows_exact_limit() {
    let cursor = Cursor::new(b"abcd".to_vec());
    let bytes =
        read_bounded_request_bytes(cursor, 4, "backend stdin request").expect("request bytes");
    assert_eq!(bytes, b"abcd");
}

#[test]
fn read_bounded_request_bytes_rejects_oversized_request() {
    let cursor = Cursor::new(b"abcde".to_vec());
    let err = read_bounded_request_bytes(cursor, 4, "backend stdin request")
        .expect_err("oversized request");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("backend stdin request exceeded 4 byte limit")
    );
}

#[test]
fn request_bounded_read_rejects_streaming_read_past_limit() {
    let cursor = Cursor::new(b"abcde".to_vec());
    let mut reader = RequestBoundedRead::new(cursor, 4, "streaming backend stdin request");
    let mut first_chunk = [0_u8; 4];
    assert_eq!(
        reader.read(&mut first_chunk).expect("first bounded read"),
        4
    );
    assert_eq!(&first_chunk, b"abcd");
    let mut probe = [0_u8; 1];
    let err = reader.read(&mut probe).expect_err("stream overflow");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("streaming backend stdin request exceeded 4 byte limit")
    );
}
