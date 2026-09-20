mod capabilities;
mod channel;
mod db;
mod stream;
mod websocket;

/// Public channel-family constructors take a Python integer, never raw ABI
/// bits. Keep integer-protocol admission and target-width checks shared.
fn capacity_from_object(py: &crate::PyToken<'_>, bits: u64) -> Option<usize> {
    let capacity = crate::index_i64_from_obj(py, bits, "channel capacity must be an integer");
    if crate::exception_pending(py) {
        return None;
    }
    if capacity < 0 {
        return crate::raise_exception::<Option<usize>>(
            py,
            "ValueError",
            "channel capacity must be non-negative",
        );
    }
    usize::try_from(capacity).ok().or_else(|| {
        crate::raise_exception::<Option<usize>>(
            py,
            "OverflowError",
            "channel capacity is too large for this target",
        )
    })
}

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

#[cfg(test)]
mod capacity_tests {
    #[test]
    fn public_channel_constructors_share_boxed_capacity_admission() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for value in [0, 1, 7] {
                assert_eq!(
                    super::capacity_from_object(py, crate::MoltObject::from_int(value).bits()),
                    Some(value as usize)
                );
            }
            let too_wide =
                crate::int_bits_from_bigint(py, num_bigint::BigInt::from(1u8) << usize::BITS);
            let constructors: [extern "C" fn(u64) -> u64; 3] = [
                super::molt_chan_new,
                super::molt_stream_new,
                super::molt_ws_pair_obj,
            ];
            for constructor in constructors {
                for (bits, exception) in [
                    (0, "TypeError"),
                    (crate::MoltObject::from_float(1.0).bits(), "TypeError"),
                    (crate::MoltObject::none().bits(), "TypeError"),
                    (crate::MoltObject::from_int(-1).bits(), "ValueError"),
                    (too_wide, "OverflowError"),
                ] {
                    assert_eq!(constructor(bits), crate::MoltObject::none().bits());
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py, error, exception
                    ));
                    crate::clear_exception(py);
                    crate::dec_ref_bits(py, error);
                }
            }
            crate::dec_ref_bits(py, too_wide);

            for capacity in [0, 1] {
                let bits = super::molt_chan_new(crate::MoltObject::from_int(capacity).bits());
                let ptr = crate::ptr_from_bits(bits);
                assert!(!ptr.is_null());
                let channel = unsafe { &*(ptr as *mut super::MoltChannel) };
                assert_eq!(
                    channel.sender.capacity(),
                    (capacity != 0).then_some(capacity as usize)
                );
                unsafe { super::molt_chan_drop(bits) };
            }
        });
    }
}
