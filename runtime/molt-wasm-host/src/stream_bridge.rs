use super::*;

pub(super) struct RuntimeExports {
    stream_new: Func,
    pub(super) stream_send: Func,
    pub(super) stream_close: Func,
    stream_drop: Func,
    int_from_i64: Func,
    exception_pending_fast: Func,
    pub(super) alloc: Func,
    pub(super) handle_resolve: Func,
    pub(super) dec_ref_obj: Func,
    pub(super) header_size: Option<Func>,
    pub(super) cancel_is_cancelled: Option<Func>,
}

pub(super) fn runtime_exports(caller: &mut Caller<HostState>) -> Result<RuntimeExports> {
    let stream_new = caller
        .get_export("molt_stream_new")
        .and_then(Extern::into_func)
        .context("missing molt_stream_new export")?;
    let stream_send = caller
        .get_export("molt_stream_send")
        .and_then(Extern::into_func)
        .context("missing molt_stream_send export")?;
    let stream_close = caller
        .get_export("molt_stream_close")
        .and_then(Extern::into_func)
        .context("missing molt_stream_close export")?;
    let stream_drop = caller
        .get_export("molt_stream_drop")
        .and_then(Extern::into_func)
        .context("missing molt_stream_drop export")?;
    let int_from_i64 = caller
        .get_export("molt_int_from_i64")
        .and_then(Extern::into_func)
        .context("missing molt_int_from_i64 export")?;
    let exception_pending_fast = caller
        .get_export("molt_exception_pending_fast")
        .and_then(Extern::into_func)
        .context("missing molt_exception_pending_fast export")?;
    let alloc = caller
        .get_export("molt_alloc")
        .and_then(Extern::into_func)
        .context("missing molt_alloc export")?;
    let handle_resolve = caller
        .get_export("molt_handle_resolve")
        .and_then(Extern::into_func)
        .context("missing molt_handle_resolve export")?;
    let dec_ref_obj = caller
        .get_export("molt_dec_ref_obj")
        .and_then(Extern::into_func)
        .context("missing molt_dec_ref_obj export")?;
    let header_size = caller
        .get_export("molt_header_size")
        .and_then(Extern::into_func);
    let cancel_is_cancelled = caller
        .get_export("molt_cancel_token_is_cancelled")
        .and_then(Extern::into_func);
    Ok(RuntimeExports {
        stream_new,
        stream_send,
        stream_close,
        stream_drop,
        int_from_i64,
        exception_pending_fast,
        alloc,
        handle_resolve,
        dec_ref_obj,
        header_size,
        cancel_is_cancelled,
    })
}

pub(super) fn call_i64(func: &Func, caller: &mut Caller<HostState>, args: &[Val]) -> Result<i64> {
    let mut results = [Val::I64(0)];
    func.call(caller, args, &mut results)?;
    match results[0] {
        Val::I64(val) => Ok(val),
        _ => bail!("unexpected wasm result type"),
    }
}

pub(super) fn new_stream(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    capacity: i64,
) -> Result<u64> {
    if call_i64(&exports.exception_pending_fast, caller, &[])? != 0 {
        bail!("pending runtime exception before stream capacity allocation");
    }
    let capacity_bits = call_i64(&exports.int_from_i64, caller, &[Val::I64(capacity)])?;
    let mut stream = None;
    let result: Result<u64> = (|| {
        if call_i64(&exports.exception_pending_fast, caller, &[])? != 0 {
            bail!("stream capacity allocation failed");
        }
        let bits = call_i64(&exports.stream_new, caller, &[Val::I64(capacity_bits)])?;
        if call_i64(&exports.exception_pending_fast, caller, &[])? != 0 {
            bail!("stream allocation failed");
        }
        stream = Some(bits);
        Ok(bits as u64)
    })();
    if let Err(cleanup) =
        exports
            .dec_ref_obj
            .call(&mut *caller, &[Val::I64(capacity_bits)], &mut [])
    {
        let mut errors = Vec::new();
        if let Err(error) = result {
            errors.push(error.to_string());
        }
        errors.push(cleanup.to_string());
        if let Some(bits) = stream {
            // Opaque stream handles are not object-refcount owners.
            if let Err(error) = exports.stream_drop.call(caller, &[Val::I64(bits)], &mut []) {
                errors.push(error.to_string());
            }
        }
        bail!("{}", errors.join("; "));
    }
    result
}

fn alloc_temp_bytes(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    bytes: &[u8],
) -> Result<(u64, u64)> {
    let alloc_bits = call_i64(&exports.alloc, caller, &[Val::I64(bytes.len() as i64)])? as u64;
    if alloc_bits == 0 {
        bail!("molt_alloc failed");
    }
    let ptr_bits = call_i64(
        &exports.handle_resolve,
        caller,
        &[Val::I64(alloc_bits as i64)],
    )? as u64;
    if ptr_bits == 0 {
        bail!("molt_handle_resolve failed");
    }
    let header_size = if let Some(ref func) = exports.header_size {
        call_i64(func, caller, &[])? as u64
    } else {
        40
    };
    let payload_ptr = ptr_bits + header_size;
    memory.write(caller, payload_ptr as usize, bytes)?;
    Ok((alloc_bits, payload_ptr))
}

pub(super) fn send_stream_frame(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    payload: &[u8],
) -> Result<()> {
    let (alloc_bits, payload_ptr) = alloc_temp_bytes(caller, exports, memory, payload)?;
    let _ = call_i64(
        &exports.stream_send,
        caller,
        &[
            Val::I64(stream_bits as i64),
            Val::I32(payload_ptr as i32),
            Val::I64(payload.len() as i64),
        ],
    )?;
    exports
        .dec_ref_obj
        .call(caller, &[Val::I64(alloc_bits as i64)], &mut [])?;
    Ok(())
}

fn json_to_msgpack(value: &JsonValue) -> MsgpackValue {
    match value {
        JsonValue::Null => MsgpackValue::Nil,
        JsonValue::Bool(val) => MsgpackValue::from(*val),
        JsonValue::Number(num) => {
            if let Some(int) = num.as_i64() {
                MsgpackValue::from(int)
            } else if let Some(uint) = num.as_u64() {
                MsgpackValue::from(uint)
            } else if let Some(float) = num.as_f64() {
                MsgpackValue::from(float)
            } else {
                MsgpackValue::Nil
            }
        }
        JsonValue::String(val) => MsgpackValue::from(val.as_str()),
        JsonValue::Array(items) => MsgpackValue::Array(items.iter().map(json_to_msgpack).collect()),
        JsonValue::Object(map) => {
            let mut entries = Vec::with_capacity(map.len());
            for (key, val) in map {
                entries.push((MsgpackValue::from(key.as_str()), json_to_msgpack(val)));
            }
            MsgpackValue::Map(entries)
        }
    }
}

fn encode_msgpack_header(
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<Vec<u8>> {
    let mut map = Vec::new();
    map.push((MsgpackValue::from("status"), MsgpackValue::from(status)));
    map.push((MsgpackValue::from("codec"), MsgpackValue::from(codec)));
    if let Some(payload) = payload {
        map.push((
            MsgpackValue::from("payload"),
            MsgpackValue::Binary(payload.to_vec()),
        ));
    }
    if let Some(error) = error {
        map.push((MsgpackValue::from("error"), MsgpackValue::from(error)));
    }
    if let Some(metrics) = metrics {
        map.push((MsgpackValue::from("metrics"), json_to_msgpack(metrics)));
    }
    let mut out = Vec::new();
    write_value(&mut out, &MsgpackValue::Map(map))?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn send_stream_header(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    status: &str,
    codec: &str,
    payload: Option<&[u8]>,
    error: Option<&str>,
    metrics: Option<&JsonValue>,
) -> Result<()> {
    let header = encode_msgpack_header(status, codec, payload, error, metrics)?;
    send_stream_frame(caller, exports, memory, stream_bits, &header)
}

pub(super) fn send_stream_error(
    caller: &mut Caller<HostState>,
    exports: &RuntimeExports,
    memory: &Memory,
    stream_bits: u64,
    message: &str,
) -> Result<()> {
    send_stream_header(
        caller,
        exports,
        memory,
        stream_bits,
        "internal_error",
        "raw",
        None,
        Some(message),
        None,
    )?;
    exports
        .stream_close
        .call(caller, &[Val::I64(stream_bits as i64)], &mut [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_capacity_boxing_preserves_allocation_and_cleanup_custody() {
        let engine = build_engine().unwrap();
        for mode in 0..5 {
            let module = Module::new(
                &engine,
                format!(
                    r#"(module
                (import "host" "new_stream" (func $new_stream (result i64)))
                (global $pending (mut i64) (i64.const 0))
                (global $boxes (mut i32) (i32.const 0))
                (global $streams (mut i32) (i32.const 0))
                (global $dropped (mut i32) (i32.const 0))
                (func (export "molt_exception_pending_fast") (result i64) global.get $pending)
                (func (export "molt_int_from_i64") (param i64) (result i64)
                    local.get 0 i64.const 0 i64.ne if unreachable end
                    i32.const 1 global.set $boxes
                    i32.const {mode} i32.const 1 i32.eq
                    if i64.const 1 global.set $pending end
                    i64.const 123)
                (func (export "molt_stream_new") (param i64) (result i64)
                    local.get 0 i64.const 123 i64.ne if unreachable end
                    i32.const {mode} i32.const 2 i32.eq
                    i32.const {mode} i32.const 4 i32.eq i32.or
                    if i64.const 1 global.set $pending i64.const 999 return end
                    i32.const 1 global.set $streams i64.const 456)
                (func (export "molt_dec_ref_obj") (param i64)
                    local.get 0 i64.const 123 i64.ne if unreachable end
                    i32.const 0 global.set $boxes
                    i32.const {mode} i32.const 3 i32.ge_s if unreachable end)
                (func (export "molt_stream_drop") (param i64)
                    local.get 0 i64.const 456 i64.ne if unreachable end
                    i32.const 0 global.set $streams
                    i32.const 1 global.set $dropped)
                (func (export "molt_stream_close") (param i64))
                (func (export "molt_stream_send") (param i64 i32 i64) (result i64) i64.const 0)
                (func (export "molt_alloc") (param i64) (result i64) i64.const 0)
                (func (export "molt_handle_resolve") (param i64) (result i64) i64.const 0)
                (func (export "run") (result i64) call $new_stream)
                (func (export "boxes") (result i32) global.get $boxes)
                (func (export "streams") (result i32) global.get $streams)
                (func (export "dropped") (result i32) global.get $dropped))"#
                ),
            )
            .unwrap();
            let mut store = Store::new(&engine, crate::main_tests::test_host_state());
            let create = Func::wrap(
                &mut store,
                |mut caller: Caller<'_, HostState>| -> Result<i64> {
                    let exports = runtime_exports(&mut caller)?;
                    new_stream(&mut caller, &exports, 0).map(|bits| bits as i64)
                },
            );
            let instance = Instance::new(&mut store, &module, &[create.into()]).unwrap();
            let run = instance
                .get_typed_func::<(), i64>(&mut store, "run")
                .unwrap();
            let result = run.call(&mut store, ());
            assert_eq!(result.is_ok(), mode == 0, "mode={mode}");
            if mode == 0 {
                assert_eq!(result.unwrap(), 456);
                instance
                    .get_typed_func::<i64, ()>(&mut store, "molt_stream_drop")
                    .unwrap()
                    .call(&mut store, 456)
                    .unwrap();
            }
            for name in ["boxes", "streams"] {
                let count = instance
                    .get_typed_func::<(), i32>(&mut store, name)
                    .unwrap()
                    .call(&mut store, ())
                    .unwrap();
                assert_eq!(count, 0, "mode={mode}, owner={name}");
            }
            let dropped = instance
                .get_typed_func::<(), i32>(&mut store, "dropped")
                .unwrap()
                .call(&mut store, ())
                .unwrap();
            assert_eq!(dropped, i32::from(mode == 0 || mode == 3), "mode={mode}");
        }
    }
}
