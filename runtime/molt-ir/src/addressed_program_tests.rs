use super::*;
use crate::ir::{ExecutionContextPolicy, FunctionReturnAbi};

fn function(name: &str, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        return_abi: FunctionReturnAbi::Void,
        params: Vec::new(),
        ops,
        param_types: None,
        source_file: None,
        is_extern: false,
        execution_context: ExecutionContextPolicy::None,
        codegen_partition: false,
        parameter_custody: Vec::new(),
    }
}

fn op(kind: crate::OpKind) -> OpIR {
    OpIR {
        kind: kind,
        ..crate::OpIR::new(kind)
    }
}

fn document(functions: Vec<FunctionIR>) -> BackendIrDocument {
    BackendIrDocument {
        ir: SimpleIR {
            functions,
            profile: None,
        },
        module_registry: None,
        code_slots: CodeSlotBindings::default(),
    }
}

fn three_functions() -> BackendIrDocument {
    document(vec![
        function("molt_main", vec![op(crate::OpKind::RetVoid)]),
        function("helper_a", vec![op(crate::OpKind::RetVoid)]),
        function(
            "helper_b",
            vec![op(crate::OpKind::Nop), op(crate::OpKind::RetVoid)],
        ),
    ])
}

/// Canonical whole-function equality: the versioned contract bytes.
fn contracts(functions: &[FunctionIR]) -> Vec<Vec<u8>> {
    functions
        .iter()
        .map(|function| {
            let mut bytes = Vec::new();
            crate::ir::write_function_ir_contract(function, &mut bytes).unwrap();
            bytes
        })
        .collect()
}

fn decode(bytes: &[u8]) -> Result<AddressedProgram, String> {
    AddressedProgram::from_msgpack_reader(&mut &bytes[..])
}

fn names(document: &BackendIrDocument) -> Vec<&str> {
    document
        .ir
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect()
}

#[test]
fn digest_domain_matches_the_cli_producer() {
    // Independent oracle: hashlib.sha256(b"molt.function-body.msgpack.v1\0" +
    // body).hexdigest() in CPython, for the empty body and the empty map.
    // tests/cli/test_function_records.py pins the same two literals.
    assert_eq!(
        FunctionDigest::of_body(b"").to_string(),
        "46cfe5b141fcf8a66c1b639d1f1efba63937ab8eee486e4ea0e917a05d1d0703"
    );
    assert_eq!(
        FunctionDigest::of_body(b"\x80").to_string(),
        "1b33b063cec4aa779ba4e30be9dec72b994a45727639f5f1bb35caa322a19f96"
    );
}

#[test]
fn hex_digest_lists_round_trip_and_refuse_malformed_text() {
    let digests = [FunctionDigest::of_body(b"a"), FunctionDigest::of_body(b"b")];
    let text: String = digests.iter().map(ToString::to_string).collect();
    assert_eq!(
        FunctionDigest::parse_concatenated_hex(&text).unwrap(),
        digests
    );
    assert_eq!(FunctionDigest::parse_concatenated_hex("").unwrap(), []);
    assert!(FunctionDigest::parse_concatenated_hex(&text[1..]).is_err());
    assert!(FunctionDigest::parse_concatenated_hex(&text.to_uppercase()).is_err());
    assert!(FunctionDigest::parse_concatenated_hex(&"g".repeat(64)).is_err());
    assert!(FunctionDigest::split_concatenated(&[0; 33]).is_err());
}

#[test]
fn a_complete_lease_reproduces_the_program_in_order() {
    let source = three_functions();
    let lease = encode_complete_lease(&source).unwrap();
    let program = decode(&lease).unwrap();
    assert_eq!(
        program.digests(),
        lease_function_digests(&source).unwrap().as_slice()
    );
    let document = program.into_complete_document().unwrap();
    assert_eq!(names(&document), ["molt_main", "helper_a", "helper_b"]);
    assert_eq!(
        contracts(&document.ir.functions),
        contracts(&source.ir.functions)
    );
}

#[test]
fn a_partial_lease_names_exactly_the_positions_it_cannot_fill() {
    let source = three_functions();
    let lease = encode_lease(&source, |position| position == 1).unwrap();
    let program = decode(&lease).unwrap();
    let digests = program.digests().to_vec();
    assert!(!program.ships(&digests[0]) && program.ships(&digests[1]));
    assert_eq!(
        program.assemble(|_, _| None).unwrap_err(),
        ProgramAssemblyError::MissingFunctions(vec![0, 2])
    );
    assert_eq!(
        decode(&lease)
            .unwrap()
            .into_complete_document()
            .unwrap_err(),
        "IR lease lacks 2 function bodies (first at program position 0)"
    );
}

#[test]
fn resident_bodies_fill_the_rest_and_stay_shared() {
    let source = three_functions();
    let digests = lease_function_digests(&source).unwrap();
    let store: HashMap<FunctionDigest, Arc<FunctionIR>> = [0, 2]
        .into_iter()
        .map(|position| {
            (
                digests[position],
                Arc::new(source.ir.functions[position].clone()),
            )
        })
        .collect();
    let program = decode(&encode_lease(&source, |position| position == 1).unwrap()).unwrap();
    let mut asked = Vec::new();
    let document = program
        .assemble(|position, digest| {
            asked.push(position);
            store.get(digest).map(Arc::clone)
        })
        .unwrap();
    assert_eq!(asked, [0, 2], "only unshipped positions consult the store");
    assert_eq!(
        contracts(&document.ir.functions),
        contracts(&source.ir.functions)
    );
    for handle in store.values() {
        assert_eq!(Arc::strong_count(handle), 1, "the store keeps its own copy");
    }
}

#[test]
fn a_body_fills_only_the_slot_its_own_bytes_name() {
    let source = three_functions();
    let mut forged = three_functions();
    forged.ir.functions[1].ops.insert(0, op(crate::OpKind::Nop));
    // The forged lease lists the honest digests but ships a different body.
    let honest = encode_lease(&source, |_| false).unwrap();
    let forged_body = rmp_serde::to_vec_named(&forged.ir.functions[1]).unwrap();
    let digests: Vec<u8> = lease_function_digests(&source)
        .unwrap()
        .iter()
        .flat_map(|digest| *digest.as_bytes())
        .collect();
    let lease = rmp_serde::to_vec_named(&LeaseWireRef {
        code_slots: &CodeSlotBindings::default(),
        function_bodies: vec![WireBytesRef(&forged_body)],
        function_digests: WireBytesRef(&digests),
        module_registry: &None,
        profile: &None,
    })
    .unwrap();
    assert!(decode(&honest).is_ok());
    let error = decode(&lease).err().unwrap();
    assert!(
        error.contains("is not in the program's digest list"),
        "{error}"
    );
}

#[test]
fn repeated_digests_and_bodies_are_refused() {
    let source = document(vec![function("only", vec![op(crate::OpKind::RetVoid)])]);
    let body = rmp_serde::to_vec_named(&source.ir.functions[0]).unwrap();
    let digest = *FunctionDigest::of_body(&body).as_bytes();
    let encode = |digests: &[u8], bodies: Vec<WireBytesRef<'_>>| {
        rmp_serde::to_vec_named(&LeaseWireRef {
            code_slots: &CodeSlotBindings::default(),
            function_bodies: bodies,
            function_digests: WireBytesRef(digests),
            module_registry: &None,
            profile: &None,
        })
        .unwrap()
    };
    let twice_listed = encode(&[digest, digest].concat(), Vec::new());
    assert!(
        decode(&twice_listed)
            .err()
            .unwrap()
            .contains("appears twice")
    );
    let twice_shipped = encode(&digest, vec![WireBytesRef(&body), WireBytesRef(&body)]);
    assert!(
        decode(&twice_shipped)
            .err()
            .unwrap()
            .contains("shipped twice")
    );
}

#[test]
fn the_lease_map_is_closed_and_complete() {
    let lease = encode_complete_lease(&three_functions()).unwrap();
    let mut trailing = lease.clone();
    trailing.push(0xc0);
    assert_eq!(
        decode(&trailing).err().unwrap(),
        "trailing bytes after the IR value"
    );
    // A retired protocol 2 `functions` array is an unknown field, not a program.
    let retired = rmp_serde::to_vec_named(&serde_json::json!({
        "functions": [],
        "function_bodies": [],
        "function_digests": serde_json::Value::Array(Vec::new()),
    }))
    .unwrap();
    assert!(decode(&retired).is_err());
    let mut truncated = lease;
    truncated.pop();
    assert!(decode(&truncated).is_err());
}

#[test]
fn bodies_pass_their_own_contract_on_admission_and_programs_on_assembly() {
    // Each body alone is valid; only the program breaks a cross-function fact.
    let mut caller = function("caller", vec![op(crate::OpKind::RetVoid)]);
    caller.execution_context = ExecutionContextPolicy::Inherited;
    caller.ops.insert(
        0,
        OpIR {
            kind: crate::OpKind::CallInternal,
            s_value: Some("callee".to_string()),
            out: Some("v0".to_string()),
            args: Some(Vec::new()),
            passes_execution_context: true,
            ..crate::OpIR::new(crate::OpKind::CallInternal)
        },
    );
    let source = document(vec![
        caller,
        function("callee", vec![op(crate::OpKind::RetVoid)]),
    ]);
    let program = decode(&encode_complete_lease(&source).unwrap()).unwrap();
    match program.assemble(|_, _| None) {
        Err(ProgramAssemblyError::Invalid(reason)) => {
            assert!(
                reason.contains("targets non-inherited function `callee`"),
                "{reason}"
            )
        }
        other => panic!("expected a program contract error, got {:?}", other.err()),
    }
    // A body that breaks its own contract never reaches assembly.
    let void_with_value = document(vec![function(
        "bad",
        vec![OpIR {
            kind: crate::OpKind::Ret,
            args: Some(vec!["v0".to_string()]),
            ..crate::OpIR::new(crate::OpKind::Ret)
        }],
    )]);
    let error = decode(&encode_complete_lease(&void_with_value).unwrap())
        .err()
        .unwrap();
    assert!(error.contains("invalid SimpleIR contract"), "{error}");
}

#[test]
fn retained_bytes_count_each_owned_allocation() {
    let base = function("f", vec![op(crate::OpKind::Nop)]);
    let mut grown = base.clone();
    grown.ops[0].s_value = Some(String::with_capacity(1000));
    assert_eq!(grown.retained_bytes() - base.retained_bytes(), 1000);
    let mut more_ops = base.clone();
    more_ops.ops.reserve_exact(10);
    assert!(
        more_ops.retained_bytes() - base.retained_bytes()
            >= (more_ops.ops.capacity() - base.ops.capacity()) * std::mem::size_of::<OpIR>()
    );
    assert!(base.retained_bytes() > std::mem::size_of::<FunctionIR>());
}
