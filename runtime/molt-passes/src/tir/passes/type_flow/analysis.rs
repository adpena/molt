//! The context worklist (design 78, section 3.3).
//!
//! A context is a body and one key: an [`ArgKind`] per parameter, `Dyn` for
//! every parameter that is not relevant or not proven. Every body has its
//! generic context (all `Dyn`), which every unseen caller enters. Evaluating
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

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirModule;
use crate::tir::type_refine::{ExactSeeds, solve_exact_scalars};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::sites::{ArgKind, Site, join_return, returns_value};

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
    pub(super) sites: &'a [Vec<Site>],
    pub(super) relevant: &'a [Vec<bool>],
    /// Whether a body may have contexts beyond its generic one.
    pub(super) specializable: &'a [bool],
}

impl Analysis {
    pub(super) fn generic(&self, body: usize) -> ContextId {
        self.index[&(body, Vec::new())]
    }

    pub(super) fn context(&self, id: ContextId) -> &Context {
        &self.contexts[id.0]
    }

    /// Solve every context reachable from the generic contexts.
    pub(super) fn run(inputs: &Inputs<'_>) -> Self {
        let mut analysis = Analysis {
            contexts: Vec::new(),
            index: HashMap::new(),
            readers: Vec::new(),
        };
        let mut queue = VecDeque::new();
        for body in 0..inputs.module.functions.len() {
            let id = analysis.intern(inputs, body, Vec::new());
            queue.push_back(id);
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
        let results: HashMap<(BlockId, usize), TirType> = inputs.sites[context.body]
            .iter()
            .zip(&context.results)
            .map(|(site, ty)| (site.position(), ty.clone()))
            .collect();
        solve_exact_scalars(
            func,
            ExactSeeds {
                params: &params,
                call_results: Some(&results),
            },
        )
    }

    /// Evaluate one context; return the contexts to evaluate next.
    fn evaluate(&mut self, inputs: &Inputs<'_>, id: ContextId) -> Vec<ContextId> {
        let mut next = Vec::new();
        let body = self.contexts[id.0].body;
        let facts = loop {
            let facts = self.facts(inputs, id);
            let mut moved = false;
            for (position, site) in inputs.sites[body].iter().enumerate() {
                let Some(key) = site_key(inputs, site, &facts) else {
                    continue;
                };
                let callee =
                    inputs.body_index[site.callee.index()].expect("a resolved site names a body");
                let fresh = self.index.len();
                let target = self.intern(inputs, callee, key);
                if self.index.len() != fresh {
                    next.push(target);
                }
                self.readers[target.0].insert(id);
                let ret = self.contexts[target.0].ret.clone();
                let context = &mut self.contexts[id.0];
                context.targets[position] = Some(target);
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
        let ret = if returns_value(func) {
            return_type(func, &facts)
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
fn site_key(inputs: &Inputs<'_>, site: &Site, facts: &HashMap<ValueId, TirType>) -> Option<Key> {
    let callee = inputs.body_index[site.callee.index()]?;
    if !inputs.specializable[callee] {
        return Some(Vec::new());
    }
    let relevant = &inputs.relevant[callee];
    site.args
        .iter()
        .enumerate()
        .map(|(position, arg)| {
            if !relevant.get(position).copied().unwrap_or(true) {
                return Some(ArgKind::Dyn);
            }
            ArgKind::of(facts.get(arg).unwrap_or(&TirType::DynBox))
        })
        .collect()
}

/// The join of the exact facts of every value a reachable `Return` returns.
fn return_type(
    func: &crate::tir::function::TirFunction,
    facts: &HashMap<ValueId, TirType>,
) -> TirType {
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let mut ret = TirType::Never;
    for block in func.blocks.values() {
        if !reachable.contains(&block.id) {
            continue;
        }
        if let Terminator::Return { values } = &block.terminator {
            for value in values {
                let fact = facts.get(value).cloned().unwrap_or(TirType::DynBox);
                ret = join_return(&ret, &fact);
            }
        }
    }
    ret
}
