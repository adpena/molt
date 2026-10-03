use super::*;

#[test]
fn public_builtin_names_never_refine_replacement_results() {
    for name in [
        "len",
        "id",
        "ord",
        "chr",
        "hasattr",
        "isinstance",
        "issubclass",
    ] {
        for dynamic in [false, true] {
            let name_value = ValueId(0);
            let argument = ValueId(1);
            let result = ValueId(2);
            let mut attrs = AttrDict::from([
                ("return_type".into(), AttrValue::Str("int".into())),
                ("_type_hint".into(), AttrValue::Str("str".into())),
            ]);
            let operands = if dynamic {
                vec![name_value, argument]
            } else {
                attrs.insert("name".into(), AttrValue::Str(name.into()));
                vec![argument]
            };
            let mut function = single_block_func(
                vec![
                    make_op(OpCode::ConstStr, vec![], vec![name_value], str_attr(name)),
                    make_op(OpCode::ConstInt, vec![], vec![argument], int_attr(1)),
                    make_op(OpCode::CallBuiltin, operands, vec![result], attrs),
                ],
                3,
            );
            refine_types(&mut function);
            assert_eq!(
                extract_type_map(&function).get(&result),
                Some(&TirType::DynBox),
                "{name}, dynamic={dynamic}"
            );
            assert!(!extract_exact_scalar_map(&function).contains_key(&result));
        }
    }
}

#[test]
fn ord_at_return_type_refines_to_i64() {
    let text = ValueId(0);
    let index = ValueId(1);
    let result = ValueId(2);
    let ops = vec![make_op(
        OpCode::OrdAt,
        vec![text, index],
        vec![result],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(text, TirType::Str);
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::I64));
}

#[test]
fn unknown_builtin_return_stays_dynbox() {
    let value = ValueId(0);
    let result = ValueId(1);
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("dynamic_builtin".into()));
    let ops = vec![make_op(
        OpCode::CallBuiltin,
        vec![value],
        vec![result],
        attrs,
    )];
    let mut func = single_block_func(ops, 2);
    func.value_types.insert(value, TirType::DynBox);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::DynBox));
}

#[test]
fn mutable_builtin_names_and_conflicting_dispatch_do_not_prove_results() {
    for name in ["bool", "int", "float", "str", "range", "math.floor"] {
        let mut attrs = AttrDict::from([("name".into(), AttrValue::Str(name.into()))]);
        attrs.insert("return_type".into(), AttrValue::Str("bool".into()));
        let mut func = single_block_func(
            vec![make_op(
                OpCode::CallBuiltin,
                vec![ValueId(0)],
                vec![ValueId(1)],
                attrs,
            )],
            2,
        );
        func.value_types.insert(ValueId(0), TirType::DynBox);
        refine_types(&mut func);
        assert_eq!(
            extract_type_map(&func).get(&ValueId(1)),
            Some(&TirType::DynBox),
            "{name}"
        );
    }
    let attrs = AttrDict::from([
        ("name".into(), AttrValue::Str("len".into())),
        ("_original_kind".into(), AttrValue::Str("print".into())),
    ]);
    let mut func = single_block_func(
        vec![make_op(
            OpCode::CallBuiltin,
            vec![ValueId(0)],
            vec![ValueId(1)],
            attrs,
        )],
        2,
    );
    func.value_types.insert(ValueId(0), TirType::DynBox);
    refine_types(&mut func);
    assert_eq!(
        extract_type_map(&func).get(&ValueId(1)),
        Some(&TirType::DynBox)
    );
}

#[test]
fn iter_next_unboxed_done_flag_refines_to_bool() {
    let iter = ValueId(0);
    let elem = ValueId(1);
    let done = ValueId(2);
    let ops = vec![make_op(
        OpCode::IterNextUnboxed,
        vec![iter],
        vec![elem, done],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(iter, TirType::DynBox);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&elem),
        Some(&TirType::DynBox),
        "iterator element stays conservative until iterator element provenance is represented"
    );
    assert_eq!(type_map.get(&done), Some(&TirType::Bool));
    assert_eq!(
        func.value_types.get(&done),
        Some(&TirType::Bool),
        "refine_types must persist multi-result done-flag facts"
    );
}

#[test]
fn get_iter_refines_known_iterable_element_types() {
    let cases = [
        (
            TirType::List(Box::new(TirType::I64)),
            TirType::Iterator(Box::new(TirType::I64)),
        ),
        (
            TirType::Set(Box::new(TirType::Str)),
            TirType::Iterator(Box::new(TirType::Str)),
        ),
        (
            TirType::Tuple(vec![TirType::I64, TirType::Str]),
            TirType::Iterator(Box::new(TirType::Union(vec![TirType::I64, TirType::Str]))),
        ),
        (
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
            TirType::Iterator(Box::new(TirType::Str)),
        ),
        (TirType::Str, TirType::Iterator(Box::new(TirType::Str))),
        (TirType::Bytes, TirType::Iterator(Box::new(TirType::I64))),
    ];

    for (iterable_ty, expected_iter_ty) in cases {
        let iterable = ValueId(0);
        let iter = ValueId(1);
        let ops = vec![make_op(
            OpCode::GetIter,
            vec![iterable],
            vec![iter],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 2);
        func.value_types.insert(iterable, iterable_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&iter),
            Some(&expected_iter_ty),
            "GetIter({iterable_ty:?}) should refine to {expected_iter_ty:?}"
        );
    }
}

#[test]
fn iterator_consumers_refine_element_types() {
    let iter = ValueId(0);
    let iter_next_elem = ValueId(1);
    let unboxed_elem = ValueId(2);
    let done = ValueId(3);
    let for_iter_elem = ValueId(4);
    let ops = vec![
        make_op(
            OpCode::IterNext,
            vec![iter],
            vec![iter_next_elem],
            AttrDict::new(),
        ),
        make_op(
            OpCode::IterNextUnboxed,
            vec![iter],
            vec![unboxed_elem, done],
            AttrDict::new(),
        ),
        make_op(
            OpCode::ForIter,
            vec![iter],
            vec![for_iter_elem],
            AttrDict::new(),
        ),
    ];
    let mut func = single_block_func(ops, 5);
    func.value_types
        .insert(iter, TirType::Iterator(Box::new(TirType::I64)));

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&iter_next_elem), Some(&TirType::I64));
    assert_eq!(type_map.get(&unboxed_elem), Some(&TirType::I64));
    assert_eq!(type_map.get(&done), Some(&TirType::Bool));
    assert_eq!(type_map.get(&for_iter_elem), Some(&TirType::I64));
}
