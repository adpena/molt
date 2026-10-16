use std::io;

use super::super::read_daemon_request_bytes;
use super::context::DaemonState;
use super::dispatch::handle_daemon_request_bytes;
use super::wire::write_daemon_response;

pub(crate) fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    state: &DaemonState,
) -> io::Result<()> {
    let mut reader = io::BufReader::new(stream.try_clone()?);
    loop {
        let raw_bytes = read_daemon_request_bytes(&mut reader, state.request_limit_bytes)?;
        if raw_bytes.is_empty() {
            return Ok(());
        }
        state.stats().requests_total += 1;
        let response = handle_daemon_request_bytes(&raw_bytes, state);
        write_daemon_response(stream, &response)?;
    }
}
