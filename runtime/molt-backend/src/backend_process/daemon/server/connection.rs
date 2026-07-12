use std::io;

use super::context::DaemonConnectionContext;
use super::dispatch::handle_daemon_request_bytes;
use super::wire::{read_daemon_request_bytes, write_daemon_response};

pub(crate) fn handle_daemon_connection(
    stream: &mut std::os::unix::net::UnixStream,
    mut ctx: DaemonConnectionContext<'_>,
) -> io::Result<()> {
    let mut reader = io::BufReader::new(stream.try_clone()?);
    loop {
        let raw_bytes = read_daemon_request_bytes(&mut reader, ctx.request_limit_bytes)?;
        if raw_bytes.is_empty() {
            return Ok(());
        }
        ctx.stats.requests_total = ctx.stats.requests_total.saturating_add(1);
        let response = handle_daemon_request_bytes(&raw_bytes, &mut ctx);
        write_daemon_response(stream, &response)?;
    }
}
