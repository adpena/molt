use super::*;
use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
use crate::tir::passes::alias_analysis::build_alias_union_find;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

fn del_op(result: ValueId) -> TirOp {
    let mut o = op(OpCode::ObjectNewBound, vec![], vec![result]);
    o.attrs.insert("defines_del".into(), AttrValue::Bool(true));
    o
}

fn del_call_bind(result: ValueId) -> TirOp {
    let mut o = op(OpCode::Call, vec![], vec![result]);
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    o.attrs.insert("defines_del".into(), AttrValue::Bool(true));
    o
}

fn original_kind_copy(kind: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    let mut o = op(OpCode::Copy, operands, results);
    o.attrs
        .insert("_original_kind".into(), AttrValue::Str(kind.into()));
    o
}

fn func() -> TirFunction {
    TirFunction::new(
        "f".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    )
}

fn lattice(func: &TirFunction) -> OwnershipLattice {
    let aliases = build_alias_union_find(func);
    OwnershipLattice::compute(func, &aliases)
}

#[test]
fn direct_finalizer_object_is_sensitive() {
    let mut f = func();
    let a = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_op(a));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.is_finalizer_sensitive_root(a));
}

#[test]
fn iter_next_unboxed_value_result_root_is_conditionally_valid_without_finalizers() {
    let mut f = func();
    let iter = f.fresh_value();
    let value = f.fresh_value();
    let value_alias = f.fresh_value();
    let done = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry
        .ops
        .push(op(OpCode::IterNextUnboxed, vec![iter], vec![value, done]));
    let mut alias = op(OpCode::Copy, vec![value], vec![value_alias]);
    alias
        .attrs
        .insert("_original_kind".into(), AttrValue::Str("copy".into()));
    entry.ops.push(alias);
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    assert_eq!(
        aliases.root(value_alias),
        aliases.root(value),
        "the test fixture must prove conditional validity is stored per root"
    );
    let lat = OwnershipLattice::compute(&f, &aliases);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    assert!(
        lat.finalizer_sensitive_roots().is_empty(),
        "the conditional-validity fact must not depend on finalizer seeds"
    );
    assert!(
        root_facts.is_conditionally_valid_result_root(aliases.root(value)),
        "IterNextUnboxed result 0 root is valid only on the not-done edge"
    );
    assert!(
        root_facts.is_conditionally_valid_result_root(aliases.root(value_alias)),
        "transparent aliases of the value result share the conditional-validity root"
    );
    assert!(
        !root_facts.is_conditionally_valid_result_root(aliases.root(done)),
        "IterNextUnboxed result 1 is the done flag and is always valid"
    );
    assert_eq!(root_facts.conditionally_valid_result_roots().len(), 1);
}

fn iter_next_region(
    branch_on_done: bool,
    body_has_exception_entry: bool,
) -> (Option<BlockId>, BlockId) {
    let mut f = func();
    let iter = f.fresh_value();
    let value = f.fresh_value();
    let done = f.fresh_value();
    let flag = f.fresh_value();
    let exit = f.fresh_block();
    let body = f.fresh_block();
    let entry = f.entry_block;
    {
        let head = f.blocks.get_mut(&entry).unwrap();
        head.ops
            .push(op(OpCode::IterNextUnboxed, vec![iter], vec![value, done]));
        head.ops.push(op(OpCode::ConstBool, vec![], vec![flag]));
        head.terminator = Terminator::CondBranch {
            cond: if branch_on_done { done } else { flag },
            then_block: exit,
            then_args: vec![],
            else_block: body,
            else_args: vec![],
        };
    }
    let mut exit_ops = Vec::new();
    if body_has_exception_entry {
        f.label_id_map.insert(body.0, 5);
        let mut check = op(OpCode::CheckException, vec![], vec![]);
        check.attrs.insert("value".into(), AttrValue::Int(5));
        exit_ops.push(check);
    }
    for (id, ops) in [(exit, exit_ops), (body, Vec::new())] {
        f.blocks.insert(
            id,
            crate::tir::blocks::TirBlock {
                id,
                args: vec![],
                ops,
                terminator: Terminator::Return { values: vec![] },
            },
        );
    }
    let aliases = build_alias_union_find(&f);
    let facts = OwnershipRootFacts::compute(&f, &aliases);
    (facts.conditionally_valid_region(aliases.root(value)), body)
}

#[test]
fn iter_next_unboxed_value_region_is_its_sole_not_done_successor() {
    let (region, body) = iter_next_region(true, false);
    assert_eq!(region, Some(body), "the not-done arm initializes the value");
    let (region, _) = iter_next_region(true, true);
    assert_eq!(
        region, None,
        "a second entry into the not-done target can bypass initialization"
    );
    let (region, _) = iter_next_region(false, false);
    assert_eq!(
        region, None,
        "a branch on another flag says nothing about initialization"
    );
}

#[test]
fn exception_creation_ref_values_select_only_exception_creation_copies() {
    let mut f = func();
    let source = f.fresh_value();
    let creation = f.fresh_value();
    let plain_copy = f.fresh_value();
    let direct_exception = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(original_kind_copy(
        "exception_new_builtin_one",
        vec![source],
        vec![creation],
    ));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![], vec![plain_copy]));
    entry
        .ops
        .push(op(OpCode::Call, vec![], vec![direct_exception]));
    entry.terminator = Terminator::Return { values: vec![] };

    let facts = exception_creation_ref_values(&f);
    assert!(facts.contains(&creation));
    assert!(!facts.contains(&plain_copy));
    assert!(!facts.contains(&direct_exception));
}

#[test]
fn copy_transparent_alias_selects_only_single_operand_copy_aliases() {
    let mut f = func();
    let source = f.fresh_value();
    let alias_result = f.fresh_value();
    let extra = f.fresh_value();
    let fresh_result = f.fresh_value();

    let alias = original_kind_copy("copy_var", vec![source], vec![alias_result]);
    assert_eq!(
        copy_transparent_alias(&alias),
        Some(NoHeapCopyAlias {
            source,
            result: alias_result,
        })
    );

    let non_no_heap = original_kind_copy("list_new", vec![source], vec![fresh_result]);
    assert_eq!(copy_transparent_alias(&non_no_heap), None);

    let too_many_operands = original_kind_copy("copy_var", vec![source, extra], vec![alias_result]);
    assert_eq!(copy_transparent_alias(&too_many_operands), None);

    let too_many_results = original_kind_copy("copy_var", vec![source], vec![alias_result, extra]);
    assert_eq!(copy_transparent_alias(&too_many_results), None);

    let non_copy = op(OpCode::Call, vec![source], vec![alias_result]);
    assert_eq!(copy_transparent_alias(&non_copy), None);
}

#[test]
fn non_owning_copy_result_roots_are_lattice_facts() {
    let mut f = func();
    let source = f.fresh_value();
    let explicit_alias = f.fresh_value();
    let unknown_passthrough = f.fresh_value();
    let bare_passthrough = f.fresh_value();
    let fresh = f.fresh_value();
    let owned_alias = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![], vec![source]));
    entry.ops.push(original_kind_copy(
        "copy",
        vec![source],
        vec![explicit_alias],
    ));
    entry.ops.push(original_kind_copy(
        "not_registered_yet",
        vec![source],
        vec![unknown_passthrough],
    ));
    entry
        .ops
        .push(op(OpCode::Copy, vec![source], vec![bare_passthrough]));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![source], vec![fresh]));
    entry.ops.push(original_kind_copy(
        "binding_alias",
        vec![source],
        vec![owned_alias],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    assert_eq!(
        aliases.root(explicit_alias),
        aliases.root(source),
        "explicit no-heap moves must already share the source root"
    );
    assert_eq!(
        aliases.root(unknown_passthrough),
        unknown_passthrough,
        "unknown passthroughs remain independent roots and need the lattice fact"
    );
    assert_eq!(
        aliases.root(bare_passthrough),
        aliases.root(source),
        "bare Copy passthroughs are already folded aliases"
    );
    assert_eq!(
        aliases.root(owned_alias),
        owned_alias,
        "owned aliases keep a distinct ownership root"
    );
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    assert!(
        root_facts.is_non_owning_copy_result_root(unknown_passthrough),
        "unknown Copy kinds fail closed as non-owning result roots"
    );
    assert!(
        !root_facts.is_non_owning_copy_result_root(aliases.root(bare_passthrough)),
        "folded bare Copy aliases must not mark their source root non-droppable"
    );
    assert!(
        !root_facts.is_non_owning_copy_result_root(aliases.root(explicit_alias)),
        "explicit aliases are handled by alias-root folding, not this root set"
    );
    assert!(
        !root_facts.is_non_owning_copy_result_root(fresh),
        "fresh-owned Copy results keep their independent drop obligation"
    );
    assert!(
        !root_facts.is_non_owning_copy_result_root(owned_alias),
        "owned alias Copy results keep their independent drop obligation"
    );
}

#[test]
fn parameter_and_stack_roots_are_lattice_drop_eligibility_facts() {
    let mut f = TirFunction::new(
        "param_stack".into(),
        vec![TirType::Str],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let param = f.blocks[&f.entry_block].args[0].id;
    let stack = f.fresh_value();
    let heap = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![], vec![heap]));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    assert!(
        root_facts.is_borrowed_parameter_root(param),
        "entry block args are borrowed from the caller"
    );
    assert!(
        !root_facts.is_drop_owned_root_candidate(param),
        "borrowed parameter roots are not function-owned drop candidates"
    );
    assert!(
        root_facts.is_drop_owned_root_candidate(stack),
        "unsupported boxed stack roots do not mint no-release facts"
    );
    assert!(
        root_facts.is_drop_owned_root_candidate(heap),
        "ordinary heap roots remain function-owned drop candidates"
    );
}

#[test]
fn drop_eligibility_combines_root_facts_and_raw_scalar_filter() {
    let mut f = TirFunction::new(
        "drop_eligibility".into(),
        vec![TirType::Str],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let param = f.blocks[&f.entry_block].args[0].id;
    let stack = f.fresh_value();
    let heap = f.fresh_value();
    let heap_alias = f.fresh_value();
    let raw = f.fresh_value();
    let unknown_passthrough = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![], vec![heap]));
    entry
        .ops
        .push(original_kind_copy("copy", vec![heap], vec![heap_alias]));
    entry.ops.push(op(OpCode::ConstInt, vec![], vec![raw]));
    entry.ops.push(original_kind_copy(
        "not_registered_yet",
        vec![heap],
        vec![unknown_passthrough],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    let raw_scalars = std::collections::HashSet::from([raw]);
    let eligibility = DropEligibility::new(&aliases, &root_facts, &raw_scalars);
    assert!(
        eligibility.is_droppable(heap),
        "ordinary heap roots are droppable"
    );
    assert!(
        !eligibility.is_droppable(param),
        "borrowed parameter roots are not droppable"
    );
    assert!(
        eligibility.is_droppable(stack),
        "boxed allocation roots retain RC obligations"
    );
    assert!(
        !eligibility.is_droppable(raw),
        "raw scalar roots carry no heap release obligation"
    );
    assert!(
        !eligibility.is_droppable(heap_alias),
        "transparent aliases are not independently droppable"
    );
    assert!(
        !eligibility.is_droppable(unknown_passthrough),
        "self-rooting unknown Copy results fail closed as non-droppable"
    );
}

#[test]
fn python_lifetime_facts_track_bound_slots_and_explicit_releases() {
    let mut f = func();
    let bound = f.fresh_value();
    let stored = f.fresh_value();
    let loaded = f.fresh_value();
    let explicit = f.fresh_value();
    let missing = f.fresh_value();
    let deleted = f.fresh_value();
    let boundary = f.fresh_value();
    let boundary_stored = f.fresh_value();
    let stack = f.fresh_value();
    let stack_stored = f.fresh_value();
    let statement = f.fresh_value();
    let deferred = f.fresh_value();
    let iterator = f.fresh_value();
    let conditional_value = f.fresh_value();
    let conditional_done = f.fresh_value();
    let conditional_stored = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    let mut bound_op = op(OpCode::ObjectNewBound, vec![], vec![bound]);
    bound_op
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    entry.ops.push(bound_op);
    entry
        .ops
        .push(original_kind_copy("store_var", vec![bound], vec![stored]));
    entry
        .ops
        .push(original_kind_copy("load_var", vec![stored], vec![loaded]));
    entry.ops.push(op(OpCode::DecRef, vec![explicit], vec![]));
    entry
        .ops
        .push(op(OpCode::DeleteVar, vec![missing, loaded], vec![deleted]));
    entry.ops.push(del_op(boundary));
    entry.ops.push(original_kind_copy(
        "store_var",
        vec![boundary],
        vec![boundary_stored],
    ));
    entry.ops.push(op(OpCode::StackAlloc, vec![], vec![stack]));
    entry.ops.push(original_kind_copy(
        "store_var",
        vec![stack],
        vec![stack_stored],
    ));
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![], vec![statement]));
    let mut deferred_op = op(OpCode::ObjectNewBound, vec![], vec![deferred]);
    deferred_op
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    entry.ops.push(deferred_op);
    let mut iter_next = op(
        OpCode::IterNextUnboxed,
        vec![iterator],
        vec![conditional_value, conditional_done],
    );
    iter_next
        .attrs
        .insert("bound_local".into(), AttrValue::Bool(true));
    entry.ops.push(iter_next);
    entry.ops.push(original_kind_copy(
        "store_var",
        vec![conditional_value],
        vec![conditional_stored],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    let facts = PythonLifetimeFacts::compute(&f, &aliases);
    let bound_root = aliases.root(bound);
    let loaded_root = aliases.root(loaded);
    let boundary_root = aliases.root(boundary);
    let stack_root = aliases.root(stack);
    let statement_root = aliases.root(statement);
    let deferred_root = aliases.root(deferred);
    let conditional_root = aliases.root(conditional_value);
    assert!(
        facts.has_explicit_release_boundary(aliases.root(explicit))
            && facts.has_explicit_release_boundary(loaded_root),
        "DecRef and DeleteVar old-slot operands are explicit release roots"
    );
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    let liveness = crate::tir::analysis::AnalysisManager::new()
        .get::<crate::tir::passes::liveness::TirLiveness>(&f)
        .clone();
    let drop_eligibility = DropEligibility::new(&aliases, &root_facts, &liveness.raw_scalars);
    assert!(
        facts.is_statement_release_boundary_root(statement_root, &drop_eligibility),
        "droppable non-slot roots can release at statement finalizer boundaries"
    );
    assert!(
        !facts.is_statement_release_boundary_root(boundary_root, &drop_eligibility),
        "local-store roots defer to the Python boundary instead of statement release"
    );
    assert!(
        !facts.is_statement_release_boundary_root(bound_root, &drop_eligibility),
        "explicit release roots do not receive a second statement release"
    );
    assert!(
        !facts.is_statement_release_boundary_root(stack_root, &drop_eligibility),
        "local-store-bound roots are not statement release roots"
    );
    assert!(
        facts.is_return_boundary_deferred_root(deferred_root, &drop_eligibility),
        "bound_local attrs define Python return-boundary deferral roots"
    );
    assert_eq!(
        facts.return_boundary_candidate_roots(&drop_eligibility),
        [deferred_root].into_iter().collect(),
        "only positively named, independently owned, non-slot roots reach the return planner"
    );
    assert!(
        !facts.is_return_boundary_deferred_root(bound_root, &drop_eligibility)
            && !facts.is_return_boundary_deferred_root(loaded_root, &drop_eligibility),
        "slot-backed local roots keep their del/rebinding boundary"
    );
    assert!(
        !facts.is_return_boundary_deferred_root(conditional_root, &drop_eligibility),
        "conditionally-valid results are not total definitions and cannot defer to an unconditional return boundary"
    );
    let lat = OwnershipLattice::compute(&f, &aliases);
    let boundary_roots = facts.boundary_release_roots(&drop_eligibility, &lat);
    assert!(
        boundary_roots.contains(&boundary_root),
        "droppable local-store roots are Python boundary release roots"
    );
    assert!(
        !boundary_roots.contains(&bound_root),
        "explicitly released local-store roots must not get a second boundary release"
    );
    assert!(
        !boundary_roots.contains(&stack_root),
        "non-finalizer local-store roots do not require scope-exit deferral"
    );
    assert!(
        !boundary_roots.contains(&conditional_root),
        "conditionally-valid local-store roots must release only on valid paths"
    );
}

#[test]
fn statement_release_plan_filters_and_sorts_boundary_roots() {
    let mut f = func();
    let statement_list = f.fresh_value();
    let statement = f.fresh_value();
    let local_list = f.fresh_value();
    let local = f.fresh_value();
    let local_slot = f.fresh_value();
    let explicit_list = f.fresh_value();
    let explicit = f.fresh_value();
    let entry_id = f.entry_block;
    let entry = f.blocks.get_mut(&entry_id).unwrap();
    entry
        .ops
        .push(op(OpCode::BuildList, vec![], vec![statement_list]));
    entry.ops.push(del_op(statement));
    entry.ops.push(original_kind_copy(
        "list_append",
        vec![statement_list, statement],
        vec![],
    ));
    entry
        .ops
        .push(op(OpCode::BuildList, vec![], vec![local_list]));
    entry.ops.push(del_op(local));
    entry.ops.push(original_kind_copy(
        "store_var",
        vec![local],
        vec![local_slot],
    ));
    entry.ops.push(original_kind_copy(
        "list_append",
        vec![local_list, local],
        vec![],
    ));
    entry
        .ops
        .push(op(OpCode::BuildList, vec![], vec![explicit_list]));
    entry.ops.push(del_op(explicit));
    entry.ops.push(op(OpCode::DecRef, vec![explicit], vec![]));
    entry.ops.push(original_kind_copy(
        "list_append",
        vec![explicit_list, explicit],
        vec![],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    let lattice = OwnershipLattice::compute(&f, &aliases);
    let lifetime_facts = PythonLifetimeFacts::compute(&f, &aliases);
    let drop_eligibility = DropEligibility::new(&aliases, &root_facts, &HashSet::new());
    let plan = StatementReleasePlan::compute(&lattice, &lifetime_facts, &drop_eligibility);
    let statement_root = aliases.root(statement);
    let local_root = aliases.root(local);
    let explicit_root = aliases.root(explicit);

    assert!(
        plan.contains_released_root(statement_root),
        "ordinary finalizer producer temps release at their storage statement"
    );
    assert!(
        !plan.contains_released_root(local_root),
        "slot/local-managed roots defer to their Python lifetime boundary"
    );
    assert!(
        !plan.contains_released_root(explicit_root),
        "explicit DecRef roots do not receive a second statement release"
    );
    assert_eq!(
        plan.after_op()
            .get(&entry_id)
            .and_then(|by_op| by_op.get(&2))
            .cloned(),
        Some(vec![statement_root]),
        "the release plan stores sorted root-space releases by exact op boundary"
    );
}

#[test]
fn container_absorbing_finalizer_object_is_sensitive() {
    // The c_scope shape: `bag = [A()]` -> BuildList absorbs the __del__ object,
    // so the list value must also be finalizer-sensitive (releasing it fires A).
    let mut f = func();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_op(a));
    entry.ops.push(op(OpCode::BuildList, vec![a], vec![list]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(
        lat.is_finalizer_sensitive_root(a),
        "the __del__ object is sensitive"
    );
    assert!(
        lat.is_finalizer_sensitive_root(list),
        "the list absorbing the __del__ object must be sensitive (#58 c_scope)"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| boundary.op_index == 1 && boundary.root == a),
        "the producer temp has a separate absorption-boundary release fact"
    );
}

#[test]
fn copy_list_new_absorbing_finalizer_object_is_sensitive() {
    // Real SimpleIR lowering preserves `list_new` as Copy{_original_kind}
    // rather than canonicalizing it to BuildList. The generated
    // result-absorption fact must cover that spelling without aliasing it.
    let mut f = func();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_op(a));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![a], vec![list]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.is_finalizer_sensitive_root(a));
    assert!(
        lat.is_finalizer_sensitive_root(list),
        "Copy-preserved list_new must absorb the __del__ object's lifetime"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| boundary.op_index == 1 && boundary.root == a),
        "Copy-preserved list_new must mark the absorbed producer"
    );
}

#[test]
fn copy_class_def_absorbs_descriptor_into_class_owner() {
    let mut f = func();
    let name = f.fresh_value();
    let descriptor = f.fresh_value();
    let class_obj = f.fresh_value();
    let entry_id = f.entry_block;
    let entry = f.blocks.get_mut(&entry_id).unwrap();
    entry.ops.push(del_op(descriptor));
    entry.ops.push(original_kind_copy(
        "class_def",
        vec![name, descriptor],
        vec![class_obj],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    let descriptor_root = aliases.root(descriptor);
    let class_obj_root = aliases.root(class_obj);
    let lat = OwnershipLattice::compute(&f, &aliases);
    assert!(lat.is_finalizer_sensitive_root(descriptor_root));
    assert!(
        lat.is_finalizer_sensitive_root(class_obj_root),
        "Copy-preserved class_def must keep class-body descriptor lifetime behind the class owner"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| {
                boundary.block == entry_id
                    && boundary.op_index == 1
                    && boundary.root == descriptor_root
            }),
        "class_def must expose the exact class-construction absorption boundary (the absorbed descriptor temp)"
    );
}

#[test]
fn call_bind_defines_del_into_list_new_is_sensitive() {
    // Finalizer classes decline OBJECT_NEW_BOUND constructor folding, so the
    // real frontend shape is CALL_BIND(class_ref, callargs) -> list_new.
    let mut f = func();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_call_bind(a));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![a], vec![list]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(
        lat.is_finalizer_sensitive_root(a),
        "defines_del call result is the owning finalizer root"
    );
    assert!(
        lat.is_finalizer_sensitive_root(list),
        "Copy-preserved list_new must absorb the call-created finalizer object"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| boundary.op_index == 1 && boundary.root == a),
        "call-created finalizer temp must release at the list_new boundary"
    );
}

#[test]
fn slice_construction_absorbs_finalizer_bounds_in_both_spellings() {
    // `slice(A(), None)` keeps its bound alive until the slice dies. Real
    // SimpleIR keeps `slice_new` as Copy{_original_kind}; first-class
    // BuildSlice, which admits omitted bounds, shares the generated fact.
    for first_class in [false, true] {
        let mut f = func();
        let a = f.fresh_value();
        let slice = f.fresh_value();
        let stop = f.fresh_value();
        let step = f.fresh_value();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(del_op(a));
        entry.ops.push(if first_class {
            op(OpCode::BuildSlice, vec![a], vec![slice])
        } else {
            original_kind_copy("slice_new", vec![a, stop, step], vec![slice])
        });
        entry.terminator = Terminator::Return { values: vec![] };

        let lat = lattice(&f);
        assert!(lat.is_finalizer_sensitive_root(a));
        assert!(
            lat.is_finalizer_sensitive_root(slice),
            "slice construction (first class: {first_class}) must absorb the __del__ bound"
        );
        assert!(
            lat.statement_release_finalizer_boundaries()
                .iter()
                .any(|boundary| boundary.op_index == 1 && boundary.root == a),
            "slice construction (first class: {first_class}) must mark the absorbed producer"
        );
    }
}

#[test]
fn list_append_absorbs_producer_into_existing_container() {
    let mut f = func();
    let list = f.fresh_value();
    let a = f.fresh_value();
    let a_alias = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(op(OpCode::BuildList, vec![], vec![list]));
    entry.ops.push(del_op(a));
    entry
        .ops
        .push(original_kind_copy("copy", vec![a], vec![a_alias]));
    entry.ops.push(original_kind_copy(
        "list_append",
        vec![list, a_alias],
        vec![],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    let aliases = build_alias_union_find(&f);
    assert_eq!(
        aliases.root(a_alias),
        aliases.root(a),
        "the test fixture must prove finalizer facts are stored per root"
    );
    let lat = OwnershipLattice::compute(&f, &aliases);
    let a_root = aliases.root(a);
    assert!(
        lat.is_finalizer_sensitive_root(a_root),
        "the finalizer-sensitive producer root survives transparent aliases"
    );
    assert!(
        lat.is_finalizer_sensitive_root(list),
        "list_append must make the existing container finalizer-sensitive"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| boundary.op_index == 3 && boundary.root == a_root),
        "list_append must expose the exact absorbed producer root boundary (the appended producer temp)"
    );
}

#[test]
fn module_set_attr_absorbs_value_into_module_storage() {
    let mut f = func();
    let module = f.fresh_value();
    let name = f.fresh_value();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let entry_id = f.entry_block;
    let entry = f.blocks.get_mut(&entry_id).unwrap();
    entry.ops.push(del_op(a));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![a], vec![list]));
    entry
        .ops
        .push(op(OpCode::ModuleSetAttr, vec![module, name, list], vec![]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.is_finalizer_sensitive_root(a));
    assert!(
        lat.is_finalizer_sensitive_root(list),
        "list_new keeps the finalizer-bearing element behind the list owner"
    );
    assert!(
        lat.is_finalizer_sensitive_root(module),
        "module storage now owns a finalizer-sensitive value"
    );
    assert!(
        lat.statement_release_finalizer_boundaries()
            .iter()
            .any(|boundary| {
                boundary.block == entry_id && boundary.op_index == 2 && boundary.root == list
            }),
        "module_set_attr must release the compiler-owned value ref at the exact storage absorption boundary"
    );
}

#[test]
fn list_pop_result_inherits_finalizer_sensitivity_from_container() {
    let mut f = func();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let popped = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_op(a));
    entry
        .ops
        .push(original_kind_copy("list_new", vec![a], vec![list]));
    entry
        .ops
        .push(original_kind_copy("list_pop", vec![list], vec![popped]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.is_finalizer_sensitive_root(list));
    assert!(
        lat.is_finalizer_sensitive_root(popped),
        "list_pop result must inherit finalizer sensitivity from the source container \
         (the discarded pop result then releases via the normal dead-result DecRef path)"
    );
}

#[test]
fn non_finalizer_function_has_empty_set() {
    let mut f = func();
    let a = f.fresh_value();
    let list = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    // A plain object with no __del__ + a list of it: nothing is sensitive.
    entry.ops.push(op(OpCode::ObjectNewBound, vec![], vec![a]));
    entry.ops.push(op(OpCode::BuildList, vec![a], vec![list]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.finalizer_sensitive_roots().is_empty());
}

#[test]
fn nested_container_propagates() {
    // `[[A()]]` â€” the inner and outer list are both sensitive (fixpoint).
    let mut f = func();
    let a = f.fresh_value();
    let inner = f.fresh_value();
    let outer = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(del_op(a));
    entry.ops.push(op(OpCode::BuildList, vec![a], vec![inner]));
    entry
        .ops
        .push(op(OpCode::BuildList, vec![inner], vec![outer]));
    entry.terminator = Terminator::Return { values: vec![] };

    let lat = lattice(&f);
    assert!(lat.is_finalizer_sensitive_root(inner));
    assert!(lat.is_finalizer_sensitive_root(outer));
}

#[test]
fn declared_custody_owns_transferred_parameters_and_adopts_operands() {
    let mut f = TirFunction::new(
        "custody".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    f.set_parameter_custody(&[ParameterCustody::Borrowed, ParameterCustody::Transferred]);
    let borrowed = f.blocks[&f.entry_block].args[0].id;
    let transferred = f.blocks[&f.entry_block].args[1].id;
    let value = f.fresh_value();
    let builder = f.fresh_value();
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry
        .ops
        .push(op(OpCode::ObjectNewBound, vec![], vec![value]));
    entry.terminator = Terminator::Return { values: vec![] };

    let mut call = op(
        OpCode::Call,
        vec![borrowed, value, value, transferred],
        vec![],
    );
    call.set_argument_custody(&[
        ParameterCustody::Borrowed,
        ParameterCustody::Transferred,
        ParameterCustody::Transferred,
        ParameterCustody::Borrowed,
    ]);
    use OperandTransfer::{Adopted, Borrowed, Consumed};
    assert_eq!(
        op_transferred_operands(&call),
        [Borrowed, Adopted, Adopted, Borrowed],
        "typed custody adopts each position, a repeated root included"
    );
    let mut bind = op(OpCode::Call, vec![borrowed, builder], vec![]);
    bind.attrs
        .insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
    assert_eq!(
        op_transferred_operands(&bind),
        [Borrowed, Consumed],
        "call_bind consumes the CallArgs builder it frees"
    );
    bind.set_argument_custody(&[ParameterCustody::Transferred, ParameterCustody::Transferred]);
    assert_eq!(
        op_transferred_operands(&bind),
        [Adopted, Consumed],
        "a position both declare is consumed"
    );
    let release = op(OpCode::DecRef, vec![value], vec![]);
    assert_eq!(
        op_transferred_operands(&release),
        [Borrowed],
        "an explicit release ends the holder's own reference; it takes nothing"
    );

    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    let eligibility = DropEligibility::new(&aliases, &root_facts, &HashSet::new());
    let facts = PythonLifetimeFacts::compute(&f, &aliases);
    let lat = OwnershipLattice::compute(&f, &aliases);
    assert!(
        root_facts.is_borrowed_parameter_root(borrowed) && !eligibility.is_droppable(borrowed),
        "a borrowed parameter stays the caller's"
    );
    assert!(
        !root_facts.is_borrowed_parameter_root(transferred)
            && eligibility.is_droppable(transferred),
        "a transferred parameter is function-owned"
    );
    assert!(
        !eligibility.return_requires_owned_publication(transferred),
        "returning a transferred parameter moves its reference"
    );
    assert_eq!(
        facts.boundary_release_roots(&eligibility, &lat),
        [transferred].into_iter().collect(),
        "a transferred parameter is a Python binding released at its boundary"
    );
    assert!(
        !facts.is_statement_release_boundary_root(transferred, &eligibility),
        "a parameter binding is never a statement temporary"
    );
}

/// A replaced result keeps the owner its operation's result contract gave it,
/// by the result eligibility DropInsertion reads, and gains none. Four results
/// are rewritten into copies of one string, as value numbering rewrites a result
/// into a copy of its equal:
/// * an owned `Call` result that is read keeps its reference as an owned alias;
/// * a borrowed getter's result (`dict_get`), its own alias root yet no owner,
///   stays a transparent copy;
/// * a no-op `TypeGuard`'s result forwards the string's reference and stays a
///   transparent copy;
/// * an owned result that nothing reads names nothing and stays transparent.
///
/// DropInsertion then releases the string once and the kept owner once.
#[test]
fn replaced_results_keep_exactly_the_owners_they_had() {
    let mut f = func();
    let text = f.fresh_value();
    let (owned, borrowed, guarded, unread) = (
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
    );
    let mut string = op(OpCode::ConstStr, vec![], vec![text]);
    string
        .attrs
        .insert("s_value".into(), AttrValue::Str("x".into()));
    let mut guard = op(OpCode::TypeGuard, vec![text], vec![guarded]);
    guard
        .attrs
        .insert("expected_type".into(), AttrValue::Str("str".into()));
    let replaced = vec![
        op(OpCode::Call, vec![text], vec![owned]),
        original_kind_copy("dict_get", vec![text, text], vec![borrowed]),
        guard,
        op(OpCode::Call, vec![text], vec![unread]),
    ];
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops.push(string);
    entry.ops.extend(replaced.iter().cloned());
    entry.ops.push(op(
        OpCode::WarnStderr,
        vec![owned, borrowed, guarded],
        vec![],
    ));
    entry.terminator = Terminator::Return { values: vec![] };

    // The per-operation query is the root facts' result eligibility.
    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    for operation in &replaced {
        let result = operation.results[0];
        assert_eq!(
            OwnershipRootFacts::result_holds_own_reference(operation, result, &aliases),
            aliases.root(result) == result && root_facts.is_drop_owned_root_candidate(result),
            "{operation:?}"
        );
    }

    let mut owners = Replacements::new(&f);
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    for index in 1..=replaced.len() {
        owners.record(&entry.ops[index]);
        let result = entry.ops[index].results[0];
        entry.ops[index] = op(OpCode::Copy, vec![text], vec![result]);
    }
    owners.finish(&mut f, None);
    let kinds: Vec<Option<&str>> = f.blocks[&f.entry_block].ops[1..=replaced.len()]
        .iter()
        .map(original_kind)
        .collect();
    assert_eq!(kinds, [Some("binding_alias"), None, None, None]);

    crate::tir::passes::drop_insertion::run(
        &mut f,
        &mut crate::tir::analysis::AnalysisManager::new(),
    );
    let mut released: Vec<ValueId> = f.blocks[&f.entry_block]
        .ops
        .iter()
        .filter(|operation| operation.opcode == OpCode::DecRef)
        .flat_map(|operation| operation.operands.iter().copied())
        .collect();
    released.sort_unstable_by_key(|value| value.0);
    assert_eq!(
        released,
        [text, owned],
        "the string and the kept owner, once each"
    );
    assert!(
        !f.blocks[&f.entry_block]
            .ops
            .iter()
            .any(|operation| operation.opcode == OpCode::IncRef),
        "a transparent copy needs no retain"
    );
}

/// Frame binding views hold no reference, and neither does a block argument
/// that every binding arc passes a view: a loop header carrying store and load
/// views around a loop, and a handler argument that a raising observation binds
/// to a view. A join that also takes an owned temporary or a raw constant owns
/// what it holds. The frame home store consumes its operand, its view result
/// held nothing to keep, and a returned view needs a publication retain.
#[test]
fn binding_views_join_through_block_arguments() {
    use crate::tir::blocks::TirBlock;
    use crate::tir::values::TirValue;
    assert!(
        crate::tir::op_kinds_generated::copy_kind_is_binding_view_table("frame_home_store")
            && crate::tir::op_kinds_generated::copy_kind_is_binding_view_table("frame_home_load"),
        "needs the frame overlay's generated classifier_binding_view \
         (frame-slot-custody COORDINATION §3)"
    );
    let mut f = func();
    let (value, first, raw, fresh) = (
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
    );
    let (entry_cond, loop_cond, rebound) = (f.fresh_value(), f.fresh_value(), f.fresh_value());
    let (looped, mixed, with_raw, handled) = (
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
        f.fresh_value(),
    );
    let (header, body, join, handler) = (
        f.fresh_block(),
        f.fresh_block(),
        f.fresh_block(),
        f.fresh_block(),
    );
    f.label_id_map.insert(handler.0, 61);
    let mut store = original_kind_copy("frame_home_store", vec![value], vec![first]);
    store.attrs.insert("value".into(), AttrValue::Int(0));
    let mut load = original_kind_copy("frame_home_load", vec![], vec![rebound]);
    load.attrs.insert("value".into(), AttrValue::Int(0));
    let mut check = op(OpCode::CheckException, vec![first], vec![]);
    check.attrs.insert("value".into(), AttrValue::Int(61));
    let mut constant = op(OpCode::ConstInt, vec![], vec![raw]);
    constant.attrs.insert("value".into(), AttrValue::Int(3));
    let entry = f.blocks.get_mut(&f.entry_block).unwrap();
    entry.ops = vec![
        op(OpCode::Call, vec![], vec![value]),
        store.clone(),
        constant,
        op(OpCode::Call, vec![], vec![fresh]),
        op(OpCode::ConstBool, vec![], vec![entry_cond]),
        check,
    ];
    entry.terminator = Terminator::CondBranch {
        cond: entry_cond,
        then_block: header,
        then_args: vec![first],
        else_block: join,
        else_args: vec![fresh, raw],
    };
    let argument = |id| TirValue {
        id,
        ty: TirType::DynBox,
    };
    f.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![argument(looped)],
            ops: vec![op(OpCode::ConstBool, vec![], vec![loop_cond])],
            terminator: Terminator::CondBranch {
                cond: loop_cond,
                then_block: body,
                then_args: vec![],
                else_block: join,
                else_args: vec![looped, looped],
            },
        },
    );
    f.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![load],
            terminator: Terminator::Branch {
                target: header,
                args: vec![rebound],
            },
        },
    );
    f.blocks.insert(
        join,
        TirBlock {
            id: join,
            args: vec![argument(mixed), argument(with_raw)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    f.blocks.insert(
        handler,
        TirBlock {
            id: handler,
            args: vec![argument(handled)],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );

    let aliases = build_alias_union_find(&f);
    let root_facts = OwnershipRootFacts::compute(&f, &aliases);
    for view in [first, rebound, looped, handled] {
        assert!(
            root_facts.is_binding_view_root(view) && !root_facts.is_drop_owned_root_candidate(view),
            "{view:?} is a view"
        );
    }
    for owner in [value, fresh, mixed, with_raw] {
        assert!(
            !root_facts.is_binding_view_root(owner)
                && root_facts.is_drop_owned_root_candidate(owner),
            "{owner:?} owns what it holds"
        );
    }
    let eligibility = DropEligibility::new(&aliases, &root_facts, &HashSet::new());
    assert!(eligibility.return_requires_owned_publication(looped));
    assert!(!eligibility.return_requires_owned_publication(mixed));
    assert!(!OwnershipRootFacts::result_holds_own_reference(
        &store, first, &aliases
    ));
    assert_eq!(
        op_transferred_operands(&store),
        [OperandTransfer::BindingStore]
    );
}

#[test]
fn explicit_release_projection_selects_operands_and_canonicalizes_roots_once() {
    let mut function = TirFunction::new(
        "release_projection".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let first = function.blocks[&function.entry_block].args[0].id;
    let second = function.blocks[&function.entry_block].args[1].id;
    let alias = function.fresh_value();
    function
        .blocks
        .get_mut(&function.entry_block)
        .unwrap()
        .ops
        .push(original_kind_copy("copy", vec![first], vec![alias]));
    for (opcode, operands, expected) in [
        (OpCode::DecRef, vec![alias, second], vec![alias, second]),
        (OpCode::DeleteVar, vec![first, second], vec![second]),
        (OpCode::DelBoundary, vec![alias], vec![alias]),
        (OpCode::IncRef, vec![first], vec![]),
        (OpCode::Free, vec![first], vec![]),
        (OpCode::DeleteVar, vec![first], vec![]),
    ] {
        let release = op(opcode, operands, vec![]);
        assert_eq!(
            explicit_release_values(&release).collect::<Vec<_>>(),
            expected,
            "{opcode:?}"
        );
        function
            .blocks
            .get_mut(&function.entry_block)
            .unwrap()
            .ops
            .push(release);
    }
    let aliases = build_alias_union_find(&function);
    assert_eq!(aliases.root(alias), aliases.root(first));
    let facts = PythonLifetimeFacts::compute(&function, &aliases);
    assert_eq!(
        facts.explicit_release_roots(),
        &HashSet::from([aliases.root(first), aliases.root(second),])
    );
    for &root in facts.explicit_release_roots() {
        assert!(facts.has_explicit_release_boundary(root));
    }
}

#[test]
fn physical_release_does_not_invent_or_erase_python_binding_provenance() {
    let mut function = TirFunction::new(
        "binding_provenance".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    function.set_parameter_custody(&[molt_ir::ParameterCustody::Transferred]);
    let parameter = function.blocks[&function.entry_block].args[0].id;
    let temporary = function.fresh_value();
    let deleted = function.fresh_value();
    let replaced = function.fresh_value();
    let named = function.fresh_value();
    for value in [temporary, deleted, replaced, named] {
        function.value_types.insert(value, TirType::DynBox);
        let mut producer = op(OpCode::Call, vec![], vec![value]);
        if value == named {
            producer
                .attrs
                .insert("bound_local".into(), AttrValue::Bool(true));
        }
        function
            .blocks
            .get_mut(&function.entry_block)
            .unwrap()
            .ops
            .push(producer);
    }
    let entry = function.blocks.get_mut(&function.entry_block).unwrap();
    entry.ops.extend([
        op(OpCode::DecRef, vec![temporary], vec![]),
        op(OpCode::DelBoundary, vec![deleted], vec![]),
        op(OpCode::DeleteVar, vec![temporary, replaced], vec![]),
        op(OpCode::DecRef, vec![named], vec![]),
        op(OpCode::DecRef, vec![parameter], vec![]),
    ]);
    let aliases = build_alias_union_find(&function);
    let ownership = OwnershipRootFacts::compute(&function, &aliases);
    let raw = HashSet::new();
    let eligibility = DropEligibility::new(&aliases, &ownership, &raw);
    let lattice = OwnershipLattice::compute(&function, &aliases);
    let mut facts = PythonLifetimeFacts::compute(&function, &aliases);
    assert_eq!(
        facts.binding_custody_roots(&eligibility, &lattice),
        HashSet::from([parameter, deleted, replaced, named]),
    );
    assert!(facts.has_explicit_release_boundary(temporary));
    // Physical normalization must not erase source deletion provenance; a
    // deleted release must also stop claiming a physical release obligation.
    function
        .blocks
        .get_mut(&function.entry_block)
        .unwrap()
        .ops
        .retain(|op| op.opcode != OpCode::DelBoundary);
    facts.refresh_explicit_release_roots(&function, &aliases);
    assert!(!facts.has_explicit_release_boundary(deleted));
    assert!(
        facts
            .binding_custody_roots(&eligibility, &lattice)
            .contains(&deleted)
    );
}
