use super::*;

fn builtin_func_ir(runtime_name: &str, arity: i64) -> SimpleIR {
    let mut builtin = wasm_test_op(molt_ir::OpKind::BuiltinFunc, Some("fn"), vec![]);
    builtin.s_value = Some(runtime_name.to_string());
    builtin.value = Some(arity);
    SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec![],
            None,
            vec![
                builtin,
                wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![]),
            ],
        )],
        profile: None,
    }
}

macro_rules! reserved_spec {
    ($runtime_name:expr) => {
        RESERVED_RUNTIME_CALLABLE_SPECS
            .iter()
            .find(|spec| spec.runtime_name == $runtime_name)
            .unwrap_or_else(|| panic!("{} must stay a reserved runtime callable", $runtime_name))
    };
}

fn compile(ir: SimpleIR, reloc_enabled: bool) -> Vec<u8> {
    let wasm = WasmBackend::with_options(WasmCompileOptions {
        native_eh_enabled: false,
        reloc_enabled,
        ..WasmCompileOptions::default()
    })
    .compile(ir);
    wasmparser::Validator::new()
        .validate_all(&wasm)
        .expect("keyed runtime callable module must be structurally valid WASM");
    wasm
}

/// HF-699 K1, K5: a `builtin_func` of a reserved `UnpackArgs` callable stores
/// the callable's key as an `i64.const`, and its trampoline is an app
/// function that unpacks argv and calls the runtime import. No table slot
/// names the runtime callable itself.
#[test]
fn builtin_func_of_a_reserved_unpack_args_callable_stores_its_key() {
    let spec = reserved_spec!("molt_importlib_import_transaction");
    assert_eq!(
        spec.trampoline_abi,
        molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi::UnpackArgs
    );
    let wasm = compile(builtin_func_ir(spec.runtime_name, 5), false);

    assert!(
        wasm_i64_consts(&wasm).contains(&(spec.key() as i64)),
        "the function object must carry key 0x{:x}",
        spec.key()
    );
    assert_eq!(
        spec.key(),
        RUNTIME_CALLABLE_KEY_BASE + u64::from(spec.index)
    );

    assert_eq!(
        argv_trampolines_calling(&wasm, spec.import.name()),
        1,
        "one app trampoline must unpack argv and call the import directly"
    );
}

/// Table entries that take `(context, closure, argv, argc)` and call
/// `import_name` directly: the app trampolines of that runtime callable. The
/// leading attached execution context is not a Python argument (HF-198).
fn argv_trampolines_calling(wasm: &[u8], import_name: &str) -> usize {
    let imports = wasm_function_import_indices(wasm);
    let Some(&import_idx) = imports.get(import_name) else {
        return 0;
    };
    let import_count = imports.len() as u32;
    let function_type_indices = wasm_function_section_type_indices(wasm);
    let signatures = wasm_type_section_signatures(wasm);
    wasm_active_function_elements(wasm)
        .values()
        .copied()
        .filter(|&func_idx| func_idx >= import_count)
        .filter(|&func_idx| {
            let type_idx = function_type_indices[(func_idx - import_count) as usize];
            signatures[type_idx as usize] == (4, 1)
                && wasm_direct_call_indices_for_body(wasm, Some(func_idx - import_count))
                    .contains(&import_idx)
        })
        .count()
}

/// A `CallFrame` callable consumes `(closure, argv, argc)` itself, as native
/// uses the runtime entry as its own trampoline: its key is both identities,
/// and the app emits no trampoline for it.
#[test]
fn builtin_func_of_a_reserved_call_frame_callable_is_its_key_twice() {
    let spec = reserved_spec!("molt_dataclasses_init_trampoline");
    assert_eq!(
        spec.trampoline_abi,
        molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi::CallFrame
    );
    let wasm = compile(builtin_func_ir(spec.runtime_name, 3), false);

    let key = spec.key() as i64;
    assert_eq!(
        wasm_i64_consts(&wasm)
            .iter()
            .filter(|&&value| value == key)
            .count(),
        2,
        "function and trampoline identity must both be the key"
    );
    assert_eq!(
        argv_trampolines_calling(&wasm, spec.import.name()),
        0,
        "the runtime entry is its own trampoline"
    );
}

/// HF-699 K3: the app's table reserves no runtime region. An empty program's
/// table holds no runtime poll, no reserved runtime callable and no sentinel,
/// and a relocatable object exports an entry only for its own slots.
#[test]
fn app_callable_tables_reserve_no_runtime_region() {
    let empty = || SimpleIR {
        functions: vec![wasm_test_function(
            "molt_main",
            vec![],
            None,
            vec![wasm_test_op(molt_ir::OpKind::RetVoid, None, vec![])],
        )],
        profile: None,
    };
    let wasm = compile(empty(), false);
    let imports = wasm_function_import_indices(&wasm);
    let runtime_owned: BTreeSet<u32> = POLL_TABLE_IMPORTS
        .iter()
        .map(|spec| spec.import.name())
        .chain(
            RESERVED_RUNTIME_CALLABLE_SPECS
                .iter()
                .map(|spec| spec.import.name()),
        )
        .filter_map(|name| imports.get(name).copied())
        .collect();
    let first_call_indirect = CALL_INDIRECT_IMPORTS
        .first()
        .expect("generated call_indirect import family must be non-empty");
    let sentinel_func_idx = wasm_function_export_indices(&wasm)[first_call_indirect.import_name]
        + CALL_INDIRECT_IMPORTS.len() as u32;
    for (&slot, &func_idx) in &wasm_active_function_elements(&wasm) {
        assert!(
            !runtime_owned.contains(&func_idx),
            "slot {slot} holds runtime-owned function {func_idx}"
        );
        assert_ne!(
            func_idx, sentinel_func_idx,
            "slot {slot} holds the sentinel"
        );
    }

    let reloc = compile(empty(), true);
    let layout_section =
        crate::wasm_abi_generated::callable_table::CALLABLE_TABLE_LAYOUT_SECTION_NAME;
    let entry_exports = wasm_function_exports(&reloc)
        .into_iter()
        .filter(|name| name.starts_with(&format!("{layout_section}.entry.")))
        .count();
    let element_count = wasm_element_function_indices(&wasm).len();
    assert_eq!(
        entry_exports, element_count,
        "a relocatable object exports exactly its own callable slots"
    );
}
