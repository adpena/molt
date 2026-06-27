use super::*;

// ---- Test: parse_return_type_str routes through TirType::from_type_hint ----
/// Pin the contract that `parse_return_type_str` uses the
/// centralized `TirType::from_type_hint` helper, so any future
/// hint added there (e.g. richer `Func:<sig>` parsing) is
/// automatically picked up by the type-refine seeding path.
/// Builtin scalars + None / NoneType keep their existing
/// behavior; containers + BigInt + user classes are newly
/// refined (previously returned None and stayed DynBox).
#[test]
fn parse_return_type_str_uses_centralized_helper() {
    // Existing builtin-scalar contracts (preserved).
    assert_eq!(parse_return_type_str("int"), Some(TirType::I64));
    assert_eq!(parse_return_type_str("float"), Some(TirType::F64));
    assert_eq!(parse_return_type_str("bool"), Some(TirType::Bool));
    assert_eq!(parse_return_type_str("str"), Some(TirType::Str));
    assert_eq!(parse_return_type_str("bytes"), Some(TirType::Bytes));
    assert_eq!(parse_return_type_str("None"), Some(TirType::None));
    assert_eq!(parse_return_type_str("NoneType"), Some(TirType::None));

    // Newly refined container/special types.
    assert_eq!(
        parse_return_type_str("list"),
        Some(TirType::List(Box::new(TirType::DynBox))),
        "method returning `list` must seed type-refine with \
         List(DynBox), not DynBox — otherwise lane inference \
         never sees the container type"
    );
    assert_eq!(
        parse_return_type_str("dict"),
        Some(TirType::Dict(
            Box::new(TirType::DynBox),
            Box::new(TirType::DynBox)
        ))
    );
    assert_eq!(
        parse_return_type_str("set"),
        Some(TirType::Set(Box::new(TirType::DynBox)))
    );
    assert_eq!(
        parse_return_type_str("tuple"),
        Some(TirType::Tuple(Vec::new()))
    );
    assert_eq!(parse_return_type_str("BigInt"), Some(TirType::BigInt));

    // User-class refinement: the live use of TirType::UserClass
    // through the type-refine seeding path.
    assert_eq!(
        parse_return_type_str("Point"),
        Some(TirType::UserClass("Point".into())),
        "method returning a user class must propagate UserClass \
         through type-refine — enables direct dispatch / \
         escape analysis precision on the result of factory \
         methods"
    );
    assert_eq!(
        parse_return_type_str("MyDataClass"),
        Some(TirType::UserClass("MyDataClass".into()))
    );

    // Structured compound containers refine through the same helper.
    assert_eq!(
        parse_return_type_str("list[int]"),
        Some(TirType::List(Box::new(TirType::I64)))
    );
    assert_eq!(
        parse_return_type_str("dict[str, list[float]]"),
        Some(TirType::Dict(
            Box::new(TirType::Str),
            Box::new(TirType::List(Box::new(TirType::F64)))
        ))
    );

    // Dynamic / malformed / unknown hints fall through to None so the
    // caller's operand-based inference takes over (rather than
    // forcing DynBox).
    assert_eq!(parse_return_type_str("Any"), None);
    assert_eq!(parse_return_type_str("Unknown"), None);
    assert_eq!(parse_return_type_str(""), None);
    assert_eq!(parse_return_type_str("Func:foo"), None);
    assert_eq!(parse_return_type_str("BoundMethod:list:append"), None);
    assert_eq!(parse_return_type_str("list[]"), None);
    assert_eq!(parse_return_type_str("list[Any]"), None);
}

#[test]
fn object_new_bound_type_hint_is_structural_class_result_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("_type_hint".into(), AttrValue::Str("Point".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::ObjectNewBound, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::UserClass("Point".into())),
        "object_new_bound _type_hint is the structural class-id contract, not legacy scalar transport",
    );
}

#[test]
fn legacy_type_hint_does_not_refine_call_return_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("_type_hint".into(), AttrValue::Str("int".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::DynBox),
        "legacy SimpleIR `_type_hint` must remain semantic transport metadata, not call-return proof",
    );
}

#[test]
fn structural_return_type_refines_call_return_type() {
    let result = ValueId(0);
    let mut attrs = AttrDict::new();
    attrs.insert("return_type".into(), AttrValue::Str("int".into()));
    attrs.insert("_type_hint".into(), AttrValue::Str("str".into()));
    let mut func = single_block_func(
        vec![make_op(OpCode::CallMethod, vec![], vec![result], attrs)],
        1,
    );
    func.blocks.get_mut(&BlockId(0)).unwrap().terminator = Terminator::Return {
        values: vec![result],
    };

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&result),
        Some(&TirType::I64),
        "explicit structural return_type remains the call-return refinement contract",
    );
}
