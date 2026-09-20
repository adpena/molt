use super::*;

pub(super) fn token_test_ir() -> FunctionIR {
    FunctionIR {
        name: "tokens".into(),
        params: vec!["borrowed".into()],
        ops: vec![
            OpIR {
                kind: "alloc".into(),
                out: Some("owner".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "copy".into(),
                args: Some(vec!["owner".into()]),
                out: Some("alias".into()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
    }
}

#[test]
fn cleanup_tokens_release_sibling_paths_without_cross_branch_dedup() {
    use cranelift_codegen::ir::InstructionData;
    for release_in_predecessor in [false, true] {
        let input = token_test_ir();
        let analysis = preanalyze_for_test(&input);
        let mut backend = SimpleBackend::new();
        let mut sig = Signature::new(CallConv::SystemV);
        sig.params
            .extend([AbiParam::new(types::I64), AbiParam::new(types::I64)]);
        let mut function = Function::with_name_signature(UserFuncName::user(0, 0), sig);
        let mut context = FunctionBuilderContext::new();
        let (left, right, merge, release);
        {
            let mut builder = FunctionBuilder::new(&mut function, &mut context);
            // Match production: ownership declarations precede entry-block
            // creation and must not emit instructions before initialization.
            let mut roots = NativeCleanupRoots::new(
                &mut builder,
                &input,
                &analysis.alias_roots,
                &ScalarRepresentationPlan::default(),
                NativeRcAuthority::NativeValueTracking,
            );
            let entry = builder.create_block();
            left = builder.create_block();
            right = builder.create_block();
            merge = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let params = builder.block_params(entry).to_vec();
            assert!(roots.contains("owner"));
            assert!(roots.shares_owner("owner", "alias"));
            assert!(
                !roots.contains("borrowed"),
                "never-acquired params need no cleanup phi"
            );
            roots.initialize(&mut builder);
            release = import_func_ref(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut BTreeMap::new(),
                "molt_dec_ref_obj",
                &[types::I64],
                &[],
            );
            roots.acquire(&mut builder, release, "owner", params[1]);
            if release_in_predecessor {
                roots.release(&mut builder, release, "alias");
            }
            builder.ins().brif(params[0], left, &[], right, &[]);
            for block in [left, right] {
                builder.switch_to_block(block);
                builder.seal_block(block);
                roots.release(&mut builder, release, "alias");
                roots.release(&mut builder, release, "owner");
                builder.ins().jump(merge, &[]);
            }
            builder.switch_to_block(merge);
            builder.seal_block(merge);
            roots.release_all(&mut builder, release);
            builder.ins().return_(&[]);
            builder.finalize();
        }
        let calls = |block| {
            function.layout.block_insts(block).filter(|&inst| matches!(
            function.dfg.insts[inst], InstructionData::Call { func_ref, .. } if func_ref == release
        )).count()
        };
        assert_eq!(
            calls(left),
            usize::from(!release_in_predecessor),
            "{}",
            function.display()
        );
        assert_eq!(
            calls(right),
            usize::from(!release_in_predecessor),
            "{}",
            function.display()
        );
        assert_eq!(calls(merge), 0, "both predecessor tokens are empty");
        verify_function(&function, &settings::Flags::new(settings::builder()))
            .unwrap_or_else(|errors| panic!("{errors}\n{}", function.display()));
    }
}

#[test]
fn cleanup_tokens_rearm_and_transfer_on_the_executing_path() {
    use cranelift_codegen::ir::InstructionData;
    let input = token_test_ir();
    let analysis = preanalyze_for_test(&input);
    let mut backend = SimpleBackend::new();
    let mut sig = Signature::new(CallConv::SystemV);
    sig.params
        .extend([AbiParam::new(types::I64), AbiParam::new(types::I64)]);
    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut context = FunctionBuilderContext::new();
    let release;
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let values = builder.block_params(entry).to_vec();
        let mut roots = NativeCleanupRoots::new(
            &mut builder,
            &input,
            &analysis.alias_roots,
            &ScalarRepresentationPlan::default(),
            NativeRcAuthority::NativeValueTracking,
        );
        roots.initialize(&mut builder);
        release = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut BTreeMap::new(),
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        roots.acquire(&mut builder, release, "owner", values[0]);
        roots.acquire(&mut builder, release, "owner", values[1]); // displaced owner
        roots.transfer(&mut builder, "alias");
        roots.release_all(&mut builder, release); // transferred: no release
        roots.acquire(&mut builder, release, "owner", values[0]);
        roots.release_all(&mut builder, release); // reacquired: one release
        builder.ins().return_(&[]);
        builder.finalize();
    }
    let calls = function.layout.blocks().flat_map(|block| function.layout.block_insts(block))
        .filter(|&inst| matches!(function.dfg.insts[inst], InstructionData::Call { func_ref, .. } if func_ref == release)).count();
    assert_eq!(calls, 2, "{}", function.display());
    verify_function(&function, &settings::Flags::new(settings::builder())).unwrap();
}

#[test]
fn tir_drop_insertion_declares_no_native_owner_tokens() {
    let input = token_test_ir();
    let analysis = preanalyze_for_test(&input);
    let mut function =
        Function::with_name_signature(UserFuncName::user(0, 0), Signature::new(CallConv::SystemV));
    let mut context = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut function, &mut context);
    let roots = NativeCleanupRoots::new(
        &mut builder,
        &input,
        &analysis.alias_roots,
        &ScalarRepresentationPlan::default(),
        NativeRcAuthority::TirDropInsertion,
    );
    assert!(!roots.contains("owner"));
    assert!(!roots.contains("alias"));
}

#[test]
fn cleanup_token_reassignment_is_carried_over_a_real_backedge() {
    let input = token_test_ir();
    let analysis = preanalyze_for_test(&input);
    let mut backend = SimpleBackend::new();
    let mut signature = Signature::new(CallConv::SystemV);
    signature
        .params
        .extend([AbiParam::new(types::I64), AbiParam::new(types::I64)]);
    let mut function = Function::with_name_signature(UserFuncName::user(0, 0), signature);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let header = builder.create_block();
        let exit = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let inputs = builder.block_params(entry).to_vec();
        let mut roots = NativeCleanupRoots::new(
            &mut builder,
            &input,
            &analysis.alias_roots,
            &ScalarRepresentationPlan::default(),
            NativeRcAuthority::NativeValueTracking,
        );
        roots.initialize(&mut builder);
        let release = import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut BTreeMap::new(),
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        builder.ins().jump(header, &[]);
        builder.switch_to_block(header);
        roots.acquire(&mut builder, release, "owner", inputs[1]);
        builder.ins().brif(inputs[0], header, &[], exit, &[]);
        builder.seal_block(header);
        builder.switch_to_block(exit);
        builder.seal_block(exit);
        roots.release_all(&mut builder, release);
        builder.ins().return_(&[]);
        builder.finalize();
    }
    verify_function(&function, &settings::Flags::new(settings::builder()))
        .unwrap_or_else(|errors| panic!("{errors}\n{}", function.display()));
}

#[test]
fn protected_cleanup_preserves_candidates_without_owning_release_state() {
    let mut carry = Vec::new();
    let cleanup = vec!["phi_in".into(), "dead".into()];
    let actual = protect_cleanup_names(&mut carry, cleanup, &BTreeSet::from(["phi_in"]));
    assert_eq!(carry, vec!["phi_in"]);
    assert_eq!(actual, vec!["dead"]);
}
