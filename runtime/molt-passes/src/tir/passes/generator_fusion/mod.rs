//! Generator frame-elision fusion — Tier-B (doc 26 Phase 1, the D1 blueprint
//! `07_D1-coroelide.md`).
//!
//! This is a **module** transform (it needs the consumer caller AND the
//! generator `_poll` body simultaneously), run from
//! [`run_module_pipeline`](crate::tir::module_phase::run_module_pipeline) AFTER
//! the E1 inliner. It recognizes the shape
//!
//! ```text
//!   g = AllocTask(task_kind="generator", poll=P, closure_size=N)   // in caller
//!   it = GetIter(g)                                                // single use
//!   loop { pair = IterNext(it); done = pair[1]; if done break;     // single use
//!          elem = pair[0]; <consumer body using elem> }
//! ```
//!
//! and **splices** `P`'s body into the caller, eliminating the heap frame
//! (`AllocTask` → `molt_task_new`), the per-yield `(value, done)` pair tuple,
//! the indirect `_poll` call, and the `STATE_SWITCH` dispatch. The generator's
//! own control flow becomes the fused loop; each `STATE_YIELD(pair)` binds the
//! element directly to the consumer's for-target and runs the consumer body
//! inline.
//!
//! ## What the splice actually rebuilds
//!
//! A generator `_poll` lowers to a **linear / structured** TIR body: a
//! `state_switch` marker op, then code interleaved with `state_yield(pair,
//! next_state)` ops, with the resume-after-yield being the *fall-through* (the
//! state dispatch CFG that the native/LLVM backends reconstruct from the
//! `next_state` ids does NOT exist as TIR edges). The frame slots are MEMORY:
//! `closure_load(self, offset)` / `closure_store(self, offset, v)` where
//! `offset < GEN_CONTROL_BYTES` (48) are the control slots (send=0, throw=8,
//! closed=16) and `offset >= 48` are the generator's captured params + spilled
//! locals.
//!
//! The fused form is the generator's own control flow, with the consumer body
//! spliced in at the yield and the frame promoted to SSA (`slots.rs`):
//!
//! ```text
//!   preheader: p_i = <the frame's reference to generator argument i>
//!       ... cloned P ops: closure_load(slot) -> a copy of the slot's reaching
//!           definition; closure_store(slot, v) -> v is the slot's new
//!           definition; a join its stores reach takes the slot as a block
//!           argument ...
//!   yield site (P's yield block, up to the yield):
//!       elem = pair[0]; br consumer
//!   consumer:
//!       <original consumer body using elem>
//!       br post_yield            // continue
//!       (or br loop_exit on break)
//!   post_yield: <P's code after the yield>
//!   exhausted return: br loop_exit
//! ```
//!
//! The control slots (send/throw/closed) are eliminated: the recognition
//! predicate proves no `.send()`/`.throw()`/`.close()` can reach this generator
//! (the object never escapes the single `GetIter` use), so every send-slot read
//! is dead and every throw-slot read is `None`; the throw-injection `raise`
//! folds away under the re-run `run_pipeline` (SCCP proves `None is not None`).
//!
//! ## Soundness
//!
//! Conservative-correct by construction: every recognition gate that is not met
//! leaves the IR byte-identical (the generator stays Tier D — heap frame +
//! runtime `molt_generator_send`, which is correct and preserved). The splice is
//! followed by `verify_function` and a `run_pipeline` re-run (which itself
//! verifies).
//!
//! Fusion runs before the terminal drop plane and places no reference
//! operation: DropInsertion owns every reference of the fused body (design 20
//! §1.2). The element is the owned result of `Index(pair, 0)`, as the eliminated
//! `IterNext` pair's element was. Each promoted read replaces a `ClosureLoad`,
//! whose result was owned, and keeps that reference through
//! `ownership_lattice_min::Replacements`. So do the frame's own references: each
//! promoted store's to the value it stored, and each parameter slot's to its
//! argument. A join argument owns what its edges move or retain into it.
//!
//! Phase 1 scope (doc 26): single- and multi-yield generators with no
//! `YieldFrom`, no real exception HANDLER region (`has_exception_handlers()`),
//! no `.send`/`.throw`/`.close`, single non-escaping `AllocTask` instance. See
//! the bail table in [`collect_fusion_candidates`] / [`is_poll_fusable`].

mod attrs;
mod clone;
mod driver;
mod recognize;
mod slots;
mod splice;
mod types;
mod wire;

#[cfg(test)]
mod tests;

pub use self::driver::run_generator_fusion;
pub use self::types::FusionStats;

pub(in crate::tir::passes::generator_fusion) use self::attrs::{
    attr_original_kind, attr_value_int, is_get_iter_op,
};
pub(in crate::tir::passes::generator_fusion) use self::attrs::{attr_s_value, attr_task_kind};
pub(in crate::tir::passes::generator_fusion) use self::recognize::{
    collect_fusion_candidates, is_poll_fusable,
};
pub(in crate::tir::passes::generator_fusion) use self::splice::apply_fusion;
pub(in crate::tir::passes::generator_fusion) use self::types::FusionCandidate;

/// Byte size of the generator control header. Frame offsets `< GEN_CONTROL_BYTES`
/// are the control slots — `GEN_SEND_OFFSET=0` (the `.send()` value),
/// `GEN_THROW_OFFSET=8` (the pending `.throw()` exception), `GEN_CLOSED_OFFSET=16`
/// (the exhausted flag), `GEN_YIELD_FROM_OFFSET=32` (the delegation target);
/// offsets `>= GEN_CONTROL_BYTES` are the generator's captured params + spilled
/// locals. Mirrors `GEN_CONTROL_SIZE` in `src/molt/frontend/_types.py` and
/// `crate::GENERATOR_CONTROL_BYTES`.
pub(super) const GEN_CONTROL_BYTES: i64 = 48;
