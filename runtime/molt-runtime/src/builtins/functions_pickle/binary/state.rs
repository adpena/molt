// Binary pickle dump state, memo table, and opcode emitters.

use super::*;
use crate::BuiltinExc;

pub(crate) struct PickleDumpState {
    pub(crate) protocol: i64,
    pub(crate) out: Vec<u8>,
    pub(crate) memo: HashMap<u64, u32>,
    pub(crate) next_memo: u32,
    pub(crate) depth: usize,
    pub(crate) persistent_id_bits: Option<u64>,
    pub(crate) buffer_callback_bits: Option<u64>,
    pub(crate) dispatch_table_bits: Option<u64>,
    /// Protocol 4 framing (PEP 3154), as CPython's pickler frames output
    /// (`Modules/_pickle.c`: _Pickler_Write, _Pickler_OpcodeBoundary,
    /// _Pickler_CommitFrame, _Pickler_write_bytes).
    framing: bool,
    frame_start: Option<usize>,
}

const PICKLE_FRAME_HEADER_SIZE: usize = 9;
const PICKLE_FRAME_SIZE_MIN: usize = 4;
const PICKLE_FRAME_SIZE_TARGET: usize = 64 * 1024;

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
            framing: false,
            frame_start: None,
        }
    }

    pub(crate) fn push(&mut self, op: u8) {
        self.open_frame();
        self.out.push(op);
    }

    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.open_frame();
        self.out.extend_from_slice(bytes);
    }

    /// Frame everything written after the protocol header from protocol 4.
    pub(crate) fn start_framing(&mut self) {
        self.framing = self.protocol >= PICKLE_PROTO_4;
    }

    fn open_frame(&mut self) {
        if self.framing && self.frame_start.is_none() {
            self.frame_start = Some(self.out.len());
            self.out.extend_from_slice(&[0u8; PICKLE_FRAME_HEADER_SIZE]);
        }
    }

    /// Called as each object's save starts: a frame that reached the target
    /// size is committed, and the next write opens a new one.
    pub(crate) fn opcode_boundary(&mut self) {
        if let Some(start) = self.frame_start
            && self.out.len() - start - PICKLE_FRAME_HEADER_SIZE >= PICKLE_FRAME_SIZE_TARGET
        {
            self.commit_frame();
        }
    }

    /// Write the open frame's header, or drop it when the frame is too short
    /// to be worth one.
    pub(crate) fn commit_frame(&mut self) {
        let Some(start) = self.frame_start.take() else {
            return;
        };
        let frame_len = self.out.len() - start - PICKLE_FRAME_HEADER_SIZE;
        if frame_len >= PICKLE_FRAME_SIZE_MIN {
            self.out[start] = PICKLE_OP_FRAME;
            self.out[start + 1..start + PICKLE_FRAME_HEADER_SIZE]
                .copy_from_slice(&(frame_len as u64).to_le_bytes());
        } else {
            self.out.drain(start..start + PICKLE_FRAME_HEADER_SIZE);
        }
    }

    /// A length-prefixed payload; one of at least the frame target size is
    /// written outside any frame.
    pub(crate) fn write_payload(&mut self, header: &[u8], data: &[u8]) {
        if data.len() >= PICKLE_FRAME_SIZE_TARGET {
            self.commit_frame();
            let framing = std::mem::replace(&mut self.framing, false);
            self.extend(header);
            self.extend(data);
            self.framing = framing;
        } else {
            self.extend(header);
            self.extend(data);
        }
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
    if !is_truthy(
        _py,
        obj_from_bits(molt_is_callable_attached_body(_py, maybe_bits)),
    ) {
        let message = format!("pickle {name} must be callable");
        return Err(raise_exception::<u64>(_py, BuiltinExc::TypeError, &message));
    }
    Ok(Some(maybe_bits))
}

pub(crate) fn pickle_emit_u32_le(state: &mut PickleDumpState, value: u32) {
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
