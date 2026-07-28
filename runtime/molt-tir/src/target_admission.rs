//! Shared pre-source target admission for semantic domains that a backend's
//! value model may not represent exactly.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::representation_plan::ScalarRepresentationPlan;
use crate::tir::cfg::CFG;
use crate::tir::op_kinds_generated::{
    SimpleIrIntegerSemantics, SimpleIrModuleIdentityAliasRole, SimpleIrModuleSlotRole,
    SimpleIrRuntimeRequirements, SimpleIrVarFieldRole, simpleir_integer_semantics_table,
    simpleir_kind_has_callable_operand, simpleir_module_identity_alias_role_table,
    simpleir_module_identity_source_name_arg, simpleir_module_slot_access_table,
    simpleir_runtime_requirements_table, simpleir_runtime_symbol_requirements_table,
    simpleir_var_field_role_table,
};
use crate::tir::simple_def_use::visit_simple_ir_reads;
use crate::{FunctionIR, OpIR, SimpleIR};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NumericTargetCapabilities {
    pub arbitrary_precision_integers: bool,
    /// Largest concrete integer magnitude the target carrier represents
    /// exactly. This admits only literal values proven in range; it does not
    /// authorize integer-producing operations that can overflow that range.
    pub exact_integer_literal_max_magnitude: Option<u128>,
    pub cpython_float_divmod: bool,
    pub cpython_power: bool,
}

impl NumericTargetCapabilities {
    /// A fixed-width/double target with no complex-number representation and no
    /// shared CPython-exact float divmod authority.
    pub const FIXED_WIDTH_FLOAT_ONLY: Self = Self {
        arbitrary_precision_integers: false,
        exact_integer_literal_max_magnitude: None,
        cpython_float_divmod: false,
        cpython_power: false,
    };

    /// Luau's sole numeric carrier is IEEE-754 binary64. Every integer through
    /// magnitude 2^53 is represented exactly, while general integer producers
    /// remain inadmissible without an arbitrary-precision value authority.
    pub const LUAU_EXACT_INTEGER_LITERALS: Self = Self {
        arbitrary_precision_integers: false,
        exact_integer_literal_max_magnitude: Some(1_u128 << 53),
        cpython_float_divmod: false,
        cpython_power: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeTargetCapabilities {
    pub execution_frame_state: bool,
    pub python_frame_introspection: bool,
    pub python_identity: bool,
    pub tuple_representation: bool,
    pub exception_model: bool,
    pub deterministic_lifetime: bool,
    pub format_protocol: bool,
    pub iterable_protocol: bool,
    pub object_model: bool,
    pub python_truthiness: bool,
    pub python_comparison: bool,
    pub structured_runtime_errors: bool,
    pub async_runtime: bool,
    pub unstructured_control_flow: bool,
    pub host_capabilities: bool,
}

impl RuntimeTargetCapabilities {
    pub const NONE: Self = Self {
        execution_frame_state: false,
        python_frame_introspection: false,
        python_identity: false,
        tuple_representation: false,
        exception_model: false,
        deterministic_lifetime: false,
        format_protocol: false,
        iterable_protocol: false,
        object_model: false,
        python_truthiness: false,
        python_comparison: false,
        structured_runtime_errors: false,
        async_runtime: false,
        unstructured_control_flow: false,
        host_capabilities: false,
    };
}

/// Validate transport shape and every generated semantic-role family before
/// any target source buffer is touched.
pub fn validate_target_contract(
    ir: &SimpleIR,
    target: &str,
    numeric: NumericTargetCapabilities,
    runtime: RuntimeTargetCapabilities,
) -> Result<(), String> {
    crate::validate_simple_ir(ir)
        .map_err(|error| format!("{target} SimpleIR validation failed: {error}"))?;
    validate_numeric_target_contract(ir, target, numeric)?;
    validate_runtime_target_contract(ir, target, runtime)
}

/// Reject unsupported numeric semantics before a backend assembles source.
///
/// Operation membership comes exclusively from the generated op-kind registry;
/// target policy supplies only capabilities. Representation facts may prove a
/// dynamic operation stays entirely in a non-integer domain.
pub fn validate_numeric_target_contract(
    ir: &SimpleIR,
    target: &str,
    capabilities: NumericTargetCapabilities,
) -> Result<(), String> {
    for function in &ir.functions {
        let plan = ScalarRepresentationPlan::for_function_ir(function);
        for (index, op) in function.ops.iter().enumerate() {
            let role = simpleir_integer_semantics_table(op.kind.as_str());
            if let Some(reason) = numeric_admission_failure(&plan, op, role, capabilities) {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: {reason}",
                    function.name, op.kind,
                ));
            }
        }
    }
    Ok(())
}

pub fn validate_runtime_target_contract(
    ir: &SimpleIR,
    target: &str,
    capabilities: RuntimeTargetCapabilities,
) -> Result<(), String> {
    let admission_checks = runtime_admission_checks(capabilities);
    for function in &ir.functions {
        let propagated_requirements = simpleir_callable_runtime_requirements(function);
        for (index, op) in function.ops.iter().enumerate() {
            let Some(mut requirements) = simpleir_op_runtime_requirements(op) else {
                return Err(format!(
                    "{target} target rejected before source generation: {}:op#{index} `{}`: operation is unclassified in the generated runtime semantic authority",
                    function.name, op.kind,
                ));
            };
            requirements = requirements.union(propagated_requirements[index]);
            if requirements.is_empty() {
                continue;
            }
            for &(requirement, supported, reason) in &admission_checks {
                if requirements.contains(requirement) && !supported {
                    return Err(format!(
                        "{target} target rejected before source generation: {}:op#{index} `{}`: {reason}",
                        function.name, op.kind,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn requirement_for_name(
    requirements: &BTreeMap<String, SimpleIrRuntimeRequirements>,
    name: &str,
) -> SimpleIrRuntimeRequirements {
    requirements
        .get(name)
        .copied()
        .unwrap_or(SimpleIrRuntimeRequirements::NONE)
}

fn union_requirement(
    requirements: &mut BTreeMap<String, SimpleIrRuntimeRequirements>,
    name: &str,
    incoming: SimpleIrRuntimeRequirements,
) {
    if incoming.is_empty() || name == "none" {
        return;
    }
    let prior = requirement_for_name(requirements, name);
    requirements.insert(name.to_string(), prior.union(incoming));
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CallableBindingState {
    locals: BTreeMap<String, SimpleIrRuntimeRequirements>,
    local_modules: BTreeMap<String, BTreeSet<String>>,
    modules: BTreeMap<(String, String), SimpleIrRuntimeRequirements>,
}

fn merge_binding_states<'a>(
    states: impl Iterator<Item = &'a CallableBindingState>,
) -> CallableBindingState {
    let mut merged = CallableBindingState::default();
    for state in states {
        for (slot, requirement) in &state.locals {
            union_requirement(&mut merged.locals, slot, *requirement);
        }
        for (slot, identities) in &state.local_modules {
            merged
                .local_modules
                .entry(slot.clone())
                .or_default()
                .extend(identities.iter().cloned());
        }
        for (slot, requirement) in &state.modules {
            let prior = merged
                .modules
                .get(slot)
                .copied()
                .unwrap_or(SimpleIrRuntimeRequirements::NONE);
            merged
                .modules
                .insert(slot.clone(), prior.union(*requirement));
        }
    }
    merged
}

fn replace_requirement(
    requirements: &mut BTreeMap<String, SimpleIrRuntimeRequirements>,
    name: &str,
    incoming: SimpleIrRuntimeRequirements,
) {
    if incoming.is_empty() {
        requirements.remove(name);
    } else {
        requirements.insert(name.to_string(), incoming);
    }
}

fn replace_module_requirement(
    requirements: &mut BTreeMap<(String, String), SimpleIrRuntimeRequirements>,
    key: (String, String),
    incoming: SimpleIrRuntimeRequirements,
) {
    if incoming.is_empty() {
        requirements.remove(&key);
    } else {
        requirements.insert(key, incoming);
    }
}

fn module_identities_for(
    identities: &BTreeMap<String, BTreeSet<String>>,
    value: &str,
) -> BTreeSet<String> {
    identities
        .get(value)
        .cloned()
        .unwrap_or_else(|| BTreeSet::from([format!("value:{value}")]))
}

/// Propagate canonical callable provenance through the generated SimpleIR
/// field roles and the mutable local/module slots those roles describe. The
/// frontend owns qualified import resolution; this pass owns only transport
/// dataflow and never guesses Python spellings in a backend.
fn simpleir_callable_runtime_requirements(
    function: &FunctionIR,
) -> Vec<SimpleIrRuntimeRequirements> {
    let mut values = BTreeMap::<String, SimpleIrRuntimeRequirements>::new();
    let mut module_identities = BTreeMap::<String, BTreeSet<String>>::new();
    let constant_strings = function
        .ops
        .iter()
        .filter_map(|op| {
            (op.kind == "const_str")
                .then(|| Some((op.out.as_ref()?.clone(), op.s_value.as_ref()?.clone())))?
        })
        .collect::<BTreeMap<_, _>>();
    let mut call_requirements = vec![SimpleIrRuntimeRequirements::NONE; function.ops.len()];
    if function.ops.is_empty() {
        return call_requirements;
    }
    let cfg = CFG::build(&function.ops);
    let mut predecessors = cfg.predecessors.clone();
    let mut successors = cfg.successors.clone();
    for &(from, to) in &cfg.exception_edges {
        if !successors[from].contains(&to) {
            successors[from].push(to);
            predecessors[to].push(from);
        }
    }
    for &(from, to, _) in &cfg.state_resume_edges {
        if !successors[from].contains(&to) {
            successors[from].push(to);
            predecessors[to].push(from);
        }
    }
    let mut reachable = vec![false; cfg.blocks.len()];
    let mut reachability = VecDeque::from([cfg.entry]);
    while let Some(block) = reachability.pop_front() {
        if reachable[block] {
            continue;
        }
        reachable[block] = true;
        reachability.extend(successors[block].iter().copied());
    }
    let mut out_states = vec![CallableBindingState::default(); cfg.blocks.len()];
    let mut value_consumer_blocks = BTreeMap::<String, BTreeSet<usize>>::new();
    for block in &cfg.blocks {
        for op in &function.ops[block.start_op..block.end_op] {
            visit_simple_ir_reads(op, |read| {
                value_consumer_blocks
                    .entry(read.name.to_string())
                    .or_default()
                    .insert(block.id);
            });
        }
    }
    let mut queued = reachable.clone();
    let mut worklist = reachable
        .iter()
        .enumerate()
        .filter_map(|(block, is_reachable)| is_reachable.then_some(block))
        .collect::<VecDeque<_>>();

    while let Some(block_id) = worklist.pop_front() {
        queued[block_id] = false;
        let block = &cfg.blocks[block_id];
        let mut changed_values = BTreeSet::<String>::new();
        let mut state = merge_binding_states(
            predecessors[block_id]
                .iter()
                .map(|predecessor| &out_states[*predecessor]),
        );

        for index in block.start_op..block.end_op {
            let op = &function.ops[index];
            let mut produced = op
                .runtime_symbol
                .as_deref()
                .map(simpleir_runtime_symbol_requirements_table)
                .unwrap_or(SimpleIrRuntimeRequirements::NONE);

            if let Some(alias_role) = simpleir_module_identity_alias_role_table(op.kind.as_str()) {
                for arg in op.args.as_deref().unwrap_or_default() {
                    produced = produced.union(requirement_for_name(&values, arg));
                }
                if let Some(out) = op.out.as_deref() {
                    let mut incoming_identities = BTreeSet::new();
                    for arg in op.args.as_deref().unwrap_or_default() {
                        incoming_identities.extend(module_identities_for(&module_identities, arg));
                    }
                    let identities = module_identities.entry(out.to_string()).or_default();
                    let changed = match alias_role {
                        SimpleIrModuleIdentityAliasRole::Strong => {
                            if *identities == incoming_identities {
                                false
                            } else {
                                *identities = incoming_identities;
                                true
                            }
                        }
                        SimpleIrModuleIdentityAliasRole::Merge => {
                            if *identities == incoming_identities {
                                false
                            } else {
                                // Recompute the complete phi union from current
                                // source contributions. A provisional fallback
                                // is not a permanent may-alias fact once that
                                // source resolves to a canonical module.
                                *identities = incoming_identities;
                                true
                            }
                        }
                    };
                    if changed {
                        changed_values.insert(out.to_string());
                    }
                }
            }
            if let Some(module_name_arg) =
                simpleir_module_identity_source_name_arg(op.kind.as_str())
                && let Some(out) = op.out.as_deref()
                && let Some(module_name_value) = op
                    .args
                    .as_deref()
                    .and_then(|args| args.get(module_name_arg))
                && let Some(module_name) = constant_strings.get(module_name_value)
            {
                let identities = module_identities.entry(out.to_string()).or_default();
                if identities.insert(format!("module:{module_name}")) {
                    changed_values.insert(out.to_string());
                }
            }

            match simpleir_var_field_role_table(op.kind.as_str()) {
                SimpleIrVarFieldRole::Definition => {
                    if let Some(slot) = op.var.as_deref().or(op.out.as_deref()) {
                        let source = op.args.as_deref().and_then(|args| args.first());
                        let incoming = source
                            .map(|name| requirement_for_name(&values, name))
                            .unwrap_or(SimpleIrRuntimeRequirements::NONE);
                        replace_requirement(&mut state.locals, slot, incoming);
                        if let Some(source) = source {
                            state.local_modules.insert(
                                slot.to_string(),
                                module_identities_for(&module_identities, source),
                            );
                        } else {
                            state.local_modules.remove(slot);
                        }
                    }
                }
                SimpleIrVarFieldRole::MetadataWhenArgs => {
                    if let Some(source) = op.args.as_deref().and_then(|args| args.first()) {
                        produced = produced.union(requirement_for_name(&values, source));
                        if let Some(out) = op.out.as_deref() {
                            let incoming_identities =
                                module_identities_for(&module_identities, source);
                            let identities = module_identities.entry(out.to_string()).or_default();
                            let before = identities.len();
                            identities.extend(incoming_identities);
                            if identities.len() != before {
                                changed_values.insert(out.to_string());
                            }
                        }
                    } else if let Some(slot) = op.var.as_deref() {
                        produced = produced.union(requirement_for_name(&state.locals, slot));
                        if let Some(out) = op.out.as_deref()
                            && let Some(identities) = state.local_modules.get(slot)
                        {
                            let out_identities =
                                module_identities.entry(out.to_string()).or_default();
                            let before = out_identities.len();
                            out_identities.extend(identities.iter().cloned());
                            if out_identities.len() != before {
                                changed_values.insert(out.to_string());
                            }
                        }
                    }
                }
                SimpleIrVarFieldRole::Read | SimpleIrVarFieldRole::Result => {}
            }

            if let Some(access) = simpleir_module_slot_access_table(op.kind.as_str())
                && let Some(args) = op.args.as_deref()
                && let (Some(module_value), Some(name_value)) =
                    (args.get(access.module_arg), args.get(access.name_arg))
                && let Some(attribute) = constant_strings.get(name_value)
            {
                let identities = module_identities_for(&module_identities, module_value);
                match access.role {
                    SimpleIrModuleSlotRole::Set => {
                        let incoming = access
                            .value_arg
                            .and_then(|position| args.get(position))
                            .map(|value| requirement_for_name(&values, value))
                            .unwrap_or(SimpleIrRuntimeRequirements::NONE);
                        if identities.len() == 1 {
                            replace_module_requirement(
                                &mut state.modules,
                                (
                                    identities.into_iter().next().expect("singleton"),
                                    attribute.clone(),
                                ),
                                incoming,
                            );
                        } else if !incoming.is_empty() {
                            for module_identity in identities {
                                let key = (module_identity, attribute.clone());
                                let prior = state
                                    .modules
                                    .get(&key)
                                    .copied()
                                    .unwrap_or(SimpleIrRuntimeRequirements::NONE);
                                state.modules.insert(key, prior.union(incoming));
                            }
                        }
                    }
                    SimpleIrModuleSlotRole::Delete => {
                        if identities.len() == 1 {
                            state.modules.remove(&(
                                identities.into_iter().next().expect("singleton"),
                                attribute.clone(),
                            ));
                        }
                    }
                    SimpleIrModuleSlotRole::Get => {
                        for module_identity in identities {
                            if let Some(requirement) =
                                state.modules.get(&(module_identity, attribute.clone()))
                            {
                                produced = produced.union(*requirement);
                            }
                        }
                    }
                }
            }

            if let Some(out) = op.out.as_deref() {
                let before = requirement_for_name(&values, out);
                // SSA results have one defining transfer. Re-evaluation must
                // replace that result so a strong slot/module update can
                // remove a requirement learned from an earlier provisional
                // state; phi itself has already merged all incoming operands.
                replace_requirement(&mut values, out, produced);
                if before != requirement_for_name(&values, out) {
                    changed_values.insert(out.to_string());
                }
            }

            if simpleir_kind_has_callable_operand(op.kind.as_str())
                && op.s_value.is_none()
                && let Some(callable) = op.args.as_deref().and_then(|args| args.first())
            {
                let incoming = requirement_for_name(&values, callable);
                // This is the transfer result for the current fixed-point
                // state, not a monotone fact of its own. Strong local/module
                // updates may remove a provisional requirement on revisit.
                call_requirements[index] = incoming;
            }
        }

        if out_states[block_id] != state {
            out_states[block_id] = state;
            for successor in &successors[block_id] {
                if reachable[*successor] && !queued[*successor] {
                    queued[*successor] = true;
                    worklist.push_back(*successor);
                }
            }
        }
        for value in changed_values {
            if let Some(consumers) = value_consumer_blocks.get(&value) {
                for candidate in consumers {
                    if reachable[*candidate] && !queued[*candidate] {
                        queued[*candidate] = true;
                        worklist.push_back(*candidate);
                    }
                }
            }
        }
    }

    call_requirements
}

/// Compose kind and canonical runtime-call metadata through one generated
/// requirement authority. Runtime symbols may arrive as builtin metadata,
/// `call_internal.s_value`, or the first operand of an invocation op.
pub fn simpleir_op_runtime_requirements(op: &OpIR) -> Option<SimpleIrRuntimeRequirements> {
    let mut requirements = simpleir_runtime_requirements_table(op.kind.as_str())?;
    if let Some(symbol) = op.builtin_name.as_deref() {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    if matches!(op.kind.as_str(), "call_internal" | "builtin_func")
        && let Some(symbol) = op.s_value.as_deref()
    {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    if simpleir_kind_has_callable_operand(op.kind.as_str())
        && op.s_value.is_none()
        && let Some(symbol) = op.args.as_deref().and_then(|args| args.first())
    {
        requirements = requirements.union(simpleir_runtime_symbol_requirements_table(symbol));
    }
    Some(requirements)
}

fn runtime_admission_checks(
    capabilities: RuntimeTargetCapabilities,
) -> [(SimpleIrRuntimeRequirements, bool, &'static str); 15] {
    [
        (
            SimpleIrRuntimeRequirements::EXECUTION_FRAME,
            capabilities.execution_frame_state,
            "operation requires internal execution-frame stack and source-location custody",
        ),
        (
            SimpleIrRuntimeRequirements::FRAME_INTROSPECTION,
            capabilities.python_frame_introspection,
            "operation requires exact Python-visible frame objects, locals, globals, and tracing state",
        ),
        (
            SimpleIrRuntimeRequirements::IDENTITY,
            capabilities.python_identity,
            "operation requires Python object identity rather than value equality",
        ),
        (
            SimpleIrRuntimeRequirements::TUPLE,
            capabilities.tuple_representation,
            "operation requires a tuple representation distinct from mutable lists",
        ),
        (
            SimpleIrRuntimeRequirements::EXCEPTION,
            capabilities.exception_model,
            "operation requires Python exception state, matching, and structured unwinding",
        ),
        (
            SimpleIrRuntimeRequirements::DETERMINISTIC_LIFETIME,
            capabilities.deterministic_lifetime,
            "operation requires deterministic Python lifetime/finalizer semantics",
        ),
        (
            SimpleIrRuntimeRequirements::FORMAT_PROTOCOL,
            capabilities.format_protocol,
            "operation requires the Python __format__ protocol and conversion semantics",
        ),
        (
            SimpleIrRuntimeRequirements::ITERABLE_PROTOCOL,
            capabilities.iterable_protocol,
            "operation requires the Python iterable/sequence protocol and exact errors",
        ),
        (
            SimpleIrRuntimeRequirements::OBJECT_MODEL,
            capabilities.object_model,
            "operation requires Python aliasing, cycles, None storage, hashing, and object protocols",
        ),
        (
            SimpleIrRuntimeRequirements::TRUTHINESS,
            capabilities.python_truthiness,
            "operation requires CPython truthiness across NaN and dynamic containers",
        ),
        (
            SimpleIrRuntimeRequirements::COMPARISON,
            capabilities.python_comparison,
            "operation requires CPython comparison dispatch, NaN behavior, and exact integers",
        ),
        (
            SimpleIrRuntimeRequirements::FALLIBLE_PROTOCOL,
            capabilities.structured_runtime_errors,
            "operation can raise and requires structured catchable Python exceptions",
        ),
        (
            SimpleIrRuntimeRequirements::ASYNC_RUNTIME,
            capabilities.async_runtime,
            "operation requires an exact async scheduler and suspension-state model",
        ),
        (
            SimpleIrRuntimeRequirements::UNSTRUCTURED_CONTROL,
            capabilities.unstructured_control_flow,
            "operation requires a target-proven unstructured control-flow lowering",
        ),
        (
            SimpleIrRuntimeRequirements::HOST_CAPABILITY,
            capabilities.host_capabilities,
            "operation requires a target host filesystem or foreign-function capability",
        ),
    ]
}

fn numeric_admission_failure(
    plan: &ScalarRepresentationPlan,
    op: &OpIR,
    role: SimpleIrIntegerSemantics,
    capabilities: NumericTargetCapabilities,
) -> Option<&'static str> {
    use SimpleIrIntegerSemantics as Role;

    match role {
        Role::None => None,
        Role::IntegerLiteral => {
            if !op_has_integer_literal_payload(op)
                || capabilities.arbitrary_precision_integers
                || capabilities
                    .exact_integer_literal_max_magnitude
                    .is_some_and(|max| exact_integer_literal_value(op, max).is_some())
            {
                None
            } else {
                Some(
                    "integer literal exceeds the target's exact concrete value authority and requires the canonical arbitrary-precision value authority",
                )
            }
        }
        Role::IntegerOnly | Role::IntegerProducer => (!capabilities.arbitrary_precision_integers)
            .then_some(
            "Python integer semantics require the canonical arbitrary-precision value authority",
        ),
        Role::DynamicAdd => {
            if capabilities.arbitrary_precision_integers
                || operands_are_float(plan, op, 2)
                || operands_are_strings(plan, op, 2)
            {
                None
            } else {
                Some(
                    "operands do not prove a non-integer add domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicNumeric | Role::DynamicTrueDiv => {
            if capabilities.arbitrary_precision_integers || operands_are_float(plan, op, 2) {
                None
            } else {
                Some(
                    "operands do not prove a float-only domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicUnaryNumeric => {
            if capabilities.arbitrary_precision_integers || operands_are_float(plan, op, 1) {
                None
            } else {
                Some(
                    "operand does not prove a float-only domain and the target lacks arbitrary-precision integers",
                )
            }
        }
        Role::DynamicDivmod => {
            if operands_are_float(plan, op, 2) {
                (!capabilities.cpython_float_divmod).then_some(
                    "float // and % require a CPython-exact signed-zero and rounding authority",
                )
            } else if operands_are_integer(plan, op, 2) {
                (!capabilities.arbitrary_precision_integers).then_some(
                    "integer // and % require the canonical arbitrary-precision value authority",
                )
            } else if capabilities.arbitrary_precision_integers && capabilities.cpython_float_divmod
            {
                None
            } else {
                Some(
                    "dynamic // and % require both arbitrary-precision integers and CPython-exact float divmod",
                )
            }
        }
        Role::DynamicPower => {
            if operands_are_float(plan, op, 2) {
                (!capabilities.cpython_power).then_some(
                    "pow requires CPython negative-base, fractional-exponent, and complex-result semantics",
                )
            } else if operands_are_integer(plan, op, 2) {
                (!capabilities.arbitrary_precision_integers).then_some(
                    "integer pow requires the canonical arbitrary-precision value authority",
                )
            } else if capabilities.arbitrary_precision_integers && capabilities.cpython_power {
                None
            } else {
                Some(
                    "dynamic pow requires arbitrary-precision integers and CPython complex-result semantics",
                )
            }
        }
    }
}

fn op_has_integer_literal_payload(op: &OpIR) -> bool {
    match op.kind.as_str() {
        "const" => op.value.is_some(),
        "const_int" | "const_bigint" => true,
        _ => false,
    }
}

/// Return a canonical concrete integer literal when its complete decimal value
/// fits the target's exact carrier. This is shared by target admission and
/// bounded-value emitters so a backend cannot admit one range and materialize
/// another.
pub fn exact_integer_literal_value(op: &OpIR, max_magnitude: u128) -> Option<i128> {
    let value = match op.kind.as_str() {
        "const" | "const_int" => i128::from(op.value?),
        "const_bigint" => op.s_value.as_deref()?.parse::<i128>().ok()?,
        _ => return None,
    };
    (value.unsigned_abs() <= max_magnitude).then_some(value)
}

fn operands(op: &OpIR, arity: usize) -> Option<&[String]> {
    let args = op.args.as_deref()?;
    (args.len() == arity).then_some(args)
}

fn operands_are_float(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity).is_some_and(|args| args.iter().all(|name| plan.name_is_float_scalar(name)))
}

fn operands_are_integer(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity)
        .is_some_and(|args| args.iter().all(|name| plan.name_is_integer_family(name)))
}

fn operands_are_strings(plan: &ScalarRepresentationPlan, op: &OpIR, arity: usize) -> bool {
    operands(op, arity).is_some_and(|args| args.iter().all(|name| plan.name_is_str_scalar(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionIR;

    fn binary(kind: &str, ty: &str) -> SimpleIR {
        SimpleIR {
            functions: vec![FunctionIR {
                name: "f".to_string(),
                params: vec!["lhs".to_string(), "rhs".to_string()],
                ops: vec![OpIR {
                    kind: kind.to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                }],
                param_types: Some(vec![ty.to_string(), ty.to_string()]),
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        }
    }

    fn runtime_without_frame_introspection() -> RuntimeTargetCapabilities {
        RuntimeTargetCapabilities {
            execution_frame_state: true,
            python_frame_introspection: false,
            python_identity: true,
            tuple_representation: true,
            exception_model: true,
            deterministic_lifetime: true,
            format_protocol: true,
            iterable_protocol: true,
            object_model: true,
            python_truthiness: true,
            python_comparison: true,
            structured_runtime_errors: true,
            async_runtime: true,
            unstructured_control_flow: true,
            host_capabilities: true,
        }
    }

    fn function_ir(ops: Vec<OpIR>) -> SimpleIR {
        SimpleIR {
            functions: vec![FunctionIR {
                name: "callable_flow".to_string(),
                params: vec![],
                ops,
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        }
    }

    fn frame_callable(out: &str) -> OpIR {
        OpIR {
            kind: "module_get_attr".to_string(),
            runtime_symbol: Some("molt_getframe".to_string()),
            args: Some(vec!["sys_module".to_string(), "frame_attr".to_string()]),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn call_value(kind: &str, callable: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(vec![callable.to_string(), "callargs".to_string()]),
            out: Some("call_result".to_string()),
            ..OpIR::default()
        }
    }

    #[test]
    fn fixed_width_targets_admit_exact_float_basics_only() {
        validate_numeric_target_contract(
            &binary("add", "float"),
            "test",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect("float add is exact in the target policy");

        for kind in ["pow", "floor_div", "mod"] {
            let error = validate_numeric_target_contract(
                &binary(kind, "float"),
                "test",
                NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
            )
            .expect_err("non-exact float semantics must reject");
            assert!(error.contains("rejected before source generation"));
        }
    }

    #[test]
    fn fixed_width_targets_reject_integer_arithmetic() {
        let error = validate_numeric_target_contract(
            &binary("add", "int"),
            "test",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect_err("i64 is not Python integer semantics");
        assert!(error.contains("arbitrary-precision"));
    }

    #[test]
    fn exact_literal_capability_admits_only_concrete_in_range_siblings() {
        let capabilities = NumericTargetCapabilities::LUAU_EXACT_INTEGER_LITERALS;
        for op in [
            OpIR {
                kind: "const".to_string(),
                value: Some(42),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".to_string(),
                value: Some(-(1_i64 << 53)),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_bigint".to_string(),
                s_value: Some((1_u64 << 53).to_string()),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
        ] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "f".to_string(),
                    params: vec![],
                    ops: vec![op],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    execution_context: Default::default(),
                }],
                profile: None,
            };
            validate_numeric_target_contract(&ir, "luau", capabilities)
                .expect("exact concrete literal must be admitted");
        }

        for payload in ["9007199254740993", "-9007199254740993", "not-an-int"] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "f".to_string(),
                    params: vec![],
                    ops: vec![OpIR {
                        kind: "const_bigint".to_string(),
                        s_value: Some(payload.to_string()),
                        out: Some("out".to_string()),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    execution_context: Default::default(),
                }],
                profile: None,
            };
            let error = validate_numeric_target_contract(&ir, "luau", capabilities)
                .expect_err("unsafe or malformed bigint literal must reject");
            assert!(error.contains("exact concrete value authority"));
        }
    }

    #[test]
    fn generic_const_non_integer_payload_is_not_an_integer_literal() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "f".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "const".to_string(),
                    f_value: Some(1.25),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        };
        validate_numeric_target_contract(
            &ir,
            "fixed",
            NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
        )
        .expect("generic const float payload must stay outside integer admission");
    }

    #[test]
    fn execution_frames_are_distinct_from_python_introspection() {
        for kind in ["frame_locals_set", "line", "trace_enter_slot", "trace_exit"] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "frame_state".to_string(),
                    params: vec!["locals".to_string()],
                    ops: vec![OpIR {
                        kind: kind.to_string(),
                        args: (kind == "frame_locals_set").then(|| vec!["locals".to_string()]),
                        value: matches!(kind, "line" | "trace_enter_slot").then_some(7),
                        ..OpIR::default()
                    }],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    execution_context: Default::default(),
                }],
                profile: None,
            };

            let error =
                validate_runtime_target_contract(&ir, "no-frames", RuntimeTargetCapabilities::NONE)
                    .expect_err("observable frame state must not degrade to a target no-op");
            assert!(error.contains("execution-frame stack and source-location"));

            validate_runtime_target_contract(
                &ir,
                "frames",
                RuntimeTargetCapabilities {
                    execution_frame_state: true,
                    ..RuntimeTargetCapabilities::NONE
                },
            )
            .expect("the execution-frame capability admits its full sibling family");
        }

        let getframe_ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "observe".to_string(),
                params: vec!["depth".to_string()],
                ops: vec![OpIR {
                    kind: "getframe".to_string(),
                    args: Some(vec!["depth".to_string()]),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        };
        let error = validate_runtime_target_contract(
            &getframe_ir,
            "execution-only",
            RuntimeTargetCapabilities {
                execution_frame_state: true,
                ..RuntimeTargetCapabilities::NONE
            },
        )
        .expect_err("execution frames must not imply Python-visible frame objects");
        assert!(error.contains("exact Python-visible frame objects"));
    }

    #[test]
    fn canonical_runtime_symbol_fields_share_frame_introspection_admission() {
        for symbol in [
            "molt_getframe",
            "molt_inspect_currentframe",
            "molt_sys_settrace",
            "molt_sys_gettrace",
            "molt_sys_setprofile",
            "molt_sys_getprofile",
        ] {
            for op in [
                OpIR {
                    kind: "call_internal".to_string(),
                    s_value: Some(symbol.to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_func".to_string(),
                    builtin_name: Some(symbol.to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_func".to_string(),
                    args: Some(vec![symbol.to_string()]),
                    ..OpIR::default()
                },
            ] {
                let requirements = simpleir_op_runtime_requirements(&op)
                    .expect("runtime-call op must be classified");
                assert!(
                    requirements.contains(SimpleIrRuntimeRequirements::FRAME_INTROSPECTION),
                    "{symbol} via {}",
                    op.kind
                );
            }
        }
    }

    #[test]
    fn canonical_callable_provenance_reaches_indirect_call_bind_through_aliases() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "aliased_frame_call".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        runtime_symbol: Some("molt_getframe".to_string()),
                        args: Some(vec!["sys_module".to_string(), "attr".to_string()]),
                        out: Some("frame_func".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "copy".to_string(),
                        args: Some(vec!["frame_func".to_string()]),
                        out: Some("alias".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store_var".to_string(),
                        var: Some("callable_slot".to_string()),
                        args: Some(vec!["alias".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "load_var".to_string(),
                        var: Some("callable_slot".to_string()),
                        out: Some("loaded".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_bind".to_string(),
                        args: Some(vec!["loaded".to_string(), "callargs".to_string()]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        };

        let error = validate_runtime_target_contract(
            &ir,
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("an indirect aliased frame call must reject before source generation");
        assert!(error.contains("op#4 `call_bind`"), "{error}");
        assert!(
            error.contains("exact Python-visible frame objects"),
            "{error}"
        );
    }

    #[test]
    fn callable_slots_replace_on_safe_store_and_delete_but_union_at_cfg_joins() {
        for tail in [
            vec![
                OpIR {
                    kind: "store_var".into(),
                    var: Some("slot".into()),
                    args: Some(vec!["safe".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".into(),
                    var: Some("slot".into()),
                    out: Some("loaded".into()),
                    ..OpIR::default()
                },
                call_value("call_indirect", "loaded"),
            ],
            vec![
                OpIR {
                    kind: "delete_var".into(),
                    var: Some("slot".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".into(),
                    var: Some("slot".into()),
                    out: Some("loaded".into()),
                    ..OpIR::default()
                },
                call_value("call_indirect", "loaded"),
            ],
        ] {
            let mut ops = vec![
                frame_callable("frame_func"),
                OpIR {
                    kind: "const_none".into(),
                    out: Some("safe".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".into(),
                    var: Some("slot".into()),
                    args: Some(vec!["frame_func".into()]),
                    ..OpIR::default()
                },
            ];
            ops.extend(tail);
            validate_runtime_target_contract(
                &function_ir(ops),
                "execution-only",
                runtime_without_frame_introspection(),
            )
            .expect("definite safe overwrite/delete must clear callable provenance");
        }

        let joined = function_ir(vec![
            frame_callable("frame_func"),
            OpIR {
                kind: "const_none".into(),
                out: Some("safe".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "if".into(),
                args: Some(vec!["condition".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                args: Some(vec!["frame_func".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "else".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                args: Some(vec!["safe".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "end_if".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".into(),
                var: Some("slot".into()),
                out: Some("joined".into()),
                ..OpIR::default()
            },
            call_value("call_bind", "joined"),
        ]);
        let error = validate_runtime_target_contract(
            &joined,
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("safe/frame branch join must retain frame provenance");
        assert!(error.contains("call_bind"), "{error}");
    }

    #[test]
    fn callable_provenance_covers_phi_loops_modules_and_every_indirect_spelling() {
        for kind in [
            "call_func",
            "call_function",
            "call_guarded",
            "call_bind",
            "call_indirect",
        ] {
            assert!(simpleir_kind_has_callable_operand(kind));
            let error = validate_runtime_target_contract(
                &function_ir(vec![
                    frame_callable("frame_func"),
                    call_value(kind, "frame_func"),
                ]),
                "execution-only",
                runtime_without_frame_introspection(),
            )
            .expect_err("every generated callable-operand spelling must reject");
            assert!(error.contains(kind), "{kind}: {error}");
        }

        let loop_carried = function_ir(vec![
            frame_callable("frame_func"),
            OpIR {
                kind: "const_none".into(),
                out: Some("safe".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                args: Some(vec!["safe".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("slot".into()),
                args: Some(vec!["frame_func".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".into(),
                var: Some("slot".into()),
                out: Some("loop_value".into()),
                ..OpIR::default()
            },
            call_value("call_indirect", "loop_value"),
        ]);
        validate_runtime_target_contract(
            &loop_carried,
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("loop-carried prohibited callable must reach the exit call");

        let module_alias = function_ir(vec![
            frame_callable("frame_func"),
            OpIR {
                kind: "const_str".into(),
                s_value: Some("alias".into()),
                out: Some("alias_name".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "module".into(),
                    "alias_name".into(),
                    "frame_func".into(),
                ]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_global".into(),
                args: Some(vec!["module".into(), "alias_name".into()]),
                out: Some("module_alias".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".into(),
                args: Some(vec!["module_alias".into(), "safe_unknown".into()]),
                out: Some("phi_alias".into()),
                ..OpIR::default()
            },
            call_value("call_function", "phi_alias"),
        ]);
        validate_runtime_target_contract(
            &module_alias,
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("module global and phi aliases must retain prohibited provenance");
    }

    #[test]
    fn callable_admission_folds_exception_and_state_resume_edges() {
        for ops in [
            vec![
                OpIR {
                    kind: "async_work_poll".into(),
                    value: Some(41),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".into(),
                    value: Some(41),
                    ..OpIR::default()
                },
                frame_callable("handler_frame"),
                call_value("call_indirect", "handler_frame"),
            ],
            vec![
                OpIR {
                    kind: "state_switch".into(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_yield".into(),
                    value: Some(7),
                    ..OpIR::default()
                },
                frame_callable("resumed_frame"),
                call_value("call_indirect", "resumed_frame"),
            ],
        ] {
            let error = validate_runtime_target_contract(
                &function_ir(ops),
                "execution-only",
                runtime_without_frame_introspection(),
            )
            .expect_err("handler/resume-only callable use must be admitted as executable");
            assert!(
                error.contains("exact Python-visible frame objects"),
                "{error}"
            );
        }
    }

    #[test]
    fn module_identity_aliases_share_replace_and_delete_semantics() {
        let prefix = || {
            vec![
                frame_callable("frame_func"),
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("sample".into()),
                    out: Some("module_name".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("alias".into()),
                    out: Some("alias_name".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".into(),
                    args: Some(vec!["module_name".into()]),
                    out: Some("module_a".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy".into(),
                    args: Some(vec!["module_a".into()]),
                    out: Some("module_alias".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".into(),
                    args: Some(vec![
                        "module_alias".into(),
                        "alias_name".into(),
                        "frame_func".into(),
                    ]),
                    ..OpIR::default()
                },
            ]
        };
        let read_and_call = || {
            vec![
                OpIR {
                    kind: "module_cache_get".into(),
                    args: Some(vec!["module_name".into()]),
                    out: Some("module_b".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_get_global".into(),
                    args: Some(vec!["module_b".into(), "alias_name".into()]),
                    out: Some("loaded".into()),
                    ..OpIR::default()
                },
                call_value("call_indirect", "loaded"),
            ]
        };

        let mut prohibited = prefix();
        prohibited.extend(read_and_call());
        validate_runtime_target_contract(
            &function_ir(prohibited),
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("module cache/copy aliases must share callable provenance");

        for clearing_op in [
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "module_a".into(),
                    "alias_name".into(),
                    "safe_unknown".into(),
                ]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_del_global".into(),
                args: Some(vec!["module_a".into(), "alias_name".into()]),
                ..OpIR::default()
            },
        ] {
            let mut cleared = prefix();
            cleared.push(clearing_op);
            cleared.extend(read_and_call());
            validate_runtime_target_contract(
                &function_ir(cleared),
                "execution-only",
                runtime_without_frame_introspection(),
            )
            .expect("definite module overwrite/delete must clear stale provenance");
        }

        let mut local_alias = prefix();
        local_alias.extend([
            OpIR {
                kind: "store_var".into(),
                var: Some("module_slot".into()),
                args: Some(vec!["module_a".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".into(),
                var: Some("module_slot".into()),
                out: Some("module_local_alias".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_global".into(),
                args: Some(vec!["module_local_alias".into(), "alias_name".into()]),
                out: Some("local_loaded".into()),
                ..OpIR::default()
            },
            call_value("call_indirect", "local_loaded"),
        ]);
        validate_runtime_target_contract(
            &function_ir(local_alias),
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect_err("module identity must survive generated local store/load roles");

        for uncertain_mutation in ["module_set_attr", "module_del_global"] {
            let mut uncertain = vec![
                frame_callable("frame_func"),
                OpIR {
                    kind: "const_none".into(),
                    out: Some("safe".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("a".into()),
                    out: Some("a_name".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("b".into()),
                    out: Some("b_name".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".into(),
                    s_value: Some("alias".into()),
                    out: Some("attr".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".into(),
                    args: Some(vec!["a_name".into()]),
                    out: Some("module_a".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".into(),
                    args: Some(vec!["b_name".into()]),
                    out: Some("module_b".into()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".into(),
                    args: Some(vec!["module_b".into(), "attr".into(), "frame_func".into()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "phi".into(),
                    args: Some(vec!["module_a".into(), "module_b".into()]),
                    out: Some("maybe_module".into()),
                    ..OpIR::default()
                },
            ];
            uncertain.push(if uncertain_mutation == "module_set_attr" {
                OpIR {
                    kind: uncertain_mutation.into(),
                    args: Some(vec!["maybe_module".into(), "attr".into(), "safe".into()]),
                    ..OpIR::default()
                }
            } else {
                OpIR {
                    kind: uncertain_mutation.into(),
                    args: Some(vec!["maybe_module".into(), "attr".into()]),
                    ..OpIR::default()
                }
            });
            uncertain.extend([
                OpIR {
                    kind: "module_get_global".into(),
                    args: Some(vec!["module_b".into(), "attr".into()]),
                    out: Some("still_prohibited".into()),
                    ..OpIR::default()
                },
                call_value("call_indirect", "still_prohibited"),
            ]);
            validate_runtime_target_contract(
                &function_ir(uncertain),
                "execution-only",
                runtime_without_frame_introspection(),
            )
            .expect_err("may-alias mutation cannot strongly clear every possible module");
        }
    }

    #[test]
    fn strong_module_copy_replaces_provisional_identity_after_out_of_order_resolution() {
        let ops = vec![
            frame_callable("frame_func"),
            OpIR {
                kind: "const_str".into(),
                s_value: Some("sample".into()),
                out: Some("module_name".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("alias".into()),
                out: Some("attr".into()),
                ..OpIR::default()
            },
            // Deliberately precede the defining import. The worklist must
            // revisit this consumer and replace its provisional value identity
            // once the canonical module identity becomes available.
            OpIR {
                kind: "copy".into(),
                args: Some(vec!["late_module".into()]),
                out: Some("module_alias".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".into(),
                args: Some(vec!["module_name".into()]),
                out: Some("late_module".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "late_module".into(),
                    "attr".into(),
                    "frame_func".into(),
                ]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "module_alias".into(),
                    "attr".into(),
                    "safe_unknown".into(),
                ]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_global".into(),
                args: Some(vec!["late_module".into(), "attr".into()]),
                out: Some("loaded".into()),
                ..OpIR::default()
            },
            call_value("call_indirect", "loaded"),
        ];

        validate_runtime_target_contract(
            &function_ir(ops),
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect("a resolved strong copy must recover definite strong-update semantics");
    }

    #[test]
    fn module_phi_recomputes_out_of_order_sources_without_provisional_poison() {
        let ops = vec![
            frame_callable("frame_func"),
            OpIR {
                kind: "const_str".into(),
                s_value: Some("sample".into()),
                out: Some("module_name".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_str".into(),
                s_value: Some("alias".into()),
                out: Some("attr".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "phi".into(),
                args: Some(vec!["late_a".into(), "late_b".into()]),
                out: Some("joined_module".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".into(),
                args: Some(vec!["module_name".into()]),
                out: Some("late_a".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_cache_get".into(),
                args: Some(vec!["module_name".into()]),
                out: Some("late_b".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec!["late_a".into(), "attr".into(), "frame_func".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_set_attr".into(),
                args: Some(vec![
                    "joined_module".into(),
                    "attr".into(),
                    "safe_unknown".into(),
                ]),
                ..OpIR::default()
            },
            OpIR {
                kind: "module_get_global".into(),
                args: Some(vec!["late_b".into(), "attr".into()]),
                out: Some("loaded".into()),
                ..OpIR::default()
            },
            call_value("call_indirect", "loaded"),
        ];

        validate_runtime_target_contract(
            &function_ir(ops),
            "execution-only",
            runtime_without_frame_introspection(),
        )
        .expect("resolved phi sources for one module must shed provisional may-alias identities");
    }
}
