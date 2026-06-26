//! Whole-program **call graph** over a [`TirModule`] (Tier-0 substrate **S4**).
//!
//! This is the foundational analysis of the interprocedural tier. Before it
//! existed, the only call graph in the compiler was the SimpleIR
//! dead-function-elimination BFS (`passes::eliminate_dead_functions`, the
//! algorithmic template this module follows) and an ad-hoc per-function "has no
//! call op" leaf scan in the native backend
//! (`native_backend::simple_backend::analyze_native_backend_functions`). Neither
//! is reusable by the TIR pipeline, and neither exposes SCC / bottom-up order /
//! a recursive set — the structure the inliner (E1), IP-escape (E3), IPSCCP
//! (E4) and monomorphization (E5) all require.
//!
//! The [`CallGraph`] is built once per module by
//! [`crate::tir::module_phase::run_module_pipeline`], before the per-function
//! [`crate::tir::pass_manager::PassManager`] pipeline runs (via
//! [`crate::tir::parallel::compile_module_parallel`]). It is a *read-only*
//! analysis of the lifted [`TirModule`] — it does not mutate the functions.
//!
//! ## Callee resolution: static-direct vs dynamic/opaque
//!
//! Every call-bearing op in a function body produces a [`CallEdge`]:
//!
//! * **[`CallEdge::StaticDirect`]** — an [`OpCode::Call`] whose `s_value` attr is
//!   `Str(name)` AND `name` is a function defined in this module. The target is
//!   known; this is the edge the inliner can act on.
//! * **[`CallEdge::Opaque`]** — every other call: a [`OpCode::Call`] with no
//!   `s_value` (indirect / computed callee), a `Str(name)` that names a function
//!   *not* in this module (extern / cross-batch), any dynamic-method opcode
//!   such as [`OpCode::CallMethod`] / [`OpCode::CallMethodIc`] /
//!   [`OpCode::CallSuperMethodIc`] (Python dynamic method dispatch), or a `Copy`
//!   op carrying a call-kind
//!   `_original_kind` (the SSA-lift fallback spelling of `call_func` /
//!   `call_indirect` / `call_bind` / `invoke_ffi` etc. — see
//!   [`crate::tir::ssa`]). An opaque call may reach *any* function (including
//!   back into this one), so it is conservatively a recursion-capable edge.
//!
//! [`OpCode::CallBuiltin`] is deliberately **not** a call edge: it lowers to a
//! direct runtime-helper call (`range`, `print`, `bool`, …), never to a
//! user-defined Python function, and the legacy SimpleIR leaf scan likewise does
//! not treat `call_builtin` as a user-level call. Treating it as an edge would
//! make the [`CallGraph::leaf_functions`] set a strict *subset* of the legacy
//! one — under-claiming leaves (a missed optimization), but more importantly it
//! would diverge from the behavioral baseline this S4 arc must preserve.
//!
//! ## Generator poll edges
//!
//! An [`OpCode::AllocTask`] whose `s_value` is `Str(poll_name)` references the
//! generator/coroutine *poll* function (`"{base}_poll"`). That is a genuine
//! callable reference (the runtime drives the task by calling its poll fn), so
//! it produces a [`CallEdge`] to `poll_name` when that function is in the module
//! — mirroring `eliminate_dead_functions`' `alloc_task` handling. It does *not*
//! disqualify the *enclosing* function from leaf-ness (allocating a task is not
//! itself a call), so `AllocTask` edges are recorded for SCC/reachability but
//! are excluded from the "makes no call" leaf test.
//!
//! ## Soundness contract (the leaf-set replacement)
//!
//! The native backend skips the recursion guard at a call site whose *callee* is
//! a [`CallGraph::leaf_functions`] member. A callee that "makes no call of its
//! own" cannot start or extend a recursion cycle through itself, so skipping the
//! guard is sound. [`CallGraph::leaf_functions`] therefore returns exactly the
//! functions with **no outgoing static-or-opaque call edge** — which is exactly
//! what the legacy SimpleIR scan computed ("contains no `call` / `call_method` /
//! … op"). The TIR graph is strictly *more precise* (TIR DCE / devirt may have
//! removed a call the raw SimpleIR still carried), so the TIR leaf set is a
//! superset of the legacy one: it may mark *more* functions leaf, never *fewer*,
//! and never marks a function leaf that actually retains a call. See the
//! `leaf_*` unit tests.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::function::TirModule;
use super::op_kinds_generated::{
    CallOpcodeRole, opcode_call_role_table, simpleir_kind_is_call_graph_user_call,
};
use super::ops::{AttrValue, OpCode, TirOp};

/// A resolved or unresolved call target referenced by one call-bearing op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEdge {
    /// A statically-resolved direct call to a function defined in this module.
    /// The inliner / IPSCCP / IP-escape act on exactly these edges.
    StaticDirect(String),
    /// A call whose concrete target is unknown to this module: an indirect /
    /// computed callee, a method dispatch, or a named callee that is extern to
    /// this module (e.g. compiled in another batch). Conservatively treated as
    /// reaching any function — so it is recursion-capable and blocks the
    /// `does-not-recurse` leaf property of its *enclosing* function.
    Opaque,
}

/// Whether a [`TirOp`] is a call for the purposes of the call graph, and if so,
/// how its callee resolves against the module's function set.
///
/// Returns `None` for non-call ops. The `defined` set is the names of functions
/// present in this [`TirModule`]; a named callee outside it resolves to
/// [`CallEdge::Opaque`] (extern / cross-batch).
fn classify_call_op(op: &TirOp, defined: &BTreeSet<String>) -> Option<CallEdge> {
    /// Read an op's `s_value` string attr, if present.
    fn s_value(op: &TirOp) -> Option<&str> {
        match op.attrs.get("s_value") {
            Some(AttrValue::Str(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    match opcode_call_role_table(op.opcode) {
        // A first-class `Call`: static-direct when its `s_value` names a defined
        // function, otherwise opaque (indirect, or a callee extern to this
        // module). The SSA lift folds `call`, `call_func`, `call_internal`,
        // `call_indirect`, `call_bind`, `call_guarded`, `invoke_ffi` all into
        // `OpCode::Call`, so this single arm covers every user-level direct-call
        // spelling.
        //
        // EXCEPTION: the gpu_* intrinsics (`gpu_thread_id`, `gpu_barrier`, …)
        // also lift to `OpCode::Call`, but with a fixed `molt_gpu_*` runtime
        // symbol as `s_value`. Those are runtime-helper calls (like
        // `CallBuiltin`), never user Python functions, and the legacy SimpleIR
        // leaf scan never listed their op kinds as user-level calls. Treating
        // them as a call edge would spuriously disqualify a gpu-kernel leaf from
        // leaf-ness and change codegen, so they are NOT edges.
        CallOpcodeRole::UserCall => {
            if matches!(s_value(op), Some(s) if is_gpu_runtime_symbol(s)) {
                None
            } else {
                Some(match s_value(op) {
                    Some(name) if defined.contains(name) => {
                        CallEdge::StaticDirect(name.to_string())
                    }
                    _ => CallEdge::Opaque,
                })
            }
        }
        // Python method dispatch is always dynamic — the receiver's runtime type
        // selects the implementation. Never statically resolvable here.
        CallOpcodeRole::DynamicMethod => Some(CallEdge::Opaque),
        // The SSA-lift fallback: a `Copy` op carrying a call-kind
        // `_original_kind` is a disguised call the lift had no first-class
        // opcode for. It lowers back to a real SimpleIR call (`lower_to_simple`),
        // so it MUST count as a call edge — missing it would mark a function
        // that actually calls as a leaf (an unsound recursion-guard skip).
        CallOpcodeRole::CopyOriginalKind => match op.attrs.get("_original_kind") {
            Some(AttrValue::Str(kind)) if simpleir_kind_is_call_graph_user_call(kind) => {
                match s_value(op) {
                    Some(name) if defined.contains(name) => {
                        Some(CallEdge::StaticDirect(name.to_string()))
                    }
                    _ => Some(CallEdge::Opaque),
                }
            }
            _ => None,
        },
        // `CallBuiltin` lowers to a runtime-helper call, never to a user Python
        // function (the legacy SimpleIR leaf scan likewise ignores
        // `call_builtin`); `AllocTask` is handled separately as a poll-fn
        // reference, not as a call out of the enclosing function.
        CallOpcodeRole::RuntimeBuiltin | CallOpcodeRole::NotCall => None,
    }
}

/// The fixed runtime-intrinsic symbols the gpu_* SimpleIR ops carry as their
/// `s_value` after the SSA lift folds them into `OpCode::Call` (see
/// `tir::ssa::gpu_runtime_symbol_for_simple_kind`). A `Call` to one of these is
/// a runtime-helper call, not a user-level call edge, so it must not disqualify
/// leaf-ness — matching the legacy SimpleIR leaf scan, which never treated the
/// gpu_* op kinds as user-level calls.
fn is_gpu_runtime_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "molt_gpu_thread_id"
            | "molt_gpu_block_id"
            | "molt_gpu_block_dim"
            | "molt_gpu_grid_dim"
            | "molt_gpu_barrier"
    )
}

/// If `op` is an [`OpCode::AllocTask`] referencing a poll function defined in
/// this module, return that poll function's name. This is a callable reference
/// (the runtime drives the task by calling its poll fn) but not a call *out of*
/// the enclosing function, so it feeds reachability/SCC but not the leaf test.
fn alloc_task_poll_target(op: &TirOp, defined: &BTreeSet<String>) -> Option<String> {
    if op.opcode != OpCode::AllocTask {
        return None;
    }
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(name)) if defined.contains(name) => Some(name.clone()),
        _ => None,
    }
}

/// The whole-program call graph of a [`TirModule`].
///
/// Built once by the module phase and shared (read-only) for the duration of
/// module compilation. Function identity is the function name (the same key the
/// SimpleIR DFE BFS and the native module context use).
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    /// Functions defined in this module, in deterministic (sorted) order.
    functions: Vec<String>,
    /// caller name → the ordered, de-duplicated list of static-direct callee
    /// names (module-internal targets only). Opaque calls are recorded in
    /// [`Self::has_opaque_call`], not here.
    edges: BTreeMap<String, Vec<String>>,
    /// Reverse edges: callee name → the callers that statically reference it.
    callers: BTreeMap<String, Vec<String>>,
    /// caller name → whether its body contains at least one *call* op (static
    /// or opaque). This is the leaf predicate's complement: a function with no
    /// call op of any kind is a leaf. (`AllocTask` poll references do NOT set
    /// this — allocating a task is not a call.)
    makes_any_call: BTreeMap<String, bool>,
    /// caller name → whether its body contains an opaque (unresolved) call.
    /// An opaque call can reach any function, so it is recursion-capable.
    has_opaque_call: BTreeMap<String, bool>,
    /// The set of functions that participate in a recursion cycle (a multi-node
    /// SCC, a direct self-edge, or any function able to reach itself through the
    /// static edges) plus every function with an opaque call (conservatively
    /// recursion-capable). See [`Self::recursive_set`].
    recursive: BTreeSet<String>,
}

impl CallGraph {
    /// Build the call graph from a lifted [`TirModule`].
    ///
    /// O(total ops + V + E): one linear scan to collect edges, then Tarjan SCC
    /// for the recursive set. Mirrors the `eliminate_dead_functions` BFS in its
    /// edge-collection structure (same op-kind treatment), but over TIR opcodes
    /// rather than SimpleIR kind strings, and additionally computes SCC /
    /// bottom-up order / the recursive set the IPO tier needs.
    pub fn build(module: &TirModule) -> CallGraph {
        let defined: BTreeSet<String> = module.functions.iter().map(|f| f.name.clone()).collect();

        let mut functions: Vec<String> = defined.iter().cloned().collect();
        functions.sort();

        let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut callers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut makes_any_call: BTreeMap<String, bool> = BTreeMap::new();
        let mut has_opaque_call: BTreeMap<String, bool> = BTreeMap::new();

        for func in &module.functions {
            let caller = &func.name;
            // De-duplicate static callees while preserving first-seen order is
            // unnecessary (we sort), so collect into a set then sort.
            let mut static_callees: BTreeSet<String> = BTreeSet::new();
            let mut any_call = false;
            let mut opaque = false;

            for block in func.blocks.values() {
                for op in &block.ops {
                    if let Some(edge) = classify_call_op(op, &defined) {
                        any_call = true;
                        match edge {
                            CallEdge::StaticDirect(callee) => {
                                static_callees.insert(callee);
                            }
                            CallEdge::Opaque => {
                                opaque = true;
                            }
                        }
                    }
                    // Generator/coroutine poll references: a callable reference
                    // that must appear in the reachability graph (so the poll fn
                    // is not treated as unreachable), but not a call out of the
                    // enclosing function.
                    if let Some(poll) = alloc_task_poll_target(op, &defined) {
                        static_callees.insert(poll);
                    }
                }
            }

            let mut callee_vec: Vec<String> = static_callees.into_iter().collect();
            callee_vec.sort();
            for callee in &callee_vec {
                callers
                    .entry(callee.clone())
                    .or_default()
                    .push(caller.clone());
            }
            edges.insert(caller.clone(), callee_vec);
            makes_any_call.insert(caller.clone(), any_call);
            has_opaque_call.insert(caller.clone(), opaque);
        }

        // Ensure every defined function has entries even if it has no edges, so
        // queries never miss a key.
        for name in &functions {
            edges.entry(name.clone()).or_default();
            makes_any_call.entry(name.clone()).or_insert(false);
            has_opaque_call.entry(name.clone()).or_insert(false);
        }
        for callees in edges.values() {
            for c in callees {
                callers.entry(c.clone()).or_default();
            }
        }
        for c in callers.values_mut() {
            c.sort();
            c.dedup();
        }

        let mut graph = CallGraph {
            functions,
            edges,
            callers,
            makes_any_call,
            has_opaque_call,
            recursive: BTreeSet::new(),
        };
        graph.recursive = graph.compute_recursive_set();
        graph
    }

    /// All functions in the module, in deterministic sorted order.
    pub fn functions(&self) -> &[String] {
        &self.functions
    }

    /// True iff `name` is a function defined in this module — the same
    /// membership predicate [`classify_call_op`]'s `defined` set encodes (a named
    /// `Call` target resolves to [`CallEdge::StaticDirect`] iff this holds, else
    /// [`CallEdge::Opaque`]). Exposed so the [`CallFacts`](crate::tir::call_facts)
    /// typed-target classifier resolves `StaticDirect` against the *same* truth
    /// the graph was built from, rather than re-deriving a `defined` set. O(log n)
    /// over the sorted [`Self::functions`] list.
    pub fn is_defined(&self, name: &str) -> bool {
        self.functions
            .binary_search_by(|f| f.as_str().cmp(name))
            .is_ok()
    }

    /// The static-direct callees of `name` (module-internal targets only),
    /// sorted and de-duplicated. Empty for an unknown function.
    pub fn callees(&self, name: &str) -> &[String] {
        self.edges.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The functions that statically call `name`, sorted and de-duplicated.
    pub fn callers(&self, name: &str) -> &[String] {
        self.callers.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// True if `name` contains at least one call op of any kind (static or
    /// opaque). The complement of the leaf predicate.
    pub fn makes_any_call(&self, name: &str) -> bool {
        self.makes_any_call.get(name).copied().unwrap_or(false)
    }

    /// True if `name` contains an opaque (unresolved / dynamic) call.
    pub fn has_opaque_call(&self, name: &str) -> bool {
        self.has_opaque_call.get(name).copied().unwrap_or(false)
    }

    /// **Leaf functions**: those whose body contains NO call op of any kind —
    /// neither a static-direct call, nor a method dispatch, nor an opaque /
    /// indirect call. A leaf cannot recurse (it calls nothing), so the native
    /// backend may skip the recursion guard at call sites targeting it.
    ///
    /// This is the SOUND, strictly-more-precise replacement for the legacy
    /// SimpleIR "has no user-level call op" scan: identical predicate, evaluated
    /// over the post-TIR-optimization opcode set.
    pub fn leaf_functions(&self) -> BTreeSet<String> {
        self.functions
            .iter()
            .filter(|name| !self.makes_any_call(name))
            .cloned()
            .collect()
    }

    /// The set of functions that may participate in recursion: any member of a
    /// non-trivial SCC, any function with a direct self-call edge, and — fail
    /// closed — any function containing an opaque call (which could re-enter
    /// the program and recurse). Consumed by a future bottom-up inliner (E1) to
    /// refuse unbounded inlining of recursive cycles.
    pub fn recursive_set(&self) -> &BTreeSet<String> {
        &self.recursive
    }

    /// Bottom-up topological order over the **SCC condensation**: each SCC
    /// appears after all SCCs it calls into, so a bottom-up interprocedural pass
    /// (inliner, IP-escape, purity) sees every callee's summary before its
    /// callers. Within one returned `Vec<String>`, the names form one SCC
    /// (singleton for non-recursive functions; multi-element for a recursion
    /// cycle), sorted for determinism. Opaque edges are not in the static graph,
    /// so they do not constrain the order (an opaque-calling function is ordered
    /// only by its static edges).
    pub fn bottom_up_order(&self) -> Vec<Vec<String>> {
        self.tarjan_sccs()
    }

    // -- internals -----------------------------------------------------------

    /// Compute the recursive set via Tarjan SCCs + self-edges + opaque calls.
    fn compute_recursive_set(&self) -> BTreeSet<String> {
        let mut recursive: BTreeSet<String> = BTreeSet::new();

        // Multi-node SCCs are recursion cycles.
        for scc in self.tarjan_sccs() {
            if scc.len() > 1 {
                for name in scc {
                    recursive.insert(name);
                }
            }
        }
        // Direct self-recursion (a single-node SCC with a self-edge — Tarjan
        // reports it as a singleton, so detect the self-edge explicitly).
        for (name, callees) in &self.edges {
            if callees.iter().any(|c| c == name) {
                recursive.insert(name.clone());
            }
        }
        // Fail-closed: an opaque call can re-enter the program and recurse.
        for (name, &opaque) in &self.has_opaque_call {
            if opaque {
                recursive.insert(name.clone());
            }
        }
        recursive
    }

    /// Tarjan's strongly-connected-components algorithm over the static-direct
    /// edge set, returned in **reverse topological (bottom-up) order**: callees
    /// before callers. Each inner `Vec` is one SCC, its members sorted.
    ///
    /// Iterative (explicit stack) to avoid blowing the native stack on deep call
    /// chains — the same reason the backend uses a 64 MB rayon stack for the TIR
    /// roundtrip.
    fn tarjan_sccs(&self) -> Vec<Vec<String>> {
        // Index functions for the array-based Tarjan state.
        let index_of: HashMap<&str, usize> = self
            .functions
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let n = self.functions.len();

        #[derive(Clone, Copy)]
        struct NodeState {
            index: Option<usize>,
            lowlink: usize,
            on_stack: bool,
        }
        let mut state = vec![
            NodeState {
                index: None,
                lowlink: 0,
                on_stack: false,
            };
            n
        ];
        let mut stack: Vec<usize> = Vec::new();
        let mut next_index = 0usize;
        let mut sccs: Vec<Vec<String>> = Vec::new();

        // Explicit DFS frame: the node plus a cursor into its successor list.
        struct Frame {
            node: usize,
            succ_cursor: usize,
        }

        // Precompute successor index lists (static edges only).
        let succ: Vec<Vec<usize>> = self
            .functions
            .iter()
            .map(|name| {
                self.callees(name)
                    .iter()
                    .filter_map(|c| index_of.get(c.as_str()).copied())
                    .collect::<Vec<usize>>()
            })
            .collect();

        for start in 0..n {
            if state[start].index.is_some() {
                continue;
            }
            let mut frames: Vec<Frame> = vec![Frame {
                node: start,
                succ_cursor: 0,
            }];
            state[start].index = Some(next_index);
            state[start].lowlink = next_index;
            next_index += 1;
            stack.push(start);
            state[start].on_stack = true;

            while let Some(frame) = frames.last_mut() {
                let v = frame.node;
                if frame.succ_cursor < succ[v].len() {
                    let w = succ[v][frame.succ_cursor];
                    frame.succ_cursor += 1;
                    match state[w].index {
                        None => {
                            state[w].index = Some(next_index);
                            state[w].lowlink = next_index;
                            next_index += 1;
                            stack.push(w);
                            state[w].on_stack = true;
                            frames.push(Frame {
                                node: w,
                                succ_cursor: 0,
                            });
                        }
                        Some(w_index) => {
                            if state[w].on_stack {
                                let v_low = state[v].lowlink;
                                state[v].lowlink = v_low.min(w_index);
                            }
                        }
                    }
                } else {
                    // All successors of v processed. If v is an SCC root, pop it.
                    if state[v].lowlink == state[v].index.unwrap() {
                        let mut component: Vec<String> = Vec::new();
                        loop {
                            let w = stack.pop().expect("tarjan stack underflow");
                            state[w].on_stack = false;
                            component.push(self.functions[w].clone());
                            if w == v {
                                break;
                            }
                        }
                        component.sort();
                        sccs.push(component);
                    }
                    // Propagate lowlink to the parent frame, if any.
                    frames.pop();
                    if let Some(parent) = frames.last() {
                        let p = parent.node;
                        let p_low = state[p].lowlink;
                        let v_low = state[v].lowlink;
                        state[p].lowlink = p_low.min(v_low);
                    }
                }
            }
        }

        // Tarjan emits SCCs in reverse-topological order already (callees before
        // callers), which is exactly the bottom-up order we want.
        sccs
    }
}

/// Convenience: BFS reachability over static-direct edges from a set of roots —
/// the same traversal shape as `passes::eliminate_dead_functions`, exposed here
/// for interprocedural passes that need "everything reachable from main".
impl CallGraph {
    /// Functions reachable from `roots` via static-direct edges (inclusive of
    /// the roots themselves that are defined in this module). Opaque edges are
    /// not followed (the target is unknown); a sound IPO consumer treats any
    /// function with an opaque caller as conservatively reachable separately.
    pub fn reachable_from<'a>(&self, roots: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
        let mut reachable: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        for r in roots {
            if self.edges.contains_key(r) && reachable.insert(r.to_string()) {
                queue.push_back(r.to_string());
            }
        }
        while let Some(name) = queue.pop_front() {
            for callee in self.callees(&name) {
                if reachable.insert(callee.clone()) {
                    queue.push_back(callee.clone());
                }
            }
        }
        reachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    /// Build a function whose entry block contains the given call ops, each as a
    /// first-class `Call` with an optional `s_value` callee name.
    fn func_calling(name: &str, callees: &[Option<&str>]) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], TirType::None);
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        for callee in callees {
            let mut attrs = AttrDict::new();
            if let Some(c) = callee {
                attrs.insert("s_value".into(), AttrValue::Str((*c).to_string()));
            }
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        func
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    // -- leaf detection -------------------------------------------------------

    #[test]
    fn leaf_is_function_with_no_calls() {
        // a → b, b is a leaf, c is a leaf (no calls).
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[]);
        let c = func_calling("c", &[]);
        let cg = CallGraph::build(&module(vec![a, b, c]));
        let leaves = cg.leaf_functions();
        assert!(leaves.contains("b"));
        assert!(leaves.contains("c"));
        assert!(!leaves.contains("a"), "a calls b → not a leaf");
    }

    #[test]
    fn opaque_call_disqualifies_leaf() {
        // f has a Call with no s_value (indirect) → opaque → not a leaf.
        let f = func_calling("f", &[None]);
        let cg = CallGraph::build(&module(vec![f]));
        assert!(!cg.leaf_functions().contains("f"));
        assert!(cg.has_opaque_call("f"));
    }

    #[test]
    fn call_to_extern_callee_is_opaque_not_leaf() {
        // g calls "ext" which is NOT in the module → opaque edge, not a leaf,
        // and no static edge recorded.
        let g = func_calling("g", &[Some("ext")]);
        let cg = CallGraph::build(&module(vec![g]));
        assert!(!cg.leaf_functions().contains("g"));
        assert!(cg.has_opaque_call("g"));
        assert!(
            cg.callees("g").is_empty(),
            "extern callee is not a static edge"
        );
    }

    #[test]
    fn call_method_disqualifies_leaf() {
        let mut f = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallMethod,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = CallGraph::build(&module(vec![f]));
        assert!(!cg.leaf_functions().contains("f"));
        assert!(cg.has_opaque_call("f"));
    }

    #[test]
    fn call_builtin_does_not_disqualify_leaf() {
        // A CallBuiltin (range/print/…) is NOT a user-level call — matches the
        // legacy SimpleIR scan, which ignores `call_builtin`.
        let mut f = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands: vec![],
            results: vec![],
            attrs: {
                let mut a = AttrDict::new();
                a.insert("name".into(), AttrValue::Str("print".into()));
                a
            },
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = CallGraph::build(&module(vec![f]));
        assert!(
            cg.leaf_functions().contains("f"),
            "builtin call ≠ user call"
        );
    }

    #[test]
    fn gpu_intrinsic_call_does_not_disqualify_leaf() {
        // A gpu_thread_id op lifts to OpCode::Call with s_value molt_gpu_thread_id
        // — a runtime intrinsic, NOT a user call. It must not disqualify leaf-ness
        // (the legacy SimpleIR scan ignored gpu_* op kinds).
        let mut f = TirFunction::new("kernel".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let v = f.fresh_value();
        let block = f.blocks.get_mut(&entry).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert(
            "s_value".into(),
            AttrValue::Str("molt_gpu_thread_id".into()),
        );
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![],
            results: vec![v],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = CallGraph::build(&module(vec![f]));
        assert!(
            cg.leaf_functions().contains("kernel"),
            "gpu intrinsic call ≠ user call → still a leaf"
        );
        assert!(!cg.has_opaque_call("kernel"));
    }

    #[test]
    fn copy_fallback_call_kind_disqualifies_leaf() {
        // A `Copy` op carrying _original_kind=call_func is a disguised call.
        let mut f = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str("call_func".into()));
        attrs.insert("s_value".into(), AttrValue::Str("g".into()));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let g = func_calling("g", &[]);
        let cg = CallGraph::build(&module(vec![f, g]));
        assert!(
            !cg.leaf_functions().contains("f"),
            "Copy[call_func] is a call"
        );
        assert_eq!(cg.callees("f"), &["g".to_string()]);
    }

    #[test]
    fn plain_copy_is_not_a_call() {
        // A `Copy` with no _original_kind (a real SSA copy) is NOT a call.
        let mut f = TirFunction::new("f".into(), vec![], TirType::None);
        let entry = f.entry_block;
        let v = f.fresh_value();
        let w = f.fresh_value();
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![v],
            results: vec![w],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = CallGraph::build(&module(vec![f]));
        assert!(cg.leaf_functions().contains("f"));
    }

    // -- edges / callers ------------------------------------------------------

    #[test]
    fn linear_chain_edges_and_callers() {
        // a → b → c
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = CallGraph::build(&module(vec![a, b, c]));
        assert_eq!(cg.callees("a"), &["b".to_string()]);
        assert_eq!(cg.callees("b"), &["c".to_string()]);
        assert!(cg.callees("c").is_empty());
        assert_eq!(cg.callers("b"), &["a".to_string()]);
        assert_eq!(cg.callers("c"), &["b".to_string()]);
        assert!(cg.callers("a").is_empty());
    }

    #[test]
    fn duplicate_call_edges_deduped() {
        // a calls b twice → one static edge.
        let a = func_calling("a", &[Some("b"), Some("b")]);
        let b = func_calling("b", &[]);
        let cg = CallGraph::build(&module(vec![a, b]));
        assert_eq!(cg.callees("a"), &["b".to_string()]);
    }

    // -- recursion / SCC ------------------------------------------------------

    #[test]
    fn direct_self_recursion_is_recursive() {
        let f = func_calling("f", &[Some("f")]);
        let cg = CallGraph::build(&module(vec![f]));
        assert!(cg.recursive_set().contains("f"));
    }

    #[test]
    fn mutual_recursion_is_recursive_scc() {
        // a → b → a
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("a")]);
        let cg = CallGraph::build(&module(vec![a, b]));
        let rec = cg.recursive_set();
        assert!(rec.contains("a"));
        assert!(rec.contains("b"));
        // The condensation has exactly one multi-node SCC: {a, b}.
        let order = cg.bottom_up_order();
        let multi: Vec<&Vec<String>> = order.iter().filter(|s| s.len() > 1).collect();
        assert_eq!(multi.len(), 1);
        assert_eq!(multi[0], &vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn non_recursive_chain_has_no_recursive_members() {
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = CallGraph::build(&module(vec![a, b, c]));
        assert!(cg.recursive_set().is_empty());
    }

    #[test]
    fn opaque_call_is_conservatively_recursive() {
        // An indirect call could re-enter → fail-closed recursive.
        let f = func_calling("f", &[None]);
        let cg = CallGraph::build(&module(vec![f]));
        assert!(cg.recursive_set().contains("f"));
    }

    // -- bottom-up order ------------------------------------------------------

    #[test]
    fn bottom_up_order_callees_before_callers() {
        // a → b → c : c must precede b must precede a.
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = CallGraph::build(&module(vec![a, b, c]));
        let order: Vec<String> = cg.bottom_up_order().into_iter().flatten().collect();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));
    }

    // -- alloc_task poll edges ------------------------------------------------

    #[test]
    fn alloc_task_records_poll_edge_but_not_leaf_disqualifier() {
        // g allocates a task referencing g_poll. g_poll is a leaf.
        let mut g = TirFunction::new("g".into(), vec![], TirType::None);
        let entry = g.entry_block;
        let block = g.blocks.get_mut(&entry).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("s_value".into(), AttrValue::Str("g_poll".into()));
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::AllocTask,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let g_poll = func_calling("g_poll", &[]);
        let cg = CallGraph::build(&module(vec![g, g_poll]));
        // alloc_task is a poll reference edge…
        assert_eq!(cg.callees("g"), &["g_poll".to_string()]);
        // …but does NOT count as a "call out of g", so g (with only the
        // alloc_task) is still a leaf in the no-call sense.
        assert!(
            cg.leaf_functions().contains("g"),
            "alloc_task ≠ call out of g"
        );
        assert!(cg.leaf_functions().contains("g_poll"));
    }

    // -- reachability ---------------------------------------------------------

    #[test]
    fn reachable_from_follows_static_edges() {
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let dead = func_calling("dead", &[]);
        let cg = CallGraph::build(&module(vec![a, b, c, dead]));
        let reach = cg.reachable_from(["a"]);
        assert!(reach.contains("a"));
        assert!(reach.contains("b"));
        assert!(reach.contains("c"));
        assert!(!reach.contains("dead"));
    }

    #[test]
    fn empty_module_builds() {
        let cg = CallGraph::build(&module(vec![]));
        assert!(cg.functions().is_empty());
        assert!(cg.leaf_functions().is_empty());
        assert!(cg.recursive_set().is_empty());
        assert!(cg.bottom_up_order().is_empty());
    }
}
