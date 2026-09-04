mod capabilities;
mod channel;
mod db;
mod stream;
mod websocket;

pub(crate) use capabilities::{
    capability_fix_hint, has_capability, is_trusted, operation_allowed, raise_capability_denied,
    require_operation,
};
#[cfg(any(target_arch = "wasm32", molt_has_net_io))]
pub use channel::molt_chan_recv_blocking;
pub use channel::{
    MoltChannel, molt_chan_drop, molt_chan_new, molt_chan_recv, molt_chan_send,
    molt_chan_send_blocking, molt_chan_try_recv, molt_chan_try_send,
};
pub use db::{molt_db_exec, molt_db_exec_obj, molt_db_query, molt_db_query_obj};
#[cfg(molt_has_net_io)]
pub use db::{molt_db_set_exec_hook, molt_db_set_query_hook};
pub(crate) use molt_runtime_core::host_capabilities_generated::OperationId;
pub use stream::*;
pub(crate) use stream::{
    default_stream_max_queued_bytes, stream_close_local, stream_enqueue_bytes_blocking,
    stream_new_with_byte_budget, stream_release_queued_bytes,
};
pub use websocket::*;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) use websocket::{ws_wait_detach_resource, ws_wait_release_detached_resource};
