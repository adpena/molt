use std::collections::HashMap;

use crate::ir::OpIR;

use super::super::call_targets::gpu_runtime_symbol_for_simple_kind;
use super::super::op_kinds_generated::{
    kind_to_opcode_table, opcode_ssa_s_value_attr_key_table,
    simpleir_kind_preserves_original_kind_for_ssa,
};
use super::super::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use super::super::types::TirType;
use super::super::values::ValueId;
use super::variables::{
    is_variable, simple_var_field_is_transport_fact, simple_var_field_is_value_operand,
};
use super::*;

impl<'a> SsaContext<'a> {
    pub(super) fn translate_op(
        &mut self,
        op_idx: usize,
        op: &OpIR,
        var_stacks: &HashMap<String, Vec<ValueId>>,
    ) -> TirOp {
        // Resolve operands from args.
        // SimpleIR args can be variable names OR inline constants (e.g., "1", "3.14").
        // Variables resolve via var_stacks; constants get a fresh ConstInt/ConstFloat value.
        let mut operands = Vec::new();
        if let Some(args) = &op.args {
            let args_iter: Box<dyn Iterator<Item = &String> + '_> = if op.kind == "unpack_sequence"
            {
                Box::new(args.iter().take(1))
            } else {
                Box::new(args.iter())
            };
            for a in args_iter {
                if let Some(vid) = self.resolve_known_var(a, var_stacks) {
                    // Resolved as a variable
                    operands.push(vid);
                } else if let Ok(int_val) = a.parse::<i64>() {
                    // Inline integer constant — emit a ConstInt op before the current op
                    let vid = self.fresh_value_typed();
                    let mut attrs = AttrDict::new();
                    attrs.insert("value".into(), AttrValue::Int(int_val));
                    let mut const_op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstInt,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                } else if let Ok(float_val) = a.parse::<f64>() {
                    // Inline float constant
                    let vid = self.fresh_value_typed();
                    let mut attrs = AttrDict::new();
                    attrs.insert("f_value".into(), AttrValue::Float(float_val));
                    let mut const_op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstFloat,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                } else {
                    // Unresolved non-numeric arg — treat as string constant
                    // (e.g., class names in isinstance, function names in call)
                    let vid = self.fresh_value_typed();
                    let mut attrs = AttrDict::new();
                    attrs.insert("s_value".into(), AttrValue::Str(a.clone()));
                    let mut const_op = TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::ConstStr,
                        operands: vec![],
                        results: vec![vid],
                        attrs,
                        source_span: None,
                    };
                    self.stamp_source_site(&mut const_op, op_idx);
                    self.pending_inline_consts.push(const_op);
                    operands.push(vid);
                }
            }
        }
        // If `var` is an input (not a local-slot mutation target or transport
        // spelling), resolve it too. For `copy_var`/`load_var`, an explicit
        // args[0] is the value source and `var` is local-name transport.
        if simple_var_field_is_value_operand(op)
            && let Some(v) = &op.var
            && is_variable(v)
            && let Some(vid) = self.resolve_known_var(v, var_stacks)
        {
            operands.push(vid);
        }
        if op.kind == "check_exception"
            && let Some(label_id) = op.value
            && let Some(target_bid) = self.block_for_label(label_id)
        {
            operands.extend(self.collect_branch_args(target_bid, var_stacks));
        }

        // Create result value if this op produces an output.
        let mut results = Vec::new();
        for _ in self.get_def_vars(op) {
            let vid = self.fresh_value_typed();
            results.push(vid);
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
        // reads on specific op kinds (task_kind, container_type, ic_index, var).
        // Without these, passthrough ops lose critical information.
        if let Some(ref v) = op.task_kind {
            attrs.insert("task_kind".into(), AttrValue::Str(v.clone()));
        }
        if let Some(ref v) = op.container_type {
            attrs.insert("container_type".into(), AttrValue::Str(v.clone()));
        }
        if let Some(v) = op.ic_index {
            attrs.insert("ic_index".into(), AttrValue::Int(v));
        }
        if let Some(ref v) = op.effect_proof {
            attrs.insert("effect_proof".into(), AttrValue::Str(v.clone()));
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
        if op.kind == "iter_next_unboxed" || op.kind == "checked_add" || op.kind == "checked_mul" {
            if let Some(ref value_out) = op.var {
                attrs.insert("_simple_result_0".into(), AttrValue::Str(value_out.clone()));
            }
            if let Some(ref done_out) = op.out {
                attrs.insert("_simple_result_1".into(), AttrValue::Str(done_out.clone()));
            }
        }
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
        let opcode = kind_to_opcode(&op.kind);

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
        if let Some(ref v) = op.s_value {
            if let Some(attr_key) = opcode_ssa_s_value_attr_key_table(opcode) {
                attrs.insert(attr_key.into(), AttrValue::Str(v.clone()));
            }
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
        // (`store`/`store_init`/`load`/`guarded_field_*`). Carried through TIR so
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
        self.stamp_source_site(&mut tir_op, op_idx);
        tir_op
    }
}

/// Map a SimpleIR `kind` string to a TIR `OpCode`.
///
/// The kind→opcode table is the single-source-of-truth op-kind registry
/// (`runtime/molt-tir/src/tir/op_kinds.toml`, generated into
/// [`crate::tir::op_kinds_generated::kind_to_opcode_table`]; see
/// `docs/design/foundation/25_op_kind_registry.md`). A kind with no first-class
/// opcode falls back to `OpCode::Copy` (carrying its spelling in
/// `_original_kind`), exactly as before — this is the runtime backstop the
/// registry's sync test (`tests/test_gen_op_kinds.py`) and the drift audit
/// (`tools/audit_op_kinds.py --check`) keep statically total for known kinds.
fn kind_to_opcode(kind: &str) -> OpCode {
    kind_to_opcode_table(kind).unwrap_or(OpCode::Copy)
}
