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
//! [`crate::tir::pass_manager::PassManager`] pipeline runs (in parallel, on
//! `molt_tir::tir::pipeline_cache`'s compiler pool). It is a *read-only*
//! analysis of the lifted [`TirModule`] — it does not mutate the functions.
//!
//! ## Callee resolution: static-direct vs dynamic/opaque
//!
//! Every call-bearing op in a function body produces a [`CallEdge`]:
//!
//! * **[`CallEdge::StaticDirect`]** — a call whose typed callee is a
//!   [`FunctionId`] with a body in this module and whose preserved source role
//!   proves a direct target. Registered direct-call Copy transports retain an
//!   edge but are not first-class inlining sites.
//! * **[`CallEdge::Opaque`]** — a call with no proved direct callee, an extern
//!   target, a dynamic method dispatch or lookup, or a `Copy`
//!   op carrying a call-kind
//!   `origin` (the SSA-lift fallback spelling of `call_func` /
//!   `call_indirect` / `call_bind` / `invoke_ffi` etc. — see
//!   [`crate::tir::ssa`]). An opaque call may reach *any* function (including
//!   back into this one), so it is conservatively a recursion-capable edge.
//!
//! [`super::ops::OpCode::CallBuiltin`] is an opaque call edge. Public names can resolve
//! to Python replacements, and dedicated runtime primitives may themselves
//! invoke Python protocols. Acquiring a builtin reference can also invoke a
//! custom namespace mapping; its preserved kind records the same opaque edge.
//! Dedicated module namespace reads have the same callback capability:
//! global lookup admits mapping callbacks, while module attributes, names,
//! and from-import acquisition admit module attribute callbacks. The same
//! operation-aware effect authority covers arithmetic, predicates, iteration,
//! attribute/index mutation, preserved runtime operations, asynchronous-work
//! observations, and explicit releases. Exact primitive and typed-slot facts
//! discharge only proved callback paths; impurity alone is not a call edge.
//! Final native leaf metadata consumes bodies after lifetime/drop finalization.
//!
//! ## Generator poll edges
//!
//! An [`super::ops::OpCode::AllocTask`] names its poll entry by typed callee id.
//! That is a genuine callable reference: the runtime later drives the task by
//! calling the poll function. The shared visitor reports it for SCC and
//! reachability when its body is in this module. It does *not*
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
//! functions with **no outgoing static-or-opaque call edge**. This includes
//! mutable builtin dispatch and namespace lookup callbacks omitted by the old
//! spelling scan. TIR DCE / devirtualization may remove edges, but retained
//! opaque edges must still prevent recursion-guard elision. See the `leaf_*`
//! and builtin-dispatch unit tests.

use std::collections::VecDeque;
use std::sync::OnceLock;

use molt_ir::{FunctionId, FunctionSet};

use super::blocks::BlockId;
use super::call_sites::{CallEvent, CallSite, CallSiteTarget, visit_sites};
use super::function::TirModule;
use super::ops::OpCode;

/// A resolved or unresolved call target referenced by one call-bearing op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEdge {
    /// A statically-resolved direct call to a function with a body in this
    /// module, by its typed id (HF-847 slice 1). The inliner / IPSCCP /
    /// IP-escape act on exactly these edges.
    StaticDirect(FunctionId),
    /// A call whose concrete target is unknown to this module: an indirect /
    /// computed callee, a method dispatch, or a callee whose row is an extern
    /// declaration (compiled in another batch). Conservatively treated as
    /// reaching any function — so it is recursion-capable and blocks the
    /// `does-not-recurse` leaf property of its *enclosing* function.
    Opaque,
}

/// Resolve the shared operation-aware site projection against this module.
fn classify_call_site(site: &CallSite, has_body: &FunctionSet) -> Option<CallEdge> {
    match site.target {
        CallSiteTarget::RuntimePrimitive => None,
        CallSiteTarget::Direct(callee) if has_body.contains(callee) => {
            Some(CallEdge::StaticDirect(callee))
        }
        CallSiteTarget::Direct(_) | CallSiteTarget::Opaque => Some(CallEdge::Opaque),
    }
}

/// A direct call with an in-module body, indexed in the caller snapshot.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StaticCallSite {
    pub(crate) block: BlockId,
    pub(crate) op_index: usize,
    pub(crate) callee: FunctionId,
}

/// The whole-program call graph of a [`TirModule`].
///
/// An immutable snapshot of the module's call edges. Rebuild it to query
/// edges changed by a transform. Every per-function fact is indexed by the
/// module table's typed [`FunctionId`] (HF-847 slice 1, I7).
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    /// Functions with a body in this module, in id order.
    functions: Vec<FunctionId>,
    has_body: FunctionSet,
    /// id → the sorted, de-duplicated static-direct callees (bodies only).
    /// Opaque calls are recorded in [`Self::has_opaque_call`], not here.
    edges: Vec<Vec<FunctionId>>,
    static_call_sites: Vec<Vec<StaticCallSite>>,
    /// id → the functions that statically reference it.
    callers: Vec<Vec<FunctionId>>,
    /// Functions whose body contains at least one *call* op (static or
    /// opaque): the leaf predicate's complement. (`AllocTask` poll references
    /// do NOT set this — allocating a task is not a call.)
    makes_any_call: FunctionSet,
    /// Functions whose body contains an opaque (unresolved) call. An opaque
    /// call can reach any function, so it is recursion-capable.
    has_opaque_call: FunctionSet,
    /// Computed only for consumers that need cycle membership or callee order.
    components: OnceLock<CallComponents>,
}

/// Components in callee-first order, with contiguous members and offsets.
/// Most components are singletons, so avoid one heap allocation per function.
#[derive(Debug, Clone)]
struct CallComponents {
    members: Vec<FunctionId>,
    offsets: Vec<usize>,
    static_cycle: FunctionSet,
    recursive: FunctionSet,
}

impl CallGraph {
    /// Build the call graph from a lifted [`TirModule`].
    ///
    /// Computes exact scalar provenance and candidate typed-slot facts once
    /// per function, then scans operation/terminator callback sites. Cycle queries
    /// share the decomposition of these immutable edges. Provenance and slot
    /// analysis include fixed points.
    pub fn build(module: &TirModule) -> CallGraph {
        let rows = module.table.len();
        let mut functions = module.ids.clone();
        functions.sort_unstable();
        let mut has_body = FunctionSet::with_capacity(rows);
        for &id in &functions {
            has_body.insert(id);
        }

        let mut edges: Vec<Vec<FunctionId>> = vec![Vec::new(); rows];
        let mut static_call_sites = vec![Vec::new(); rows];
        let mut callers: Vec<Vec<FunctionId>> = vec![Vec::new(); rows];
        let mut makes_any_call = FunctionSet::with_capacity(rows);
        let mut has_opaque_call = FunctionSet::with_capacity(rows);

        for (func, &caller) in module.functions.iter().zip(&module.ids) {
            let mut static_callees: Vec<FunctionId> = Vec::new();
            let sites = &mut static_call_sites[caller.index()];
            let mut any_call = false;
            let mut opaque = false;

            visit_sites(func, |event| {
                match event {
                    CallEvent::Operation {
                        position: (block, op_index),
                        op,
                        site,
                    } => {
                        if let Some(edge) = classify_call_site(&site, &has_body) {
                            any_call = true;
                            match edge {
                                CallEdge::StaticDirect(callee) => {
                                    static_callees.push(callee);
                                    if op.opcode == OpCode::Call {
                                        sites.push(StaticCallSite {
                                            block,
                                            op_index,
                                            callee,
                                        });
                                    }
                                }
                                CallEdge::Opaque => opaque = true,
                            }
                        }
                    }
                    CallEvent::Terminator { .. } => {
                        any_call = true;
                        opaque = true;
                    }
                    CallEvent::PollReference(poll) => {
                        if has_body.contains(poll) {
                            static_callees.push(poll);
                        }
                    }
                }
                false
            });

            sites.sort_unstable_by_key(|site| (site.block, site.op_index));
            static_callees.sort_unstable();
            static_callees.dedup();
            for &callee in &static_callees {
                callers[callee.index()].push(caller);
            }
            edges[caller.index()] = static_callees;
            if any_call {
                makes_any_call.insert(caller);
            }
            if opaque {
                has_opaque_call.insert(caller);
            }
        }
        for list in &mut callers {
            list.sort_unstable();
            list.dedup();
        }

        CallGraph {
            functions,
            has_body,
            edges,
            static_call_sites,
            callers,
            makes_any_call,
            has_opaque_call,
            components: OnceLock::new(),
        }
    }

    /// The functions with a body in this module, in id order.
    pub fn functions(&self) -> &[FunctionId] {
        &self.functions
    }

    /// True iff `id` has a body in this module — the membership predicate
    /// [`classify_call_site`] encodes (a direct callee resolves to
    /// [`CallEdge::StaticDirect`] iff this holds, else [`CallEdge::Opaque`]).
    /// Exposed so the [`CallFacts`](crate::tir::call_facts) typed-target
    /// classifier resolves against the *same* truth the graph was built from.
    pub fn has_body(&self, id: FunctionId) -> bool {
        self.has_body.contains(id)
    }

    /// The static-direct callees of `id` (bodies in this module only),
    /// sorted and de-duplicated. Empty for a function without a body.
    pub fn callees(&self, id: FunctionId) -> &[FunctionId] {
        self.edges.get(id.index()).map(Vec::as_slice).unwrap_or(&[])
    }

    /// First-class direct calls in ascending block and operation order.
    /// Coordinates belong to the caller body used to build this graph.
    pub(crate) fn static_call_sites(&self, caller: FunctionId) -> &[StaticCallSite] {
        self.static_call_sites
            .get(caller.index())
            .map_or(&[], Vec::as_slice)
    }

    /// The functions that statically call `id`, sorted and de-duplicated.
    pub fn callers(&self, id: FunctionId) -> &[FunctionId] {
        self.callers
            .get(id.index())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// True if `id` contains at least one call op of any kind (static or
    /// opaque). The complement of the leaf predicate.
    pub fn makes_any_call(&self, id: FunctionId) -> bool {
        self.makes_any_call.contains(id)
    }

    /// True if `id` contains an opaque (unresolved / dynamic) call.
    pub fn has_opaque_call(&self, id: FunctionId) -> bool {
        self.has_opaque_call.contains(id)
    }

    /// **Leaf functions**: those whose body contains NO call op of any kind —
    /// neither a static-direct call, nor a method dispatch, nor an opaque /
    /// indirect call. A leaf cannot recurse (it calls nothing), so the native
    /// backend may skip the recursion guard at call sites targeting it.
    pub fn leaf_functions(&self) -> FunctionSet {
        let mut leaves = FunctionSet::with_capacity(self.edges.len());
        for &id in &self.functions {
            if !self.makes_any_call(id) {
                leaves.insert(id);
            }
        }
        leaves
    }

    /// The set of functions that may participate in recursion: any member of a
    /// non-trivial SCC, any function with a direct self-call edge, and — fail
    /// closed — any function containing an opaque call (which could re-enter
    /// the program and recurse). The native recursion guard reads it.
    pub fn recursive_set(&self) -> &FunctionSet {
        &self.components().recursive
    }

    /// Whether `id` lies on a static call cycle: a member of a multi-node SCC
    /// or a function that calls itself directly. A splice (inlining, generator
    /// fusion) copies a body at compile time, so only such a cycle can make it
    /// unbounded; an opaque call cannot (design spliced_frames.md, 4.1).
    pub fn in_static_cycle(&self, id: FunctionId) -> bool {
        self.components().static_cycle.contains(id)
    }

    /// Bottom-up topological order over the **SCC condensation**: each SCC
    /// appears after all SCCs it calls into, so a bottom-up interprocedural pass
    /// (inliner, IP-escape, purity) sees every callee's summary before its
    /// callers. Each slice is one SCC, with its members sorted. Opaque
    /// edges do not constrain the order.
    pub fn bottom_up_order(&self) -> impl ExactSizeIterator<Item = &[FunctionId]> {
        let components = self.components();
        components
            .offsets
            .windows(2)
            .map(|bounds| &components.members[bounds[0]..bounds[1]])
    }

    fn components(&self) -> &CallComponents {
        self.components.get_or_init(|| self.tarjan_sccs())
    }

    /// Iterative Tarjan over static edges. The explicit stack also handles
    /// giant call chains without growing the native stack.
    fn tarjan_sccs(&self) -> CallComponents {
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
            self.edges.len()
        ];
        let mut stack = Vec::new();
        let mut next_index = 0usize;
        let mut members = Vec::with_capacity(self.functions.len());
        let mut offsets = Vec::with_capacity(self.functions.len() + 1);
        offsets.push(0);
        let mut static_cycle = FunctionSet::with_capacity(self.edges.len());
        let mut recursive = self.has_opaque_call.clone();

        struct Frame {
            node: FunctionId,
            succ_cursor: usize,
        }

        let mut frames = Vec::new();
        for &start in &self.functions {
            if state[start.index()].index.is_some() {
                continue;
            }
            frames.push(Frame {
                node: start,
                succ_cursor: 0,
            });
            state[start.index()] = NodeState {
                index: Some(next_index),
                lowlink: next_index,
                on_stack: true,
            };
            next_index += 1;
            stack.push(start);

            while let Some(frame) = frames.last_mut() {
                let v = frame.node;
                let successors = self.callees(v);
                if frame.succ_cursor < successors.len() {
                    let w = successors[frame.succ_cursor];
                    frame.succ_cursor += 1;
                    match state[w.index()].index {
                        None => {
                            state[w.index()] = NodeState {
                                index: Some(next_index),
                                lowlink: next_index,
                                on_stack: true,
                            };
                            next_index += 1;
                            stack.push(w);
                            frames.push(Frame {
                                node: w,
                                succ_cursor: 0,
                            });
                        }
                        Some(w_index) if state[w.index()].on_stack => {
                            let low = &mut state[v.index()].lowlink;
                            *low = (*low).min(w_index);
                        }
                        Some(_) => {}
                    }
                } else {
                    if Some(state[v.index()].lowlink) == state[v.index()].index {
                        let start = members.len();
                        loop {
                            let w = stack.pop().expect("tarjan stack underflow");
                            state[w.index()].on_stack = false;
                            members.push(w);
                            if w == v {
                                break;
                            }
                        }
                        let component = &mut members[start..];
                        component.sort_unstable();
                        if component.len() > 1 || successors.binary_search(&v).is_ok() {
                            for &member in component.iter() {
                                static_cycle.insert(member);
                                recursive.insert(member);
                            }
                        }
                        offsets.push(members.len());
                    }
                    frames.pop();
                    if let Some(parent) = frames.last() {
                        let v_low = state[v.index()].lowlink;
                        let low = &mut state[parent.node.index()].lowlink;
                        *low = (*low).min(v_low);
                    }
                }
            }
        }
        CallComponents {
            members,
            offsets,
            static_cycle,
            recursive,
        }
    }
}

/// Convenience: BFS reachability over static-direct edges from a set of roots,
/// for interprocedural passes that need "everything reachable from main".
impl CallGraph {
    /// Functions reachable from `roots` via static-direct edges (inclusive of
    /// the roots with a body in this module). Opaque edges are not followed.
    pub fn reachable_from(&self, roots: impl IntoIterator<Item = FunctionId>) -> FunctionSet {
        let mut reachable = FunctionSet::with_capacity(self.edges.len());
        let mut queue = VecDeque::new();
        for root in roots {
            if self.has_body(root) && reachable.insert(root) {
                queue.push_back(root);
            }
        }
        while let Some(id) = queue.pop_front() {
            for &callee in self.callees(id) {
                if reachable.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }
        reachable
    }
}

#[cfg(test)]
pub(crate) mod named {
    //! The graph read back through its module's symbols, for assertions in
    //! tests that build modules by name ([`TirModule::decode_spelled`]).

    use super::*;
    use std::collections::BTreeSet;

    pub(crate) struct NamedCallGraph {
        cg: CallGraph,
        table: molt_ir::FunctionTable,
    }

    impl std::ops::Deref for NamedCallGraph {
        type Target = CallGraph;
        fn deref(&self) -> &CallGraph {
            &self.cg
        }
    }

    impl NamedCallGraph {
        pub(crate) fn build(module: &TirModule) -> Self {
            Self {
                cg: CallGraph::build(module),
                table: module.table.clone(),
            }
        }
        pub(crate) fn id(&self, name: &str) -> FunctionId {
            self.table
                .ids()
                .find(|id| self.table.symbol(*id) == name)
                .unwrap_or_else(|| panic!("no row {name}"))
        }
        fn names(&self, ids: impl IntoIterator<Item = FunctionId>) -> Vec<String> {
            ids.into_iter()
                .map(|id| self.table.symbol(id).to_string())
                .collect()
        }
        fn set(&self, set: &FunctionSet) -> BTreeSet<String> {
            self.names(set.iter()).into_iter().collect()
        }
        pub(crate) fn is_defined(&self, name: &str) -> bool {
            self.table
                .ids()
                .any(|id| self.table.symbol(id) == name && self.cg.has_body(id))
        }
        pub(crate) fn functions(&self) -> Vec<String> {
            self.names(self.cg.functions().iter().copied())
        }
        pub(crate) fn leaf_functions(&self) -> BTreeSet<String> {
            self.set(&self.cg.leaf_functions())
        }
        pub(crate) fn recursive_set(&self) -> BTreeSet<String> {
            self.set(self.cg.recursive_set())
        }
        pub(crate) fn callees(&self, name: &str) -> Vec<String> {
            self.names(self.cg.callees(self.id(name)).iter().copied())
        }
        pub(crate) fn callers(&self, name: &str) -> Vec<String> {
            self.names(self.cg.callers(self.id(name)).iter().copied())
        }
        pub(crate) fn in_static_cycle(&self, name: &str) -> bool {
            self.cg.in_static_cycle(self.id(name))
        }
        pub(crate) fn has_opaque_call(&self, name: &str) -> bool {
            self.cg.has_opaque_call(self.id(name))
        }
        pub(crate) fn makes_any_call(&self, name: &str) -> bool {
            self.cg.makes_any_call(self.id(name))
        }
        pub(crate) fn bottom_up_order(&self) -> Vec<Vec<String>> {
            self.cg
                .bottom_up_order()
                .map(|scc| self.names(scc.iter().copied()))
                .collect()
        }
        pub(crate) fn reachable_from<const N: usize>(&self, roots: [&str; N]) -> BTreeSet<String> {
            self.set(&self.cg.reachable_from(roots.map(|root| self.id(root))))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tir::blocks::Terminator;
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    /// Build a function whose entry block contains the given call ops, each as a
    /// first-class `Call` with an optional `s_value` callee name.
    fn func_calling(name: &str, callees: &[Option<&str>]) -> TirFunction {
        let mut func = TirFunction::new(
            name.into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        for callee in callees {
            let mut attrs = AttrDict::default();
            if let Some(c) = callee {
                attrs.insert("s_value".into(), AttrValue::Str((*c).to_string()));
            }
            block.ops.push(TirOp {
                origin: None,
                runtime: None,
                callee: None,
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
        TirModule::decode_spelled("m", funcs)
    }

    use super::{CallGraph, named::NamedCallGraph as G};

    // -- leaf detection -------------------------------------------------------

    #[test]
    fn leaf_is_function_with_no_calls() {
        // a → b, b is a leaf, c is a leaf (no calls).
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[]);
        let c = func_calling("c", &[]);
        let cg = G::build(&module(vec![a, b, c]));
        let leaves = cg.leaf_functions();
        assert!(leaves.contains("b"));
        assert!(leaves.contains("c"));
        assert!(!leaves.contains("a"), "a calls b → not a leaf");
    }

    #[test]
    fn opaque_call_disqualifies_leaf() {
        // f has a Call with no s_value (indirect) → opaque → not a leaf.
        let f = func_calling("f", &[None]);
        let cg = G::build(&module(vec![f]));
        assert!(!cg.leaf_functions().contains("f"));
        assert!(cg.has_opaque_call("f"));
    }

    #[test]
    fn call_to_extern_callee_is_opaque_not_leaf() {
        // g calls "ext" which is NOT in the module → opaque edge, not a leaf,
        // and no static edge recorded.
        let g = func_calling("g", &[Some("ext")]);
        let cg = G::build(&module(vec![g]));
        assert!(!cg.leaf_functions().contains("g"));
        assert!(cg.has_opaque_call("g"));
        assert!(
            cg.callees("g").is_empty(),
            "extern callee is not a static edge"
        );
    }

    #[test]
    fn call_method_disqualifies_leaf() {
        let mut f = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CallMethod,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::default(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = G::build(&module(vec![f]));
        assert!(!cg.leaf_functions().contains("f"));
        assert!(cg.has_opaque_call("f"));
    }

    #[test]
    fn builtin_dispatch_and_namespace_acquisition_are_opaque_call_edges() {
        for opcode in [OpCode::CallBuiltin, OpCode::Copy] {
            let mut function = TirFunction::new(
                "f".into(),
                vec![TirType::Str],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let argument = function.blocks[&function.entry_block].args[0].id;
            let result = function.fresh_value();
            let mut attrs = AttrDict::default();
            if opcode == OpCode::Copy {
                attrs.insert("builtin_name".into(), AttrValue::Str("len".into()));
            } else {
                attrs.insert("name".into(), AttrValue::Str("print".into()));
            }
            function
                .blocks
                .get_mut(&function.entry_block)
                .unwrap()
                .ops
                .push(TirOp {
                    origin: Some(molt_ir::OpKind::BuiltinFunc),
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![argument],
                    results: vec![result],
                    attrs,
                    source_span: None,
                });
            let graph = G::build(&module(vec![function]));
            assert!(graph.makes_any_call("f"));
            assert!(graph.has_opaque_call("f"));
            assert!(!graph.leaf_functions().contains("f"));
        }
    }

    #[test]
    fn gpu_intrinsic_call_does_not_disqualify_leaf() {
        for (kind, symbol) in [
            (molt_ir::OpKind::GpuThreadId, "molt_gpu_thread_id"),
            (molt_ir::OpKind::GpuBlockId, "molt_gpu_block_id"),
            (molt_ir::OpKind::GpuBlockDim, "molt_gpu_block_dim"),
            (molt_ir::OpKind::GpuGridDim, "molt_gpu_grid_dim"),
            (molt_ir::OpKind::GpuBarrier, "molt_gpu_barrier"),
        ] {
            let mut f = func_calling("kernel", &[Some(symbol)]);
            f.blocks.get_mut(&f.entry_block).unwrap().ops[0].origin = Some(kind);
            let cg = G::build(&module(vec![f]));
            assert!(
                cg.leaf_functions().contains("kernel"),
                "{kind}: GPU intrinsic call is not a user edge"
            );
            assert!(!cg.has_opaque_call("kernel"), "{kind}");
            assert!(cg.callees("kernel").is_empty(), "{kind}");
        }
    }

    #[test]
    fn opaque_call_spellings_cannot_borrow_gpu_or_module_identity() {
        for kind in [
            molt_ir::OpKind::CallFunc,
            molt_ir::OpKind::CallFunction,
            molt_ir::OpKind::CallIndirect,
            molt_ir::OpKind::CallBind,
            molt_ir::OpKind::CallGuarded,
            molt_ir::OpKind::InvokeFfi,
        ] {
            for symbol in ["molt_gpu_thread_id", "module_function"] {
                let mut caller = func_calling("caller", &[Some(symbol)]);
                caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0].origin = Some(kind);
                let cg = G::build(&module(vec![caller, func_calling(symbol, &[])]));
                assert!(cg.has_opaque_call("caller"), "{kind}: {symbol}");
                assert!(!cg.leaf_functions().contains("caller"), "{kind}: {symbol}");
                assert!(cg.callees("caller").is_empty(), "{kind}: {symbol}");
            }
        }
    }

    #[test]
    fn ordinary_gpu_named_module_calls_keep_their_edges() {
        for symbol in [
            "molt_gpu_thread_id",
            "molt_gpu_block_id",
            "molt_gpu_block_dim",
            "molt_gpu_grid_dim",
            "molt_gpu_barrier",
        ] {
            for original in [
                None,
                Some(molt_ir::OpKind::Call),
                Some(molt_ir::OpKind::CallInternal),
            ] {
                let mut caller = func_calling("caller", &[Some(symbol)]);
                if let Some(kind) = original {
                    caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0].origin = Some(kind);
                }
                let cg = G::build(&module(vec![caller, func_calling(symbol, &[])]));
                assert!(!cg.has_opaque_call("caller"), "{symbol}: {original:?}");
                assert!(
                    !cg.leaf_functions().contains("caller"),
                    "{symbol}: {original:?}"
                );
                assert_eq!(
                    cg.callees("caller"),
                    &[symbol.to_string()],
                    "{symbol}: {original:?}"
                );
            }
        }
    }

    #[test]
    fn copy_call_transport_uses_generated_direct_and_opaque_roles() {
        for kind in [
            molt_ir::OpKind::Call,
            molt_ir::OpKind::CallInternal,
            molt_ir::OpKind::CallFunc,
            molt_ir::OpKind::CallFunction,
            molt_ir::OpKind::CallIndirect,
            molt_ir::OpKind::CallBind,
            molt_ir::OpKind::CallGuarded,
            molt_ir::OpKind::InvokeFfi,
        ] {
            for symbol in ["module_function", "molt_gpu_thread_id"] {
                let mut caller = func_calling("caller", &[Some(symbol)]);
                let result = caller.fresh_value();
                let op = &mut caller.blocks.get_mut(&caller.entry_block).unwrap().ops[0];
                op.opcode = OpCode::Copy;
                op.results = vec![result];
                op.origin = Some(kind);
                let cg = G::build(&module(vec![caller, func_calling(symbol, &[])]));
                let direct = matches!(kind, molt_ir::OpKind::Call | molt_ir::OpKind::CallInternal);
                assert_eq!(cg.has_opaque_call("caller"), !direct, "{kind}: {symbol}");
                assert_eq!(
                    cg.callees("caller").len(),
                    usize::from(direct),
                    "{kind}: {symbol}"
                );
                assert!(!cg.leaf_functions().contains("caller"), "{kind}: {symbol}");
            }
        }
    }

    #[test]
    fn copy_fallback_call_kind_disqualifies_leaf() {
        // A `Copy` op carrying origin=call_func is a disguised call.
        let mut f = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        let mut attrs = AttrDict::default();

        attrs.insert("s_value".into(), AttrValue::Str("g".into()));
        block.ops.push(TirOp {
            origin: Some(molt_ir::OpKind::CallFunc),
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![],
            results: vec![],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let g = func_calling("g", &[]);
        let cg = G::build(&module(vec![f, g]));
        assert!(
            !cg.leaf_functions().contains("f"),
            "Copy[call_func] is a call"
        );
        assert!(cg.has_opaque_call("f"));
        assert!(cg.callees("f").is_empty());
    }

    #[test]
    fn plain_copy_is_not_a_call() {
        // A `Copy` with no origin (a real SSA copy) is NOT a call.
        let mut f = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = f.entry_block;
        let v = f.fresh_value();
        let w = f.fresh_value();
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![v],
            results: vec![w],
            attrs: AttrDict::default(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let cg = G::build(&module(vec![f]));
        assert!(cg.leaf_functions().contains("f"));
    }

    // -- edges / callers ------------------------------------------------------

    #[test]
    fn linear_chain_edges_and_callers() {
        // a → b → c
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = G::build(&module(vec![a, b, c]));
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
        let cg = G::build(&module(vec![a, b]));
        assert_eq!(cg.callees("a"), &["b".to_string()]);
    }

    // -- recursion / SCC ------------------------------------------------------

    #[test]
    fn direct_self_recursion_is_recursive() {
        let f = func_calling("f", &[Some("f")]);
        let cg = G::build(&module(vec![f]));
        assert!(cg.recursive_set().contains("f"));
        assert!(cg.in_static_cycle("f"));
    }

    #[test]
    fn mutual_recursion_is_recursive_scc() {
        // a → b → a
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("a")]);
        let cg = G::build(&module(vec![a, b]));
        let rec = cg.recursive_set();
        assert!(rec.contains("a"));
        assert!(rec.contains("b"));
        assert!(cg.in_static_cycle("a") && cg.in_static_cycle("b"));
        // The condensation has exactly one multi-node SCC: {a, b}.
        let order = cg.bottom_up_order();
        let multi: Vec<&Vec<String>> = order.iter().filter(|s| s.len() > 1).collect();
        assert_eq!(multi.len(), 1);
        assert_eq!(multi[0], &vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn rebuilding_after_edge_removal_preserves_each_graphs_components() {
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("a")]);
        let mut module = module(vec![a, b]);
        let (a, b) = (module.ids[0], module.ids[1]);
        let cycle = CallGraph::build(&module);

        // Remove b -> a. Ask for the old graph's decomposition only after
        // building its replacement, so both queries must use their own edges.
        let body = &mut module.functions[1];
        body.blocks.get_mut(&body.entry_block).unwrap().ops.clear();
        let chain = CallGraph::build(&module);
        assert!(chain.recursive_set().is_empty());
        assert_eq!(
            chain.bottom_up_order().collect::<Vec<_>>(),
            [&[b][..], &[a][..]]
        );
        assert!(cycle.in_static_cycle(a));
        assert!(cycle.in_static_cycle(b));
        assert_eq!(cycle.bottom_up_order().collect::<Vec<_>>(), [&[a, b][..]]);
    }

    #[test]
    fn non_recursive_chain_has_no_recursive_members() {
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = G::build(&module(vec![a, b, c]));
        assert!(cg.recursive_set().is_empty());
    }

    #[test]
    fn opaque_call_is_conservatively_recursive() {
        // An indirect call could re-enter → fail-closed recursive.
        let f = func_calling("f", &[None]);
        let cg = G::build(&module(vec![f]));
        assert!(cg.recursive_set().contains("f"));
        // A splice copies the body once: the opaque call makes no static cycle.
        assert!(!cg.in_static_cycle("f"));
    }

    #[test]
    fn static_cycle_set_is_the_recursive_set_without_opaque_calls() {
        // a → b → a is a static cycle; c calls an unknown target and a; d
        // calls c. Only a and b lie on a static cycle.
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("a")]);
        let c = func_calling("c", &[None, Some("a")]);
        let d = func_calling("d", &[Some("c")]);
        use std::collections::BTreeSet;
        let cg = G::build(&module(vec![a, b, c, d]));
        let cycle: BTreeSet<String> = cg
            .functions()
            .into_iter()
            .filter(|name| cg.in_static_cycle(name))
            .collect();
        assert_eq!(cycle, BTreeSet::from(["a".to_string(), "b".to_string()]));
        assert_eq!(
            cg.recursive_set(),
            BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    // -- bottom-up order ------------------------------------------------------

    #[test]
    fn bottom_up_order_callees_before_callers() {
        // a → b → c : c must precede b must precede a.
        let a = func_calling("a", &[Some("b")]);
        let b = func_calling("b", &[Some("c")]);
        let c = func_calling("c", &[]);
        let cg = G::build(&module(vec![a, b, c]));
        let order: Vec<String> = cg.bottom_up_order().into_iter().flatten().collect();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));
    }

    // -- alloc_task poll edges ------------------------------------------------

    #[test]
    fn alloc_task_records_poll_edge_but_not_leaf_disqualifier() {
        // g allocates a task referencing g_poll. g_poll is a leaf.
        let mut g = TirFunction::new(
            "g".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let entry = g.entry_block;
        let block = g.blocks.get_mut(&entry).unwrap();
        let mut attrs = AttrDict::default();
        attrs.insert("s_value".into(), AttrValue::Str("g_poll".into()));
        block.ops.push(TirOp {
            origin: None,
            runtime: None,
            callee: None,
            dialect: Dialect::Molt,
            opcode: OpCode::AllocTask,
            operands: vec![],
            results: vec![crate::tir::values::ValueId(0)],
            attrs,
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        let g_poll = func_calling("g_poll", &[]);
        let cg = G::build(&module(vec![g, g_poll]));
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
        let cg = G::build(&module(vec![a, b, c, dead]));
        let reach = cg.reachable_from(["a"]);
        assert!(reach.contains("a"));
        assert!(reach.contains("b"));
        assert!(reach.contains("c"));
        assert!(!reach.contains("dead"));
    }

    #[test]
    fn empty_module_builds() {
        let cg = G::build(&module(vec![]));
        assert!(cg.functions().is_empty());
        assert!(cg.leaf_functions().is_empty());
        assert!(cg.recursive_set().is_empty());
        assert!(cg.bottom_up_order().is_empty());
    }
}
