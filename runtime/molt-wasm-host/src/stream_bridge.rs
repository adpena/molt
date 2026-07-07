use super::*;

pub(super) struct RuntimeExports {
    pub(super) stream_new: Func,
    pub(super) stream_send: Func,
    pub(super) stream_close: Func,
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
