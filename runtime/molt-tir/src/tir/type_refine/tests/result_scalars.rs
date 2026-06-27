use super::*;

// ---- Test 1: Constants resolve to concrete types ----
#[test]
fn constants_resolve_to_concrete_types() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(42)),
        make_op(OpCode::ConstFloat, vec![], vec![ValueId(1)], float_attr(PI)),
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(2)],
            str_attr("hello"),
        ),
        make_op(OpCode::ConstBool, vec![], vec![ValueId(3)], AttrDict::new()),
        make_op(OpCode::ConstNone, vec![], vec![ValueId(4)], AttrDict::new()),
        make_op(
            OpCode::ConstBytes,
            vec![],
            vec![ValueId(5)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 6);
    let refined = refine_types(&mut func);
    // All 6 values should be refined from DynBox to concrete types.
    assert_eq!(refined, 6);
}

// ---- Test 2: Arithmetic propagates types ----
#[test]
fn arithmetic_propagates_i64() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3); // two consts + one add result
}

#[test]
fn module_get_attr_result_stays_dynbox() {
    let ops = vec![
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(0)],
            str_attr("module_name"),
        ),
        make_op(
            OpCode::ConstStr,
            vec![],
            vec![ValueId(1)],
            str_attr("Point"),
        ),
        make_op(
            OpCode::ModuleGetAttr,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(refined, 2, "only the const_str operands refine to Str");
    assert_eq!(
        type_map.get(&ValueId(2)),
        Some(&TirType::DynBox),
        "module_get_attr result must not inherit the module operand type"
    );
}

#[test]
fn module_lookup_results_stay_dynbox() {
    for opcode in [
        OpCode::ModuleCacheGet,
        OpCode::ModuleGetGlobal,
        OpCode::ModuleGetName,
    ] {
        let operands = if opcode == OpCode::ModuleCacheGet {
            vec![ValueId(0)]
        } else {
            vec![ValueId(0), ValueId(1)]
        };
        let ops = vec![
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(0)],
                str_attr("module_name"),
            ),
            make_op(
                OpCode::ConstStr,
                vec![],
                vec![ValueId(1)],
                str_attr("answer"),
            ),
            make_op(opcode, operands, vec![ValueId(2)], AttrDict::new()),
        ];
        let mut func = single_block_func(ops, 3);
        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&ValueId(2)),
            Some(&TirType::DynBox),
            "{opcode:?} result must not inherit the module/name operand type"
        );
    }
}

// ---- Test 3: Mixed arithmetic promotes to F64 ----
#[test]
fn mixed_arithmetic_promotes_to_f64() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(1)],
            float_attr(2.0),
        ),
        make_op(
            OpCode::Add,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3);
}

// ---- Test 4: Comparison produces Bool ----
#[test]
fn comparison_produces_bool() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(1)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(2)),
        make_op(
            OpCode::Eq,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 3);
    let refined = refine_types(&mut func);
    assert_eq!(refined, 3);
}

/// Locks in the contract that `InplaceAdd`/`InplaceSub`/`InplaceMul`
/// participate in numeric arithmetic inference identically to their
/// regular `Add`/`Sub`/`Mul` counterparts. Without this, an
/// accumulator pattern like `total += i` (lowered as `InplaceAdd`)
/// stays at DynBox even when both operands are I64, causing the
/// native backend to coerce to a float lane and silently miscompile
/// the integer accumulator (printed bits look like a denormal float).
#[test]
fn inplace_add_typed_to_i64_for_int_operands() {
    let ops = vec![
        make_op(OpCode::ConstInt, vec![], vec![ValueId(0)], int_attr(10)),
        make_op(OpCode::ConstInt, vec![], vec![ValueId(1)], int_attr(20)),
        make_op(
            OpCode::InplaceAdd,
            vec![ValueId(0), ValueId(1)],
            vec![ValueId(2)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::InplaceSub,
            vec![ValueId(2), ValueId(1)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
        make_op(
            OpCode::InplaceMul,
            vec![ValueId(3), ValueId(0)],
            vec![ValueId(4)],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 5);
    let _refined = refine_types(&mut func);

    // Re-extract the type map post-refinement to inspect op result
    // types (block args were already covered by other tests).
    let env = extract_type_map(&func);
    assert_eq!(
        env.get(&ValueId(2)),
        Some(&TirType::I64),
        "InplaceAdd of (I64, I64) must produce I64"
    );
    assert_eq!(
        env.get(&ValueId(3)),
        Some(&TirType::I64),
        "InplaceSub of (I64, I64) must produce I64"
    );
    assert_eq!(
        env.get(&ValueId(4)),
        Some(&TirType::I64),
        "InplaceMul of (I64, I64) must produce I64"
    );
}

/// Round-8 regression: a FRESH-VALUE scalar-conversion `Copy`
/// (`int_from_obj`/`float_from_obj`) is NOT a transparent type alias of its
/// operand — it mints a NEW raw-register value whose type the conversion
/// determines. `int(t)` with `t: float` lowers to `Copy[int_from_obj](t)`; the
/// old `Copy => operand_types.first()` rule type-aliased it to `t`'s `F64`,
/// flooding the downstream integer accumulator (`total += int(t)`) with a
/// spurious float carrier → native `def_var` repr mismatch / LIR-verifier
/// branch-repr divergence (`os._seconds_float_to_sec_nsec`). A TRANSPARENT
/// alias (`copy_var`/bare `Copy`) MUST still propagate the operand type.
#[test]
fn int_from_obj_copy_of_float_is_i64_not_aliased_to_operand() {
    let int_from_obj_attr = {
        let mut a = AttrDict::new();
        a.insert(
            "_original_kind".into(),
            AttrValue::Str("int_from_obj".into()),
        );
        a
    };
    let copy_var_attr = {
        let mut a = AttrDict::new();
        a.insert("_original_kind".into(), AttrValue::Str("copy_var".into()));
        a
    };
    let ops = vec![
        // t = <float> (a const float stands in for the float parameter).
        make_op(
            OpCode::ConstFloat,
            vec![],
            vec![ValueId(0)],
            AttrDict::new(),
        ),
        // sec = int(t)  →  Copy[int_from_obj](t). MUST type to I64, not F64.
        make_op(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(1)],
            int_from_obj_attr,
        ),
        // total = 0; total += sec  →  the integer accumulator that mis-typed.
        make_op(OpCode::ConstInt, vec![], vec![ValueId(2)], int_attr(0)),
        make_op(
            OpCode::InplaceAdd,
            vec![ValueId(2), ValueId(1)],
            vec![ValueId(3)],
            AttrDict::new(),
        ),
        // A TRANSPARENT alias of the float MUST keep the operand's F64 type.
        make_op(
            OpCode::Copy,
            vec![ValueId(0)],
            vec![ValueId(4)],
            copy_var_attr,
        ),
    ];
    let mut func = single_block_func(ops, 5);
    refine_types(&mut func);
    let env = extract_type_map(&func);
    assert_eq!(
        env.get(&ValueId(1)),
        Some(&TirType::I64),
        "Copy[int_from_obj](F64) must produce I64 (a fresh int), NOT alias the float operand"
    );
    assert_eq!(
        env.get(&ValueId(3)),
        Some(&TirType::I64),
        "InplaceAdd(I64 accumulator, int(t)) must stay I64 — the accumulator must not float-contaminate"
    );
    assert_eq!(
        env.get(&ValueId(4)),
        Some(&TirType::F64),
        "a TRANSPARENT-alias Copy (copy_var) must still propagate operand 0's F64 type"
    );
}

/// The `copy_kind_raw_carrier_type` source of truth: raw-carrier scalar
/// conversions map to their precise scalar; every other `Copy` kind (including
/// heap-producing fresh values and transparent aliases) returns `None` so the
/// caller keeps operand-0 propagation. Pins the narrow scope that keeps the
/// heap-value type lattice byte-identical to the pre-fix behavior.
#[test]
fn raw_carrier_type_is_scoped_to_scalar_conversions() {
    use crate::tir::passes::alias_analysis::copy_kind_raw_carrier_type;
    assert_eq!(
        copy_kind_raw_carrier_type(Some("int_from_obj")),
        Some(TirType::I64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("int_from_str_of_obj")),
        Some(TirType::I64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("float_from_obj")),
        Some(TirType::F64)
    );
    assert_eq!(
        copy_kind_raw_carrier_type(Some("contains")),
        Some(TirType::Bool)
    );
    // Heap-producing fresh values → None (operand-0 propagation / DynBox floor).
    assert_eq!(copy_kind_raw_carrier_type(Some("str_from_obj")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("list_new")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("tuple_new")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("enumerate")), None);
    // Transparent aliases / bare Copy / unknown → None.
    assert_eq!(copy_kind_raw_carrier_type(Some("copy_var")), None);
    assert_eq!(copy_kind_raw_carrier_type(Some("guard_tag")), None);
    assert_eq!(copy_kind_raw_carrier_type(None), None);
}
