use super::*;

#[test]
fn preserved_iterator_spelling_requires_the_iterable_protocol() {
    for kind in [
        molt_ir::OpKind::Iter,
        molt_ir::OpKind::GetIter,
        molt_ir::OpKind::IterNext,
        molt_ir::OpKind::IterNextUnboxed,
    ] {
        let op = OpIR {
            kind,
            args: Some(vec!["source".into()]),
            out: Some("iterator".into()),
            var: (kind == molt_ir::OpKind::IterNextUnboxed).then(|| "item".into()),
            ..OpIR::new(kind)
        };
        assert!(
            op.runtime_requirements()
                .expect("iterator operations must be classified")
                .contains(SimpleIrRuntimeRequirements::ITERABLE_PROTOCOL)
        );
        let ir = function_ir(vec![op]);
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::wasm_release_fast(),
            TargetInfo::llvm_release_fast(),
            TargetInfo::luau_release_fast(),
        ] {
            validate_runtime_target_contract(&ir, &target)
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
        }
        let error = validate_runtime_target_contract(&ir, &TargetInfo::rust_release_fast())
            .expect_err("a target without the iterable protocol must still reject iterators");
        assert!(
            error.contains("Python iterable/sequence protocol"),
            "{error}"
        );
    }
}

#[test]
fn runtime_aliases_and_typed_siblings_cannot_escape_existing_capabilities() {
    for kind in [
        molt_ir::OpKind::Load,
        molt_ir::OpKind::GuardedLoad,
        molt_ir::OpKind::Store,
        molt_ir::OpKind::StringEq,
        molt_ir::OpKind::IndexSet,
        molt_ir::OpKind::DelAttr,
        molt_ir::OpKind::BuildTuple,
        molt_ir::OpKind::BuildSet,
        molt_ir::OpKind::BuildSlice,
        molt_ir::OpKind::ObjectNewBound,
        molt_ir::OpKind::CallBuiltin,
        molt_ir::OpKind::LoadMethod,
        molt_ir::OpKind::LoadSuperMethod,
        molt_ir::OpKind::CallLoadedMethod,
        molt_ir::OpKind::Yield,
        molt_ir::OpKind::YieldFrom,
        molt_ir::OpKind::StateSet,
        molt_ir::OpKind::StateBlockStart,
        molt_ir::OpKind::StateBlockEnd,
        molt_ir::OpKind::IsPending,
        molt_ir::OpKind::TaskWait,
        molt_ir::OpKind::ExceptionPending,
        molt_ir::OpKind::FunctionDefaultsVersion,
        molt_ir::OpKind::GetIter,
        molt_ir::OpKind::ForIterStart,
        molt_ir::OpKind::ForIterEnd,
        molt_ir::OpKind::AsyncForStart,
        molt_ir::OpKind::AsyncForEnd,
        molt_ir::OpKind::Free,
        molt_ir::OpKind::DelBoundary,
        molt_ir::OpKind::DeleteVar,
        molt_ir::OpKind::GpuThreadId,
        molt_ir::OpKind::GpuBarrier,
    ] {
        let ir = function_ir(vec![OpIR {
            kind,
            ..OpIR::new(kind)
        }]);
        let error =
            validate_runtime_target_contract(&ir, &TargetInfo::rust_release_fast()).unwrap_err();
        assert!(
            error.contains("before source generation"),
            "{kind}: {error}"
        );
        for target in [
            TargetInfo::native_release_fast(),
            TargetInfo::wasm_release_fast(),
            TargetInfo::llvm_release_fast(),
        ] {
            validate_runtime_target_contract(&ir, &target)
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
        }
    }
}

#[test]
fn canonical_integer_literal_aliases_share_exact_admission() {
    for kind in [
        molt_ir::OpKind::Const,
        molt_ir::OpKind::ConstInt,
        molt_ir::OpKind::LoadConst,
    ] {
        let op = OpIR {
            kind,
            value: Some(-7),
            out: Some("value".into()),
            ..OpIR::new(kind)
        };
        assert_eq!(exact_integer_literal_value(&op, 7), Some(-7));
        assert_eq!(exact_integer_literal_value(&op, 6), None);
        let ir = function_ir(vec![op]);
        assert!(
            validate_numeric_target_contract(&ir, &TargetInfo::rust_release_fast()).is_err(),
            "{kind}"
        );
        validate_numeric_target_contract(&ir, &TargetInfo::native_release_fast())
            .unwrap_or_else(|error| panic!("{kind}: {error}"));
    }
}
