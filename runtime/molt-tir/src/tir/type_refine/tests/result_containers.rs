use super::*;

#[test]
fn list_index_refines_to_element_type() {
    let list = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![list, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::Bool)));
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::Bool));
    assert_eq!(
        func.value_types.get(&item),
        Some(&TirType::Bool),
        "refine_types must persist list element facts for backend plans"
    );
}

#[test]
fn list_index_with_non_integer_index_stays_dynbox() {
    let list = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![list, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(list, TirType::List(Box::new(TirType::Bool)));
    func.value_types.insert(index, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
}

#[test]
fn str_index_refines_to_str_for_integer_indices() {
    for index_ty in [TirType::I64, TirType::Bool] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, TirType::Str);
        func.value_types.insert(index, index_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::Str),
            "str indexed by {index_ty:?} should refine to Str"
        );
    }
}

#[test]
fn bytes_index_refines_to_i64_for_integer_indices() {
    for index_ty in [TirType::I64, TirType::Bool] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, TirType::Bytes);
        func.value_types.insert(index, index_ty.clone());

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::I64),
            "bytes indexed by {index_ty:?} should refine to I64"
        );
    }
}

#[test]
fn immutable_sequence_index_with_non_integer_index_stays_dynbox() {
    for value_ty in [TirType::Str, TirType::Bytes] {
        let value = ValueId(0);
        let index = ValueId(1);
        let item = ValueId(2);
        let ops = vec![make_op(
            OpCode::Index,
            vec![value, index],
            vec![item],
            AttrDict::new(),
        )];
        let mut func = single_block_func(ops, 3);
        func.value_types.insert(value, value_ty.clone());
        func.value_types.insert(index, TirType::Str);

        refine_types(&mut func);
        let type_map = extract_type_map(&func);

        assert_eq!(
            type_map.get(&item),
            Some(&TirType::DynBox),
            "{value_ty:?} indexed by Str must stay conservative"
        );
    }
}

#[test]
fn tuple_index_refines_homogeneous_element_type() {
    let tuple = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![tuple, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types
        .insert(tuple, TirType::Tuple(vec![TirType::Str, TirType::Str]));
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::Str));
}

#[test]
fn tuple_index_refines_to_element_join_for_mixed_tuple() {
    let tuple = ValueId(0);
    let index = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![tuple, index],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        tuple,
        TirType::Tuple(vec![TirType::I64, TirType::Str, TirType::I64]),
    );
    func.value_types.insert(index, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(
        type_map.get(&item),
        Some(&TirType::Union(vec![TirType::I64, TirType::Str]))
    );
}

#[test]
fn dict_index_refines_matching_key_to_value_type() {
    let dict = ValueId(0);
    let key = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![dict, key],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        dict,
        TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
    );
    func.value_types.insert(key, TirType::Str);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::I64));
}

#[test]
fn dict_index_with_nonmatching_key_stays_dynbox() {
    let dict = ValueId(0);
    let key = ValueId(1);
    let item = ValueId(2);
    let ops = vec![make_op(
        OpCode::Index,
        vec![dict, key],
        vec![item],
        AttrDict::new(),
    )];
    let mut func = single_block_func(ops, 3);
    func.value_types.insert(
        dict,
        TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64)),
    );
    func.value_types.insert(key, TirType::I64);

    refine_types(&mut func);
    let type_map = extract_type_map(&func);

    assert_eq!(type_map.get(&item), Some(&TirType::DynBox));
}
