//! The context worklist (design 78, section 3.3).
//!
//! A context is a body and one key: an [`ArgKind`] per parameter, `Dyn` for
//! every parameter that is not relevant or not proven. The worklist starts
//! with generic contexts (all `Dyn`) for both endpoints of resolved calls.
//! Unseen callers still enter the original compiled body. Evaluating
//! a context solves the body's exact scalar facts from the key and from the
//! return types of the contexts its resolved calls enter; each call's
//! argument facts select that context, creating it on first use. Return
//! types start at bottom and rise to the fixpoint, so a recursive `fib(int)`
//! returns `int`.
//!
//! Termination: a key has a finite domain, and every fact rises in a finite
//! lattice. A site's result is the meet of every context it has entered, so
//! it never falls when its key widens; a context is evaluated again only
//! when a return type it read rises.

use molt_ir::deterministic_hash::HashMap;
use std::cell::OnceCell;
use std::collections::{BTreeSet, VecDeque};

use crate::tir::function::TirModule;
use crate::tir::type_refine::{ExactCallResults, ExactScalarPlan, ExactSeeds};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::sites::{ArgKind, Resolution, Site, join_return, returns_value};

pub(super) type Key = Vec<ArgKind>;

/// A context's index in [`Analysis::contexts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct ContextId(pub(super) usize);

#[derive(Debug)]
pub(super) struct Context {
    /// The body's index in the module.
    pub(super) body: usize,
    pub(super) key: Key,
    /// The exact return type; `Never` until a return is seen.
    pub(super) ret: TirType,
    /// Per resolved site of the body (aligned with its site list): the
    /// context it enters, once its argument facts are known.
    pub(super) targets: Vec<Option<ContextId>>,
    /// Per resolved site: the meet of the return types of every context it
    /// has entered.
    pub(super) results: Vec<TirType>,
}

pub(super) struct Analysis {
    pub(super) contexts: Vec<Context>,
    index: HashMap<(usize, Key), ContextId>,
    /// Who read each context's return type.
    readers: Vec<BTreeSet<ContextId>>,
}

/// What the worklist may assume about the module.
pub(super) struct Inputs<'a> {
    pub(super) module: &'a TirModule,
    pub(super) body_index: &'a [Option<usize>],
    pub(super) sites: &'a [Vec<Site<'a>>],
    pub(super) result_sites: Vec<HashMap<ValueId, usize>>,
    pub(super) relevant: &'a [Vec<bool>],
    pub(super) participates: &'a [bool],
    pub(super) exact_plans: Vec<OnceCell<ExactScalarPlan<'a>>>,
    /// Whether a body may have contexts beyond its generic one.
    pub(super) specializable: &'a [bool],
}

impl Inputs<'_> {
    /// A shared entry executes one body. A proven positional argument can
    /// still determine an observed result without creating a new context.
    fn forwarded_result(
        &self,
        callee: usize,
        site: &Site<'_>,
        facts: &HashMap<ValueId, TirType>,
    ) -> Option<TirType> {
        if self.specializable[callee] || !site.result_demanded {
            return None;
        }
        let first = usize::from(self.module.functions[callee].has_closure_parameter());
        if !site
            .parameter_arguments(first)
            .any(|(_, value)| facts.get(&value).is_some_and(|ty| *ty != TirType::DynBox))
        {
            return None;
        }
        facts.get(&self.forwarded_argument(callee, site)?).cloned()
    }

    /// The actual whose Python value this invocation returns on every successful exit.
    pub(super) fn forwarded_argument(&self, callee: usize, site: &Site<'_>) -> Option<ValueId> {
        let body = &self.module.functions[callee];
        let first = usize::from(body.has_closure_parameter());
        if !returns_value(body) || body.is_codegen_partition() || body.param_names.len() <= first {
            return None;
        }
        let parameter = self.exact_plan(callee).forwarded_return_parameter()?;
        site.argument_for(parameter, first)
    }

    pub(super) fn exact_plan(&self, body: usize) -> &ExactScalarPlan<'_> {
        self.exact_plans[body].get_or_init(|| ExactScalarPlan::new(&self.module.functions[body]))
    }
}

impl Analysis {
    pub(super) fn generic(&self, body: usize) -> Option<ContextId> {
        self.index.get(&(body, Vec::new())).copied()
    }

    pub(super) fn context(&self, id: ContextId) -> &Context {
        &self.contexts[id.0]
    }

    /// Solve contexts that can change a resolved call or supply its result.
    pub(super) fn run(inputs: &Inputs<'_>) -> Self {
        let mut analysis = Analysis {
            contexts: Vec::new(),
            index: HashMap::default(),
            readers: Vec::new(),
        };
        let mut queue = VecDeque::new();
        for (body, &participates) in inputs.participates.iter().enumerate() {
            if participates {
                let id = analysis.intern(inputs, body, Vec::new());
                queue.push_back(id);
            }
        }
        let mut queued: BTreeSet<ContextId> = queue.iter().copied().collect();
        while let Some(id) = queue.pop_front() {
            queued.remove(&id);
            for next in analysis.evaluate(inputs, id) {
                if queued.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        analysis
    }

    /// Start at generic bodies, which can be entered by unseen callers.
    /// Final edges from unreachable contexts, including recursive cycles,
    /// contribute to the solved facts but cannot keep a clone executable.
    pub(super) fn reachable(&self) -> Vec<bool> {
        let mut reached = vec![false; self.contexts.len()];
        let mut pending = Vec::new();
        for (index, context) in self.contexts.iter().enumerate() {
            if context.key.is_empty() {
                reached[index] = true;
                pending.push(ContextId(index));
            }
        }
        while let Some(id) = pending.pop() {
            for target in self.context(id).targets.iter().flatten() {
                if !reached[target.0] {
                    reached[target.0] = true;
                    pending.push(*target);
                }
            }
        }
        reached
    }

    /// The context of `body` under `key`; a generic key is stored empty.
    fn intern(&mut self, inputs: &Inputs<'_>, body: usize, key: Key) -> ContextId {
        let key = if key.iter().all(|kind| *kind == ArgKind::Dyn) {
            Vec::new()
        } else {
            key
        };
        if let Some(&id) = self.index.get(&(body, key.clone())) {
            return id;
        }
        let id = ContextId(self.contexts.len());
        let sites = inputs.sites[body].len();
        self.contexts.push(Context {
            body,
            key: key.clone(),
            ret: TirType::Never,
            targets: vec![None; sites],
            results: vec![TirType::Never; sites],
        });
        self.readers.push(BTreeSet::new());
        self.index.insert((body, key), id);
        id
    }

    /// The exact facts of a context's body under its key and current call
    /// results.
    pub(super) fn facts(&self, inputs: &Inputs<'_>, id: ContextId) -> HashMap<ValueId, TirType> {
        let context = &self.contexts[id.0];
        let func = &inputs.module.functions[context.body];
        let params = seeds_of(&context.key, func.param_names.len());
        let sites = &inputs.result_sites[context.body];
        inputs.exact_plan(context.body).solve(ExactSeeds {
            params: &params,
            call_results: (!sites.is_empty()).then_some(ExactCallResults {
                sites,
                types: &context.results,
            }),
        })
    }

    /// Evaluate one context; return the contexts to evaluate next.
    fn evaluate(&mut self, inputs: &Inputs<'_>, id: ContextId) -> Vec<ContextId> {
        let mut next = Vec::new();
        let body = self.contexts[id.0].body;
        let facts = loop {
            let facts = self.facts(inputs, id);
            let mut moved = false;
            for (position, site) in inputs.sites[body].iter().enumerate() {
                let (target, ret) = match site.resolve(&facts) {
                    Resolution::Pending => continue,
                    Resolution::Opaque => (None, TirType::DynBox),
                    Resolution::Body(callee) => {
                        let callee = inputs.body_index[callee.index()]
                            .expect("a resolved site names a body");
                        let Some(key) = site_key(inputs, callee, site, &facts) else {
                            continue;
                        };
                        let fresh = self.contexts.len();
                        let target = self.intern(inputs, callee, key);
                        if self.contexts.len() != fresh {
                            next.push(target);
                        }
                        let ret = if site.result_demanded {
                            self.readers[target.0].insert(id);
                            inputs
                                .forwarded_result(callee, site, &facts)
                                .unwrap_or_else(|| self.contexts[target.0].ret.clone())
                        } else {
                            TirType::Never
                        };
                        (Some(target), ret)
                    }
                };
                let context = &mut self.contexts[id.0];
                context.targets[position] = target;
                if !site.result_demanded {
                    continue;
                }
                let result = join_return(&context.results[position], &ret);
                if result != context.results[position] {
                    context.results[position] = result;
                    moved = true;
                }
            }
            if !moved {
                break facts;
            }
        };
        let func = &inputs.module.functions[body];
        let ret = if returns_value(func) && !inputs.exact_plan(body).has_non_value_return() {
            inputs
                .exact_plan(body)
                .return_values()
                .iter()
                .fold(TirType::Never, |ret, value| {
                    join_return(&ret, facts.get(value).unwrap_or(&TirType::DynBox))
                })
        } else {
            TirType::DynBox
        };
        let context = &mut self.contexts[id.0];
        let joined = join_return(&context.ret, &ret);
        if joined != context.ret {
            context.ret = joined;
            next.extend(self.readers[id.0].iter().copied());
        }
        next
    }
}

/// One exact seed per parameter for `key` (empty means generic).
pub(super) fn seeds_of(key: &Key, params: usize) -> Vec<TirType> {
    (0..params)
        .map(|index| key.get(index).copied().unwrap_or(ArgKind::Dyn).seed())
        .collect()
}

/// The key a site's argument facts select in its callee, or `None` while an
/// argument the key reads is still bottom.
fn site_key(
    inputs: &Inputs<'_>,
    callee: usize,
    site: &Site<'_>,
    facts: &HashMap<ValueId, TirType>,
) -> Option<Key> {
    if !inputs.specializable[callee] {
        return Some(Vec::new());
    }
    let relevant = &inputs.relevant[callee];
    let first = usize::from(inputs.module.functions[callee].has_closure_parameter());
    // Captured cells can change even when the executable target is unique.
    let mut key = Vec::with_capacity(relevant.len());
    key.resize(first, ArgKind::Dyn);
    for (position, arg) in site.parameter_arguments(first) {
        let kind = if relevant[position] {
            ArgKind::of(facts.get(&arg).unwrap_or(&TirType::DynBox))?
        } else {
            ArgKind::Dyn
        };
        key.push(kind);
    }
    Some(key)
}
