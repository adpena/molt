use super::*;

/// Deterministic typed transport for values live into a semantic TIR block.
///
/// Stack/frame-backed names already have explicit memory custody and are not
/// duplicated here. Proven immutable values bind their actual emitted SSA
/// value directly. Only names needing incoming transport receive parameters;
/// every semantic predecessor emits the matching argument vector.
#[cfg(feature = "native-backend")]
#[derive(Clone, Debug)]
pub(in crate::native_backend::function_compiler) struct BlockTransportPlan {
    vars: Vec<Variable>,
    types: Vec<cranelift_codegen::ir::Type>,
    direct: Vec<(u32, Variable)>,
}

#[cfg(feature = "native-backend")]
impl BlockTransportPlan {
    #[cfg(test)]
    pub(in crate::native_backend::function_compiler) fn for_test(
        vars: Vec<Variable>,
        types: Vec<cranelift_codegen::ir::Type>,
    ) -> Self {
        Self {
            vars,
            types,
            direct: Vec::new(),
        }
    }

    pub(in crate::native_backend::function_compiler) fn from_live_ids(
        live_ids: impl Iterator<Item = u32>,
        ssa_values: &NativeSsaValues,
        point: crate::tir::dominators::SimpleProgramPoint,
    ) -> Self {
        let mut plan_vars = Vec::new();
        let mut types = Vec::new();
        let mut direct = Vec::new();
        for id in live_ids {
            let Some(binding) = ssa_values.bindings[id as usize] else {
                continue;
            };
            if ssa_values.can_bind_directly(id, point) {
                direct.push((id, binding.var));
            } else {
                plan_vars.push(binding.var);
                types.push(binding.ty);
            }
        }
        Self {
            vars: plan_vars,
            types,
            direct,
        }
    }

    pub(in crate::native_backend::function_compiler) fn append_block_params(
        &self,
        builder: &mut FunctionBuilder<'_>,
        block: Block,
    ) {
        for &ty in &self.types {
            builder.append_block_param(block, ty);
        }
    }

    pub(in crate::native_backend::function_compiler) fn edge_args(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) -> Vec<Value> {
        self.vars.iter().map(|&var| builder.use_var(var)).collect()
    }

    pub(in crate::native_backend::function_compiler) fn bind_block_params(
        &self,
        builder: &mut FunctionBuilder<'_>,
        block: Block,
        ssa_values: &NativeSsaValues,
    ) {
        let params = builder.block_params(block).to_vec();
        assert_eq!(
            params.len(),
            self.vars.len(),
            "semantic transport block parameter arity drift"
        );
        for (&var, param) in self.vars.iter().zip(params) {
            builder.def_var(var, param);
        }
        for &(id, var) in &self.direct {
            let value = ssa_values.emitted[id as usize].unwrap_or_else(|| {
                panic!("proven canonical SSA definition #{id} was not captured before block materialization")
            });
            builder.def_var(var, value);
        }
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
struct NativeTransportBinding {
    var: Variable,
    ty: cranelift_codegen::ir::Type,
}

/// Retained emitted values indexed by the canonical liveness name table.
/// Variable/representation/storage projections are resolved once after setup;
/// they never establish identity or create another name registry.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct NativeSsaValues {
    definitions: Vec<Option<crate::tir::simple_def_use::SimpleDefinitionSite>>,
    dominance: Option<crate::tir::dominators::SimpleExecutionDominance>,
    emitted: Vec<Option<Value>>,
    bindings: Vec<Option<NativeTransportBinding>>,
}

#[cfg(feature = "native-backend")]
impl NativeSsaValues {
    fn with_facts(
        parameters: &[String],
        ops: &[OpIR],
        names: &crate::tir::cfg_liveness::SimpleNameTable,
        dominance: Option<crate::tir::dominators::SimpleExecutionDominance>,
    ) -> Self {
        let facts = crate::tir::simple_def_use::SimpleDefinitionFacts::compute(parameters, ops);
        Self {
            definitions: (0..names.len())
                .map(|id| facts.unique_definition(names.name(id as u32)))
                .collect(),
            dominance,
            emitted: vec![None; names.len()],
            bindings: vec![None; names.len()],
        }
    }

    #[cfg(test)]
    pub(in crate::native_backend::function_compiler) fn for_test(
        parameters: &[String],
        ops: &[OpIR],
        names: &crate::tir::cfg_liveness::SimpleNameTable,
    ) -> Self {
        Self::with_facts(
            parameters,
            ops,
            names,
            Some(crate::tir::cfg::CFG::build(ops).execution_points(ops)),
        )
    }

    pub(in crate::native_backend::function_compiler) fn for_function(
        func: &FunctionIR,
        cfg: &crate::tir::cfg::CFG,
        stateful: bool,
        names: &crate::tir::cfg_liveness::SimpleNameTable,
    ) -> Self {
        // Backend-managed loop carriers and resume-frame values do not expose
        // every definition as an immutable value in this invocation.
        let explicit_definitions = !stateful
            && !func.ops.iter().any(|op| {
                crate::tir::op_kinds_generated::simpleir_kind_is_pre_ssa_rewritten(&op.kind)
            });
        Self::with_facts(
            &func.params,
            &func.ops,
            names,
            explicit_definitions.then(|| cfg.execution_points(&func.ops)),
        )
    }

    pub(in crate::native_backend::function_compiler) fn project_variables(
        &mut self,
        names: &crate::tir::cfg_liveness::SimpleNameTable,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
        slots: &BTreeMap<String, cranelift_codegen::ir::StackSlot>,
    ) {
        for (id, binding) in self.bindings.iter_mut().enumerate() {
            let name = names.name(id as u32);
            if name == "none" || slots.contains_key(name) {
                continue;
            }
            if let Some(&var) = vars.get(name) {
                *binding = Some(NativeTransportBinding {
                    var,
                    ty: if representation_plan.is_float_unboxed(name) {
                        types::F64
                    } else {
                        types::I64
                    },
                });
            }
        }
    }

    pub(in crate::native_backend::function_compiler) fn can_bind_directly(
        &self,
        id: u32,
        point: crate::tir::dominators::SimpleProgramPoint,
    ) -> bool {
        let Some(definition) = self.definitions[id as usize] else {
            return false;
        };
        // Textual emission is independent of execution dominance. A later
        // initializer dominating a backward body still needs explicit transport.
        if let crate::tir::simple_def_use::SimpleDefinitionSite::Operation(index) = definition
            && index >= point.operation()
        {
            return false;
        }
        self.dominance
            .as_ref()
            .is_some_and(|d| d.definition_available(definition, point))
    }

    pub(in crate::native_backend::function_compiler) fn capture_parameters(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        parameters: &[String],
        names: &crate::tir::cfg_liveness::SimpleNameTable,
    ) {
        if self.dominance.is_none() {
            return;
        }
        for name in parameters {
            let Some(id) = names.id(name) else {
                continue;
            };
            if self.definitions[id as usize]
                == Some(crate::tir::simple_def_use::SimpleDefinitionSite::Invocation)
                && let Some(binding) = self.bindings[id as usize]
            {
                self.emitted[id as usize] = Some(builder.use_var(binding.var));
            }
        }
    }

    pub(in crate::native_backend::function_compiler) fn capture_operation(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        index: usize,
        op: &OpIR,
        names: &crate::tir::cfg_liveness::SimpleNameTable,
    ) {
        if self.dominance.is_none() {
            return;
        }
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            let id = names.id(name).expect("canonical definition name");
            if self.definitions[id as usize]
                == Some(crate::tir::simple_def_use::SimpleDefinitionSite::Operation(
                    index,
                ))
                && let Some(binding) = self.bindings[id as usize]
            {
                self.emitted[id as usize] = Some(builder.use_var(binding.var));
            }
        });
    }
}

/// Zero-runtime-cost preservation of unrelated SSA values across a backend
/// mini-CFG emitted while lowering one SimpleIR operation.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct OpLiveThroughSnapshot {
    origin_block: Option<Block>,
    vars: Vec<Variable>,
    values: Vec<Value>,
}

#[cfg(feature = "native-backend")]
impl OpLiveThroughSnapshot {
    pub(in crate::native_backend::function_compiler) fn empty() -> Self {
        Self {
            origin_block: None,
            vars: Vec::new(),
            values: Vec::new(),
        }
    }

    pub(in crate::native_backend::function_compiler) fn capture(
        builder: &mut FunctionBuilder<'_>,
        live_after: &[u32],
        defining_op: &OpIR,
        names: &crate::tir::cfg_liveness::SimpleNameTable,
        ssa_values: &NativeSsaValues,
        point: crate::tir::dominators::SimpleProgramPoint,
    ) -> Self {
        let mut defined = Vec::new();
        crate::tir::simple_def_use::visit_simple_ir_defined_names(defining_op, |name| {
            defined.push(names.id(name).expect("canonical definition name"));
        });
        let mut snapshot_vars = Vec::new();
        let mut values = Vec::new();
        for &id in live_after {
            if defined.contains(&id) {
                continue;
            }
            let Some(binding) = ssa_values.bindings[id as usize] else {
                continue;
            };
            snapshot_vars.push(binding.var);
            values.push(if ssa_values.can_bind_directly(id, point) {
                ssa_values.emitted[id as usize]
                    .expect("live-through SSA definition was not captured")
            } else {
                builder.use_var(binding.var)
            });
        }
        Self {
            origin_block: builder.current_block(),
            vars: snapshot_vars,
            values,
        }
    }

    pub(in crate::native_backend::function_compiler) fn rebind(
        &self,
        builder: &mut FunctionBuilder<'_>,
    ) {
        if builder.current_block() == self.origin_block {
            return;
        }
        for (&var, &value) in self.vars.iter().zip(&self.values) {
            builder.def_var(var, value);
        }
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn collect_slot_backed_join_names(
    ops: &[OpIR],
    exception_label_ids: &BTreeSet<i64>,
    stateful: bool,
) -> BTreeSet<String> {
    let mut slot_backed_join_names: BTreeSet<String> = BTreeSet::new();

    // Join carriers that are explicitly materialized with store/load in the IR
    // are memory-backed transport by construction. Keep them on the stack-backed
    // path so later label materialization does not try to reinterpret them as
    // structured phi joins.
    for op in ops {
        if let Some(binding) = simple_ir_binding(op)
            && is_join_slot_name(binding.destination)
        {
            slot_backed_join_names.insert(binding.destination.to_string());
        }
    }

    // Stateful functions (generators / async / comprehension polls) carry their
    // SSA values across state_yield / state_label resume points the same way
    // exception-bearing functions carry values across check_exception splits.
    // The state machine generates many block edges that aren't eagerly sealed,
    // so phi resolution at seal_all_blocks() time can explode block-parameter
    // counts past regalloc2's u32-indexed entity tables (u32::MAX panic).
    //
    // Treat stateful functions like exception functions: route all store_var
    // targets through stack slots so the state machine carries memory values,
    // not SSA values, across resume edges.
    if exception_label_ids.is_empty() && !stateful {
        return slot_backed_join_names;
    }

    let mut exception_region_depth = 0i32;
    let mut first_seen_join_in_exception: BTreeMap<String, bool> = BTreeMap::new();
    let mut exception_written_locals: BTreeSet<String> = BTreeSet::new();

    // Collect ALL persistent local-slot mutation targets that appear anywhere
    // in a function with exception handling or stateful resume points. When the function defers
    // block sealing to seal_all_blocks(), Cranelift must resolve SSA phi
    // nodes for every variable that has definitions reaching from different
    // predecessors. Each check_exception or state_yield creates a new block
    // split, and variables carried across these splits become block
    // parameters. In functions with many such splits (e.g. try/except
    // bodies, generator/async poll state machines), the block parameter
    // count explodes and can overflow regalloc2's internal index tables
    // (u32::MAX index panic).
    //
    // By routing all persistent local storage through stack slots instead of SSA
    // variables, we eliminate the phi nodes entirely. Stack loads/stores
    // are slightly slower than register-to-register moves, but:
    // 1. Exception-handling and poll functions are already on the cold path
    // 2. The alternative is a hard backend compile failure
    // 3. regalloc2 phi resolution for many-predecessor blocks is O(n^2)
    //
    // This is the same strategy used by LLVM's mem2reg in the presence of
    // exception handling: keep values in memory across EH boundaries.
    let mut all_store_var_targets: BTreeSet<String> = BTreeSet::new();
    for op in ops {
        if let Some(binding) = simple_ir_binding(op)
            && is_persistent_local_slot_name(binding.destination)
        {
            all_store_var_targets.insert(binding.destination.to_string());
        }
    }
    // All persistent store_var targets in exception-bearing or stateful functions
    // use stack slots. Compiler SSA temps remain SSA values; they are not Python
    // local storage and widening them to stack slots can erase representation
    // facts at check_exception boundaries.
    slot_backed_join_names.extend(all_store_var_targets);

    for op in ops {
        match op.kind.as_str() {
            "try_start" => {
                exception_region_depth += 1;
            }
            "exception_pop" => {
                exception_region_depth = (exception_region_depth - 1).max(0);
            }
            _ if exception_region_depth > 0 && simple_ir_binding(op).is_some() => {
                if let Some(binding) = simple_ir_binding(op)
                    && is_persistent_local_slot_name(binding.destination)
                {
                    let name = binding.destination;
                    exception_written_locals.insert(name.to_string());
                    if is_join_slot_name(name) {
                        first_seen_join_in_exception
                            .entry(name.to_string())
                            .or_insert(true);
                    }
                }
            }
            "copy_var" | "load_var" if exception_region_depth > 0 => {
                // Use the same generated read roles as copy/load emission:
                // explicit args make `var` metadata, not another storage home.
                if let Some(name) = preanalyze_alias_source(op)
                    && is_join_slot_name(name)
                {
                    first_seen_join_in_exception
                        .entry(name.to_string())
                        .or_insert(true);
                }
            }
            _ => {}
        }
    }
    for (name, in_exception) in first_seen_join_in_exception {
        if in_exception {
            slot_backed_join_names.insert(name);
        }
    }
    slot_backed_join_names.extend(exception_written_locals);
    slot_backed_join_names
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn materialize_label_block(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
    transport: Option<&BlockTransportPlan>,
    ssa_values: &NativeSsaValues,
) {
    ensure_block_in_layout(builder, block);
    // If we're already inside `block` and it's still open, the label has
    // effectively materialised in place — do not emit a self-jump to itself,
    // which would (a) close the block, (b) wire it as its own predecessor,
    // and (c) generate an unreachable trailing instruction. The
    // `is_block_filled` guard alone is not sufficient because a fresh
    // resume block created by `state_yield` lowering also has
    // `is_block_filled == false` while already being the current block.
    let already_in_target = builder.current_block() == Some(block);
    if !already_in_target {
        if !*is_block_filled {
            let args = transport
                .map(|plan| plan.edge_args(builder))
                .unwrap_or_default();
            jump_block(builder, block, &args);
        }
        crate::switch_to_block_tracking(builder, block, is_block_filled);
    }
    if let Some(plan) = transport {
        plan.bind_block_params(builder, block, ssa_values);
    }
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn switch_to_block_materialized(
    builder: &mut FunctionBuilder,
    block: Block,
) {
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
}
