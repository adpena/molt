use super::guest_wait_result;
use molt_runtime_platform::synthetic_errno::GuestErrno;

#[test]
fn guest_wait_status_keeps_synthetic_retry_timeout_and_error_codes() {
    assert_eq!(guest_wait_result(0), Ok(()));
    // Host synthetic EAGAIN11 is not raw WASI ECANCELED11.
    assert_eq!(guest_wait_result(-11), Err(GuestErrno::EAGAIN));
    assert_eq!(guest_wait_result(-110), Err(GuestErrno::ETIMEDOUT));
    assert_eq!(guest_wait_result(-9), Err(GuestErrno::EBADF));
    for malformed in [1, i32::MIN, -123456] {
        assert_eq!(guest_wait_result(malformed), Err(GuestErrno::EIO));
    }
}
