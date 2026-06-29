use super::*;

#[test]
fn builtin_len_return_refines_to_i64_without_transport_hint() {
    let list = ValueId(0);
    let result = ValueId(1);
    let mut attrs = AttrDict::new();
    attrs.insert("name".into(), AttrValue::Str("len".into()));
    let ops = vec![make_op(
        OpCode::CallBuiltin,
        vec![list],
        vec![result],
        attrs,
    )];
    let mut func = single_block_func(ops, 2);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::DynBox)));

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&result), Some(&TirType::I64));
}

#[test]
fn builtin_predicate_returns_refine_to_bool() {
    for name in ["bool", "hasattr", "isinstance", "issubclass"] {
        let value = ValueId(0);
        let result = ValueId(1);
        let mut attrs = AttrDict::new();
        attrs.insert("name".into(), AttrValue::Str(name.into()));
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

        assert_eq!(
            type_map.get(&result),
            Some(&TirType::Bool),
            "call_builtin {name} should refine to Bool"
        );
    }
}

#[test]
fn builtin_ord_and_chr_return_types_refine() {
    let value = ValueId(0);
    let ord_result = ValueId(1);
    let chr_result = ValueId(2);
    let mut ord_attrs = AttrDict::new();
    ord_attrs.insert("name".into(), AttrValue::Str("ord".into()));
    let mut chr_attrs = AttrDict::new();
    chr_attrs.insert("name".into(), AttrValue::Str("chr".into()));
    let ops = vec![
        make_op(
            OpCode::CallBuiltin,
            vec![value],
            vec![ord_result],
            ord_attrs,
        ),
        make_op(
            OpCode::CallBuiltin,
            vec![ord_result],
            vec![chr_result],
            chr_attrs,
        ),
    ];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(value, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&ord_result), Some(&TirType::I64));
    assert_eq!(type_map.get(&chr_result), Some(&TirType::Str));
}

#[test]
fn ord_at_return_type_refines_to_dynbox() {
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

    assert_eq!(type_map.get(&result), Some(&TirType::DynBox));
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
