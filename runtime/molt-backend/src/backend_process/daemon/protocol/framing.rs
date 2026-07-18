use std::io::{self, BufRead, Read};

use super::response::DaemonResponse;

pub(crate) fn read_daemon_request_bytes<R: BufRead>(
    reader: &mut R,
    request_limit_bytes: usize,
) -> io::Result<Vec<u8>> {
    let mut raw_bytes = Vec::new();
    let limit = u64::try_from(request_limit_bytes)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    reader.take(limit).read_until(b'\n', &mut raw_bytes)?;
    if raw_bytes.len() > request_limit_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("daemon request exceeded {request_limit_bytes} byte limit"),
        ));
    }
    Ok(raw_bytes)
}

pub(crate) fn daemon_response_payload(response: &DaemonResponse) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&response.to_json_value()).map_err(io::Error::other)
}
