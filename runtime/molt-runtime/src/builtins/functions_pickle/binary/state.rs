// Binary pickle dump state, memo table, and opcode emitters.

use super::*;

pub(crate) struct PickleDumpState {
    pub(crate) protocol: i64,
    pub(crate) out: Vec<u8>,
    pub(crate) memo: HashMap<u64, u32>,
    pub(crate) next_memo: u32,
    pub(crate) depth: usize,
    pub(crate) persistent_id_bits: Option<u64>,
    pub(crate) buffer_callback_bits: Option<u64>,
    pub(crate) dispatch_table_bits: Option<u64>,
}

impl PickleDumpState {
    pub(crate) fn new(
        protocol: i64,
        persistent_id_bits: Option<u64>,
        buffer_callback_bits: Option<u64>,
        dispatch_table_bits: Option<u64>,
    ) -> Self {
        Self {
            protocol,
            out: Vec::with_capacity(256),
            memo: HashMap::new(),
            next_memo: 0,
            depth: 0,
            persistent_id_bits,
            buffer_callback_bits,
            dispatch_table_bits,
        }
    }

    pub(crate) fn push(&mut self, op: u8) {
        self.out.push(op);
    }

    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
}

pub(crate) fn pickle_option_callable_bits(
    _py: &crate::PyToken<'_>,
    maybe_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(maybe_bits).is_none() {
        return Ok(None);
    }
    if !is_truthy(_py, obj_from_bits(molt_is_callable(maybe_bits))) {
        let message = format!("pickle {name} must be callable");
        return Err(raise_exception::<u64>(_py, "TypeError", &message));
    }
    Ok(Some(maybe_bits))
}

pub(crate) fn pickle_emit_u32_le(state: &mut PickleDumpState, value: u32) {
    state.extend(&value.to_le_bytes());
}

pub(crate) fn pickle_emit_u64_le(state: &mut PickleDumpState, value: u64) {
    state.extend(&value.to_le_bytes());
}

fn pickle_emit_memo_put(state: &mut PickleDumpState, index: u32) {
    if state.protocol >= PICKLE_PROTO_4 {
        state.push(PICKLE_OP_MEMOIZE);
        return;
    }
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINPUT);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINPUT);
        pickle_emit_u32_le(state, index);
    }
}

pub(crate) fn pickle_emit_memo_get(state: &mut PickleDumpState, index: u32) {
    if index <= u8::MAX as u32 {
        state.push(PICKLE_OP_BINGET);
        state.push(index as u8);
    } else {
        state.push(PICKLE_OP_LONG_BINGET);
        pickle_emit_u32_le(state, index);
    }
}

fn pickle_memo_key(bits: u64) -> Option<u64> {
    let obj = obj_from_bits(bits);
    if obj.as_ptr().is_some() {
        Some(bits)
    } else {
        None
    }
}

pub(crate) fn pickle_memo_lookup(state: &PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    state.memo.get(&key).copied()
}

fn pickle_memo_store(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    let key = pickle_memo_key(bits)?;
    if let Some(found) = state.memo.get(&key).copied() {
        return Some(found);
    }
    let index = state.next_memo;
    state.next_memo = state.next_memo.saturating_add(1);
    state.memo.insert(key, index);
    pickle_emit_memo_put(state, index);
    Some(index)
}

pub(crate) fn pickle_memo_store_if_absent(state: &mut PickleDumpState, bits: u64) -> Option<u32> {
    if let Some(found) = pickle_memo_lookup(state, bits) {
        return Some(found);
    }
    pickle_memo_store(state, bits)
}

pub(crate) fn pickle_emit_proto_header(state: &mut PickleDumpState) {
    state.push(PICKLE_OP_PROTO);
    state.push(state.protocol as u8);
}

pub(crate) fn pickle_emit_global_opcode(state: &mut PickleDumpState, module: &str, name: &str) {
    state.push(PICKLE_OP_GLOBAL);
    state.extend(module.as_bytes());
    state.push(b'\n');
    state.extend(name.as_bytes());
    state.push(b'\n');
}
