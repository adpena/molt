use std::collections::HashMap;

use crate::ir::OpIR;

use super::super::call_targets::gpu_runtime_symbol_for_simple_kind;
use super::super::dominators;
use super::super::op_kinds_generated::{
    kind_to_opcode_table, opcode_ssa_s_value_attr_key_table,
    simpleir_first_trailing_result_arg_table, simpleir_kind_is_async_work_poll,
    simpleir_kind_may_carry_async_work_poll_marker, simpleir_kind_preserves_original_kind_for_ssa,
};
use super::super::ops::{ASYNC_WORK_POLL_ATTR, AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::super::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};
use super::super::types::TirType;
use super::super::values::ValueId;
use super::variables::{
    is_variable, simple_ir_ssa_result_count, simple_var_field_is_transport_fact,
    simple_var_field_is_value_operand,
};
use super::*;

impl<'a> SsaContext<'a> {
    /// Reserved singleton reads and ordinary names have one resolution path
    /// across argument, var, and terminator fields. Reserved `none` is never a
    /// mutable stack entry or a textual string literal.
    pub(super) fn resolve_known_or_reserved_operand(
        &mut self,
        op_idx: usize,
        name: &str,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> Option<ValueId> {
        if name == "none" {
            Some(self.materialize_simple_literal(op_idx, name))
        } else if is_variable(name) {
            self.resolve_known_var(name, var_stacks)
        } else {
            None
        }
    }

    fn materialize_simple_literal(&mut self, op_idx: usize, value: &str) -> ValueId {
        let mut attrs = AttrDict::new();
        let opcode = if value == "none" {
            OpCode::ConstNone
        } else if let Ok(value) = value.parse::<i64>() {
            attrs.insert("value".into(), AttrValue::Int(value));
            OpCode::ConstInt
        } else if let Ok(value) = value.parse::<f64>() {
            attrs.insert("f_value".into(), AttrValue::Float(value));
            OpCode::ConstFloat
        } else {
            // Existing nonnumeric metadata literals (for example class names)
            // remain strings; this does not introduce boolean-token syntax.
            attrs.insert("s_value".into(), AttrValue::Str(value.to_string()));
            OpCode::ConstStr
        };
        let result = self.fresh_value_typed();
        let mut constant = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![result],
            attrs,
            source_span: None,
        };
        self.stamp_source_identity(&mut constant, op_idx);
        self.pending_inline_consts.push(constant);
        result
    }

    pub(super) fn translate_op(
        &mut self,
        op_idx: usize,
        op: &OpIR,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> TirOp {
        let opcode = kind_to_opcode(&op.kind);
        // Resolve operands from args.
        // SimpleIR args can be variable names OR inline constants (e.g., "1", "3.14").
        // Variables resolve via var_stacks; constants get a fresh ConstInt/ConstFloat value.
        let mut operands = Vec::new();
        if let Some(args) = &op.args {
            let read_arity = simpleir_first_trailing_result_arg_table(op.kind.as_str())
                .unwrap_or(args.len())
                .min(args.len());
            for a in args.iter().take(read_arity) {
                let value = self
                    .resolve_known_or_reserved_operand(op_idx, a, var_stacks)
                    .unwrap_or_else(|| self.materialize_simple_literal(op_idx, a));
                operands.push(value);
            }
        }
        // If `var` is an input (not a local-slot mutation target or transport
        // spelling), resolve it too. For `copy_var`/`load_var`, an explicit
        // args[0] is the value source and `var` is local-name transport.
        let mut var_operand_index = None;
        if simple_var_field_is_value_operand(op)
            && let Some(v) = &op.var
            && let Some(vid) = self.resolve_known_or_reserved_operand(op_idx, v, var_stacks)
        {
            var_operand_index = Some(operands.len());
            operands.push(vid);
        }
        if dominators::is_exception_transfer_edge(opcode)
            && let Some(label_id) = op.value
            && let Some(target_bid) = self.block_for_label(label_id)
        {
            // Exception-transfer operands are the target block's serialized
            // SSA environment, not ordinary SimpleIR value operands. Frontend
            // transfer ops have no explicit data operands; replacing rather
            // than appending makes this one authority for CheckException,
            // TryStart, and every generated wire alias.
            operands.clear();
            var_operand_index = None;
            operands.extend(self.collect_branch_args(target_bid, var_stacks));
        }

        // Create result value if this op produces an output.
        let mut results = Vec::new();
        let result_count = simple_ir_ssa_result_count(op);
        results.reserve(result_count);
        for _ in 0..result_count {
            results.push(self.fresh_value_typed());
        }

        // Build attrs from literal values on the op.
        let mut attrs = AttrDict::new();
        if let Some(v) = op.value {
            // ConstBool values must be stored as AttrValue::Bool so that
            // downstream passes (SCCP, canonicalize, GVN) can read the
            // boolean constant correctly.  The SSA lift previously stored
            // all values as AttrValue::Int, which made ConstBool(True)
            // and ConstBool(False) indistinguishable to passes that only
            // pattern-matched on AttrValue::Bool.
            if op.kind == "const_bool" {
                attrs.insert("value".into(), AttrValue::Bool(v != 0));
            } else {
                attrs.insert("value".into(), AttrValue::Int(v));
            }
        }
        if let Some(v) = op.f_value {
            attrs.insert("f_value".into(), AttrValue::Float(v));
        }
        if let Some(ref v) = op.s_value {
            attrs.insert("s_value".into(), AttrValue::Str(v.clone()));
        }
        if op.s_value.is_none()
            && let Some(symbol) = gpu_runtime_symbol_for_simple_kind(op.kind.as_str())
        {
            attrs.insert("s_value".into(), AttrValue::Str(symbol.to_string()));
        }
        if let Some(ref v) = op.bytes {
            attrs.insert("bytes".into(), AttrValue::Bytes(v.clone()));
        }
        // Preserve additional SimpleIR metadata fields that the native backend
        // reads on specific op kinds (task kind/closure size, container_type, var).
        // Without these, passthrough ops lose critical information.
        if let Some(ref v) = op.task_kind {
            attrs.insert("task_kind".into(), AttrValue::Str(v.clone()));
        }
        if let Some(v) = op.task_closure_size {
            attrs.insert("task_closure_size".into(), AttrValue::Int(v));
        }
        if let Some(ref v) = op.container_type {
            attrs.insert("container_type".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.native_callable_export {
            attrs.insert("native_callable_export".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.native_callable_binding {
            attrs.insert("native_callable_binding".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.native_callable_symbol {
            attrs.insert("native_callable_symbol".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.native_callable_abi {
            attrs.insert("native_callable_abi".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.builtin_name {
            attrs.insert("builtin_name".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.runtime_symbol {
            attrs.insert("runtime_symbol".into(), AttrValue::Str(v.clone()));
        }
        if op.runtime_requirement_bits != 0 {
            attrs.insert(
                "runtime_requirement_bits".into(),
                AttrValue::Int(i64::from(op.runtime_requirement_bits)),
            );
        }
        if op.passes_execution_context {
            attrs.insert("passes_execution_context".into(), AttrValue::Bool(true));
        }
        // Finalizer fact for `object_new_bound`: the instance's class defines
        // `__del__` (frontend-resolved through the MRO, excluding `object`). The
        // escape pass reads this to keep the instance heap-allocated with a live
        // refcount — never stack-promoting it to an IMMORTAL object and never
        // stripping its IncRef/DecRef — so the finalizer-aware `dec_ref_ptr`
        // dispatches `__del__` at the last reference drop.
        if op.defines_del == Some(true) {
            attrs.insert("defines_del".into(), AttrValue::Bool(true));
        }
        // Named-local fact (#58): generic lift, same shape as `defines_del`.
        if op.bound_local == Some(true) {
            attrs.insert("bound_local".into(), AttrValue::Bool(true));
        }
        if let Some(ref out) = op.out {
            attrs.insert("_simple_out".into(), AttrValue::Str(out.clone()));
        }
        let mut positional_results = false;
        visit_simple_ir_results(op, |result| {
            positional_results |= result.field == SimpleIrResultField::Var;
            if positional_results && let Some(name) = result.name {
                let index = match result.field {
                    SimpleIrResultField::Var => 0,
                    SimpleIrResultField::Out => 1,
                    SimpleIrResultField::Arg(_) => {
                        unreachable!("fixed results cannot carry trailing outputs")
                    }
                };
                attrs.insert(
                    format!("_simple_result_{index}"),
                    AttrValue::Str(name.to_string()),
                );
            }
        });
        // Preserve only the structural class-id hint needed by object
        // allocation round-trips. Scalar `fast_int` / `fast_float` flags are
        // SimpleIR transport metadata and must not become TIR attributes; TIR
        // scalar authority lives in `value_types` and the refined LIR facts.
        if let Some(ref th) = op.type_hint {
            attrs.insert("_type_hint".into(), AttrValue::Str(th.clone()));
            // Type-refine result values from the frontend's hint.
            // Currently we only refine to `UserClass` at SSA lift; builtin
            // scalar refinement is the responsibility of the type-refine pass
            // and function-owned `value_types`, not legacy transport hints.
            //
            // UserClass refinement is the *live* use of
            // `TirType::UserClass` — every typed-class allocation
            // (`OBJECT_NEW_BOUND`, dataclass instantiation, etc.)
            // carries a `type_hint` whose value is the qualified
            // class name.  Refining DynBox → UserClass(name) lets
            // downstream passes (escape analysis, devirt, GVN)
            // reason about class identity without parsing the
            // attr string at every call site.
            //
            // Soundness: `from_type_hint` returns DynBox for any
            // non-identifier or built-in tag, so we only refine
            // when the hint is a plain class name.  Joining a
            // UserClass with DynBox at a phi collapses to DynBox
            // (covered by the `meet` lattice), so type-erased
            // exception handler args stay sound.
            let refined = TirType::from_type_hint(th);
            if matches!(refined, TirType::UserClass(_)) {
                for &result in &results {
                    self.value_types.insert(result, refined.clone());
                }
            }
        }
        if op.async_work_poll {
            assert!(
                simpleir_kind_may_carry_async_work_poll_marker(&op.kind),
                "SimpleIR op {:?} cannot carry the async-work poll marker",
                op.kind
            );
        }
        if op.async_work_poll || simpleir_kind_is_async_work_poll(&op.kind) {
            attrs.insert(ASYNC_WORK_POLL_ATTR.into(), AttrValue::Bool(true));
        }

        if std::env::var("MOLT_TRACE_SSA_IMPORT").as_deref() == Ok("1") && opcode == OpCode::Import
        {
            eprintln!(
                "SSA import trace: func={} kind={} args={:?} var={:?} out={:?} operands={:?}",
                self.func_name, op.kind, op.args, op.var, op.out, operands
            );
        }

        // Opcode-specific attr key aliases: the lowering reads SimpleIR's
        // `s_value` under generated stable names. The registry owns opcode
        // membership; SSA owns copying the live attr payload.
        if let Some(ref v) = op.s_value
            && let Some(attr_key) = opcode_ssa_s_value_attr_key_table(opcode)
        {
            attrs.insert(attr_key.into(), AttrValue::Str(v.clone()));
        }

        // range_new maps to CallBuiltin but has no s_value to provide the
        // callee name.  Set it explicitly so downstream passes (range_devirt)
        // can pattern-match on name = "range".
        if op.kind == "range_new" && !attrs.contains_key("name") {
            attrs.insert("name".into(), AttrValue::Str("range".into()));
        }

        // Preserve the SimpleIR `var` spelling as transport metadata for
        // re-emission. For `copy_var`/`load_var` it is both resolved into an SSA
        // operand above and carried here as the original local-name fact; the
        // operand is value authority, `_var` is stream-identity authority.
        if simple_var_field_is_transport_fact(op.kind.as_str())
            && let Some(ref v) = op.var
        {
            attrs.insert("_var".into(), AttrValue::Str(v.clone()));
            // Record only a value actually resolved above. Unresolved var
            // spellings remain transport metadata; lowering must
            // never guess that the final positional argument came from var.
            if let Some(index) = var_operand_index {
                attrs.insert("_simple_var_operand".into(), AttrValue::Int(index as i64));
            }
        }

        // Preserve `_original_kind` for unknown Copy fallbacks and for mapped
        // spellings whose non-canonical name is semantically visible to
        // round-trip/backends. The generated predicate owns the mapped spelling
        // set; unknown fallback preservation stays here because SSA is the
        // backstop for kinds with no first-class opcode.
        let mapped_kind = kind_to_opcode_table(op.kind.as_str()).is_some();
        if (opcode == OpCode::Copy && !mapped_kind)
            || simpleir_kind_preserves_original_kind_for_ssa(op.kind.as_str())
        {
            attrs.insert("_original_kind".into(), AttrValue::Str(op.kind.clone()));
        }

        // The concrete class authoring a typed-slot field op's byte-offset
        // (`store`/`load`/`guarded_field_get`/`guarded_field_set`). Carried through TIR so
        // the alias oracle (`region_of`) can assign a class+offset `TypedField`
        // memory region. The frontend emits these offset-based forms only when
        // the object's class is proven at the op (runtime version-guard for the
        // `guarded_field_*` forms, static type inference for the plain forms), so
        // the class is the layout authority for `value` (the offset).
        if let Some(ref class) = op.class_name {
            attrs.insert("_class".into(), AttrValue::Str(class.clone()));
        }

        let mut tir_op = TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        };
        self.stamp_source_identity(&mut tir_op, op_idx);
        tir_op
    }
}

/// Map a SimpleIR `kind` string to a TIR `OpCode`.
///
/// The kind→opcode table is the single-source-of-truth op-kind registry
/// (`runtime/molt-ir/src/tir/op_kinds.toml`, generated into
/// [`crate::tir::op_kinds_generated::kind_to_opcode_table`]; see
/// `docs/design/foundation/25_op_kind_registry.md`). A kind with no first-class
/// opcode falls back to `OpCode::Copy` (carrying its spelling in
/// `_original_kind`), exactly as before — this is the runtime backstop the
/// registry's sync test (`tests/test_gen_op_kinds.py`) and the drift audit
/// (`tools/audit_op_kinds.py --check`) keep statically total for known kinds.
fn kind_to_opcode(kind: &str) -> OpCode {
    kind_to_opcode_table(kind).unwrap_or(OpCode::Copy)
}

#[cfg(test)]
mod positional_result_tests {
    use super::*;
    use crate::tir::cfg::CFG;

    #[test]
    fn fixed_results_allocate_discarded_slots_and_keep_surviving_return_identity() {
        for kind in ["checked_add", "checked_mul", "iter_next_unboxed"] {
            for discarded in [None, Some("none")] {
                for (var, out, returned, index) in [
                    (discarded, Some("flag"), Some("flag"), 1),
                    (Some("value"), discarded, Some("value"), 0),
                    (discarded, discarded, None, 0),
                ] {
                    let ops = vec![
                        OpIR {
                            kind: "const_int".into(),
                            out: Some("source".into()),
                            value: Some(1),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: kind.into(),
                            var: var.map(str::to_string),
                            out: out.map(str::to_string),
                            args: Some(if kind == "iter_next_unboxed" {
                                vec!["source".into()]
                            } else {
                                vec!["source".into(), "source".into()]
                            }),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: if returned.is_some() {
                                "ret"
                            } else {
                                "ret_void"
                            }
                            .into(),
                            args: returned.map(|name| vec![name.to_string()]),
                            ..OpIR::default()
                        },
                    ];
                    let cfg = CFG::build(&ops);
                    let output = super::super::convert_to_ssa(&cfg, &ops);
                    let op = output
                        .blocks
                        .iter()
                        .flat_map(|block| &block.ops)
                        .find(|op| op.opcode == kind_to_opcode(kind))
                        .unwrap();
                    assert_eq!(op.results.len(), 2, "{kind}");
                    if let Some(name) = returned {
                        assert_eq!(
                            op.attrs.get(&format!("_simple_result_{index}")),
                            Some(&AttrValue::Str(name.into()))
                        );
                        let returned = output
                            .blocks
                            .iter()
                            .find_map(|block| match &block.terminator {
                                crate::tir::blocks::Terminator::Return { values } => {
                                    values.first().copied()
                                }
                                _ => None,
                            })
                            .expect("returned surviving result");
                        assert_eq!(returned, op.results[index], "{kind}");
                    }
                }
            }
        }
    }
}
