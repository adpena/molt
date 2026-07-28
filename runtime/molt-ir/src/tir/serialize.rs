//! Canonical full-TIR-function artifact serialization for the compiler cache.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::blocks::{BlockId, LoopBreakKind, LoopRole, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::types::TirType;
use super::values::{TirValue, ValueId};
use crate::ExecutionContextPolicy;

const TIR_FUNCTION_CACHE_MAGIC: &[u8] = b"MOLT:TIRFUNC:v2\0";

#[derive(Serialize, Deserialize)]
struct TirFunctionArtifact {
    name: String,
    execution_context: ExecutionContextPolicy,
    param_names: Vec<String>,
    param_types: Vec<TirType>,
    return_type: TirType,
    blocks: Vec<(BlockId, TirBlockArtifact)>,
    entry_block: BlockId,
    next_value: u32,
    next_block: u32,
    attrs: Vec<(String, AttrValue)>,
    value_types: Vec<(ValueId, TirType)>,
    has_exception_handling: bool,
    label_id_map: Vec<(u32, i64)>,
    loop_roles: Vec<(BlockId, LoopRole)>,
    loop_pairs: Vec<(BlockId, BlockId)>,
    loop_break_kinds: Vec<(BlockId, LoopBreakKind)>,
    loop_cond_blocks: Vec<(BlockId, BlockId)>,
}

#[derive(Serialize, Deserialize)]
struct TirBlockArtifact {
    id: BlockId,
    args: Vec<TirValue>,
    ops: Vec<TirOpArtifact>,
    terminator: Terminator,
}

#[derive(Serialize, Deserialize)]
struct TirOpArtifact {
    dialect: Dialect,
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: Vec<(String, AttrValue)>,
    source_span: Option<(u32, u32)>,
}

#[derive(Serialize)]
struct TirFunctionArtifactRef<'a> {
    name: &'a str,
    execution_context: ExecutionContextPolicy,
    param_names: &'a [String],
    param_types: &'a [TirType],
    return_type: &'a TirType,
    blocks: Vec<(BlockId, TirBlockArtifactRef<'a>)>,
    entry_block: BlockId,
    next_value: u32,
    next_block: u32,
    attrs: Vec<(&'a String, &'a AttrValue)>,
    value_types: Vec<(&'a ValueId, &'a TirType)>,
    has_exception_handling: bool,
    label_id_map: Vec<(&'a u32, &'a i64)>,
    loop_roles: Vec<(&'a BlockId, &'a LoopRole)>,
    loop_pairs: Vec<(&'a BlockId, &'a BlockId)>,
    loop_break_kinds: Vec<(&'a BlockId, &'a LoopBreakKind)>,
    loop_cond_blocks: Vec<(&'a BlockId, &'a BlockId)>,
}

#[derive(Serialize)]
struct TirBlockArtifactRef<'a> {
    id: BlockId,
    args: &'a [TirValue],
    ops: Vec<TirOpArtifactRef<'a>>,
    terminator: &'a Terminator,
}

#[derive(Serialize)]
struct TirOpArtifactRef<'a> {
    dialect: Dialect,
    opcode: OpCode,
    operands: &'a [ValueId],
    results: &'a [ValueId],
    attrs: Vec<(&'a String, &'a AttrValue)>,
    source_span: Option<(u32, u32)>,
}

fn sorted_entry_refs<K: Ord, V>(map: &HashMap<K, V>) -> Vec<(&K, &V)> {
    let mut entries = map.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    entries
}

fn keys_are_strictly_sorted<K: Ord, V>(entries: &[(K, V)]) -> bool {
    entries.windows(2).all(|window| window[0].0 < window[1].0)
}

impl TirFunctionArtifact {
    fn has_canonical_map_entries(&self) -> bool {
        keys_are_strictly_sorted(&self.blocks)
            && self.blocks.iter().all(|(key, block)| {
                *key == block.id
                    && block
                        .ops
                        .iter()
                        .all(|op| keys_are_strictly_sorted(&op.attrs))
            })
            && keys_are_strictly_sorted(&self.attrs)
            && keys_are_strictly_sorted(&self.value_types)
            && keys_are_strictly_sorted(&self.label_id_map)
            && keys_are_strictly_sorted(&self.loop_roles)
            && keys_are_strictly_sorted(&self.loop_pairs)
            && keys_are_strictly_sorted(&self.loop_break_kinds)
            && keys_are_strictly_sorted(&self.loop_cond_blocks)
    }
}

fn artifact_op_ref(op: &TirOp) -> TirOpArtifactRef<'_> {
    TirOpArtifactRef {
        dialect: op.dialect,
        opcode: op.opcode,
        operands: &op.operands,
        results: &op.results,
        attrs: sorted_entry_refs(&op.attrs),
        source_span: op.source_span,
    }
}

fn artifact_block_ref(block: &TirBlock) -> TirBlockArtifactRef<'_> {
    TirBlockArtifactRef {
        id: block.id,
        args: &block.args,
        ops: block.ops.iter().map(artifact_op_ref).collect(),
        terminator: &block.terminator,
    }
}

impl<'a> From<&'a TirFunction> for TirFunctionArtifactRef<'a> {
    fn from(function: &'a TirFunction) -> Self {
        let mut blocks = function
            .blocks
            .iter()
            .map(|(id, block)| (*id, artifact_block_ref(block)))
            .collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|(id, _)| *id);
        Self {
            name: &function.name,
            execution_context: function.execution_context,
            param_names: &function.param_names,
            param_types: &function.param_types,
            return_type: &function.return_type,
            blocks,
            entry_block: function.entry_block,
            next_value: function.next_value,
            next_block: function.next_block,
            attrs: sorted_entry_refs(&function.attrs),
            value_types: sorted_entry_refs(&function.value_types),
            has_exception_handling: function.has_exception_handling,
            label_id_map: sorted_entry_refs(&function.label_id_map),
            loop_roles: sorted_entry_refs(&function.loop_roles),
            loop_pairs: sorted_entry_refs(&function.loop_pairs),
            loop_break_kinds: sorted_entry_refs(&function.loop_break_kinds),
            loop_cond_blocks: sorted_entry_refs(&function.loop_cond_blocks),
        }
    }
}

impl From<TirOpArtifact> for TirOp {
    fn from(op: TirOpArtifact) -> Self {
        Self {
            dialect: op.dialect,
            opcode: op.opcode,
            operands: op.operands,
            results: op.results,
            attrs: op.attrs.into_iter().collect::<AttrDict>(),
            source_span: op.source_span,
        }
    }
}

impl From<TirBlockArtifact> for TirBlock {
    fn from(block: TirBlockArtifact) -> Self {
        Self {
            id: block.id,
            args: block.args,
            ops: block.ops.into_iter().map(TirOp::from).collect(),
            terminator: block.terminator,
        }
    }
}

impl From<TirFunctionArtifact> for TirFunction {
    fn from(function: TirFunctionArtifact) -> Self {
        Self {
            name: function.name,
            execution_context: function.execution_context,
            param_names: function.param_names,
            param_types: function.param_types,
            return_type: function.return_type,
            blocks: function
                .blocks
                .into_iter()
                .map(|(id, block)| (id, TirBlock::from(block)))
                .collect(),
            entry_block: function.entry_block,
            next_value: function.next_value,
            next_block: function.next_block,
            attrs: function.attrs.into_iter().collect(),
            value_types: function.value_types.into_iter().collect(),
            has_exception_handling: function.has_exception_handling,
            label_id_map: function.label_id_map.into_iter().collect(),
            loop_roles: function.loop_roles.into_iter().collect(),
            loop_pairs: function.loop_pairs.into_iter().collect(),
            loop_break_kinds: function.loop_break_kinds.into_iter().collect(),
            loop_cond_blocks: function.loop_cond_blocks.into_iter().collect(),
        }
    }
}

/// Serialize a complete [`TirFunction`] through a canonical key-sorted carrier.
pub fn serialize_tir_function(func: &TirFunction) -> Result<Vec<u8>, String> {
    let artifact = TirFunctionArtifactRef::from(func);
    let mut out = Vec::with_capacity(TIR_FUNCTION_CACHE_MAGIC.len() + 1024);
    out.extend_from_slice(TIR_FUNCTION_CACHE_MAGIC);
    let mut serializer = rmp_serde::Serializer::new(&mut out).with_struct_map();
    artifact
        .serialize(&mut serializer)
        .map_err(|error| format!("TIR function artifact serialization failed: {error}"))?;
    Ok(out)
}

/// Deserialize a cached [`TirFunction`] written by
/// [`serialize_tir_function`]. Corrupt or differently-versioned artifacts miss.
pub fn deserialize_tir_function(bytes: &[u8]) -> Option<TirFunction> {
    let payload = bytes.strip_prefix(TIR_FUNCTION_CACHE_MAGIC)?;
    if payload.is_empty() {
        return None;
    }
    let artifact = rmp_serde::from_slice::<TirFunctionArtifact>(payload).ok()?;
    if !artifact.has_canonical_map_entries() {
        return None;
    }
    let function = TirFunction::from(artifact);
    super::verify::verify_function(&function).ok()?;
    Some(function)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tir_function_artifact_preserves_execution_context() {
        let mut func = TirFunction::new("cached".into(), vec![TirType::DynBox], TirType::DynBox);
        func.execution_context = ExecutionContextPolicy::Inherited;
        let arg = func.blocks[&func.entry_block].args[0].id;
        func.blocks.get_mut(&func.entry_block).unwrap().terminator =
            Terminator::Return { values: vec![arg] };
        let bytes = serialize_tir_function(&func).expect("TIR artifact serialization");
        assert!(bytes.starts_with(TIR_FUNCTION_CACHE_MAGIC));
        let restored = deserialize_tir_function(&bytes).expect("TIR artifact roundtrip");
        assert_eq!(restored.name, func.name);
        assert_eq!(
            restored.execution_context,
            ExecutionContextPolicy::Inherited
        );
        assert_eq!(restored.entry_block, func.entry_block);
    }

    fn function_with_map_order(reverse: bool) -> TirFunction {
        fn ordered<T>(mut values: Vec<T>, reverse: bool) -> Vec<T> {
            if reverse {
                values.reverse();
            }
            values
        }

        let mut function = TirFunction::new(
            "deterministic".into(),
            vec![TirType::I64, TirType::F64],
            TirType::None,
        );
        let nan = f64::from_bits(0x7ff8_0000_0000_0042);
        let mut op_attrs = AttrDict::new();
        let attrs = vec![
            ("zeta".to_string(), AttrValue::Int(9)),
            ("f_value".to_string(), AttrValue::Float(nan)),
        ];
        for (key, value) in ordered(attrs, reverse) {
            op_attrs.insert(key, value);
        }
        let mut entry = function.blocks.remove(&function.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstFloat,
            operands: vec![],
            results: vec![ValueId(2)],
            attrs: op_attrs,
            source_span: Some((4, 8)),
        });
        entry.terminator = Terminator::Return { values: vec![] };
        function.next_value = 3;
        let second = TirBlock {
            id: BlockId(1),
            args: vec![],
            ops: vec![],
            terminator: Terminator::Unreachable,
        };
        for block in ordered(vec![entry, second], reverse) {
            function.blocks.insert(block.id, block);
        }
        function.next_block = 2;
        let function_attrs = vec![
            ("beta".to_string(), AttrValue::Bool(true)),
            ("alpha".to_string(), AttrValue::Bytes(vec![0, 255])),
        ];
        for (key, value) in ordered(function_attrs, reverse) {
            function.attrs.insert(key, value);
        }
        function.value_types.insert(ValueId(2), TirType::F64);
        let value_types = std::mem::take(&mut function.value_types)
            .into_iter()
            .collect::<Vec<_>>();
        for (key, value) in ordered(value_types, reverse) {
            function.value_types.insert(key, value);
        }
        for (key, value) in ordered(vec![(1, 90), (0, 20)], reverse) {
            function.label_id_map.insert(key, value);
        }
        for (key, value) in ordered(
            vec![
                (BlockId(1), LoopRole::LoopEnd),
                (BlockId(0), LoopRole::LoopHeader),
            ],
            reverse,
        ) {
            function.loop_roles.insert(key, value);
        }
        for (key, value) in ordered(
            vec![(BlockId(0), BlockId(1)), (BlockId(1), BlockId(0))],
            reverse,
        ) {
            function.loop_pairs.insert(key, value);
        }
        for (key, value) in ordered(
            vec![
                (BlockId(0), LoopBreakKind::BreakIfFalse),
                (BlockId(1), LoopBreakKind::BreakIfTrue),
            ],
            reverse,
        ) {
            function.loop_break_kinds.insert(key, value);
        }
        for (key, value) in ordered(
            vec![(BlockId(0), BlockId(0)), (BlockId(1), BlockId(1))],
            reverse,
        ) {
            function.loop_cond_blocks.insert(key, value);
        }
        function
    }

    #[test]
    fn tir_artifacts_are_canonical_across_hash_seeds_and_insertion_orders() {
        let first = function_with_map_order(false);
        let second = function_with_map_order(true);
        let first_bytes = serialize_tir_function(&first).unwrap();
        let second_bytes = serialize_tir_function(&second).unwrap();
        assert_eq!(first_bytes, second_bytes);
        let restored = deserialize_tir_function(&first_bytes).unwrap();
        let AttrValue::Float(nan) = &restored.blocks[&BlockId(0)].ops[0].attrs["f_value"] else {
            panic!("missing float attr")
        };
        assert_eq!(nan.to_bits(), 0x7ff8_0000_0000_0042);
    }

    fn encode_owned_artifact(artifact: &TirFunctionArtifact) -> Vec<u8> {
        let mut bytes = Vec::from(TIR_FUNCTION_CACHE_MAGIC);
        let mut serializer = rmp_serde::Serializer::new(&mut bytes).with_struct_map();
        artifact.serialize(&mut serializer).unwrap();
        bytes
    }

    fn decode_owned_artifact(function: &TirFunction) -> TirFunctionArtifact {
        let bytes = serialize_tir_function(function).unwrap();
        rmp_serde::from_slice(bytes.strip_prefix(TIR_FUNCTION_CACHE_MAGIC).unwrap()).unwrap()
    }

    #[test]
    fn canonical_looking_artifacts_fail_closed_on_duplicate_keys_identity_and_invalid_tir() {
        let function = function_with_map_order(false);

        let mut duplicate = decode_owned_artifact(&function);
        duplicate.attrs.insert(0, duplicate.attrs[0].clone());
        assert!(deserialize_tir_function(&encode_owned_artifact(&duplicate)).is_none());

        let mut mismatched_block = decode_owned_artifact(&function);
        mismatched_block.blocks[0].1.id = BlockId(99);
        assert!(deserialize_tir_function(&encode_owned_artifact(&mismatched_block)).is_none());

        let mut invalid_tir = decode_owned_artifact(&function);
        invalid_tir.blocks[0].1.terminator = Terminator::Return {
            values: vec![ValueId(999)],
        };
        assert!(deserialize_tir_function(&encode_owned_artifact(&invalid_tir)).is_none());
    }

    #[test]
    fn tir_function_artifact_rejects_wrong_magic_and_corruption() {
        assert!(deserialize_tir_function(b"[]").is_none());
        assert!(deserialize_tir_function(TIR_FUNCTION_CACHE_MAGIC).is_none());
        let mut corrupt = Vec::from(TIR_FUNCTION_CACHE_MAGIC);
        corrupt.extend_from_slice(b"not-messagepack");
        assert!(deserialize_tir_function(&corrupt).is_none());
    }
}
