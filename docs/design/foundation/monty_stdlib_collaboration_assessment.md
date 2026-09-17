# Monty (Pydantic): Rust reuse and collaboration assessment

Reviewed 2026-09-17 against upstream revision
`d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a`.

## Decision

Selective reuse of Monty Rust code is welcome when it improves Molt's existing
implementation. Evaluate a routine, algorithm or test corpus against its actual
contract; neither a blanket ban nor wholesale VM adoption is appropriate.
Integrate accepted work into the owning Molt authority and remove replaced
implementations. Do not create a second object model, permission registry,
resource tracker, clock policy or stdlib lane.

This supersedes the June 2026 assessment's blanket "borrow crate choices, not
code" recommendation and its version-specific coverage comparisons. Those
observations are not evidence of current Monty or Molt support.

## Current upstream boundary

Monty is a sandboxed Python interpreter in Rust. Its README describes host access
through supplied functions and mounts, VM resource limits, and serializable
pause/resume. Its current limitations document aims for CPython **3.14** behavior
inside its subset while explicitly recording divergences. It now describes
simple classes and context managers; the earlier absence claims are obsolete.

That is useful engineering material, not evidence of Molt's CPython >=3.12,
native/WASM, OS/architecture, C-API or ecosystem conformance. Inspect the pinned
implementation and its relevant limitations before importing it. Differences in
representation, ownership, exception order and capability policy remain Molt's
responsibility.

## Findings and canonical integration seams

- **Text splitting:** the inspected `crates/monty/src/types/str.rs` uses Rust
  `str`, VM-owned values and separately implemented left/right whitespace
  splitting. Its `str_splitlines` scans CR/LF, unlike CPython's broader string
  line-boundary set. Do not replace Molt's shared WTF-8 split/predicate authority
  with these routines. Molt's relevant owners are
  `runtime/molt-runtime/src/builtins/strings.rs` and
  `runtime/molt-runtime/src/object/ops_string*.rs`. This is a source comparison,
  not a benchmark or a claim that either complete string surface is verified.
- **Target-aware clocks:** Monty's `crates/monty-types/src/resource.rs` selects
  `web_time::Instant` for `wasm32-unknown-unknown` and standard `Instant`
  elsewhere. Molt's `runtime/molt-runtime-resource/src/lib.rs::LimitedTracker`
  constructs a standard `Instant` unconditionally; scheduler timing also uses
  standard `Instant`. This identifies a static portability risk, not an
  executed failure. Reuse the target-aware design through Molt's existing host
  clock contract; importing a browser-only clock dependency would not establish
  compatibility with custom WASM hosts.
- **Replacement-size estimation:** Monty's
  `crates/monty/src/resource_checks.rs::check_replace_size` bounds empty-pattern
  insertions by the requested count. Molt's
  `OperationEstimate::StringReplace` in the existing resource crate ignores
  count for an empty pattern: `input_len=3, new_len=2, count=0` estimates 11,
  rather than 3, bytes. The inspected runtime has no production callers for
  these operation checks. This is an estimator discrepancy, not a demonstrated
  user-visible limit failure. Any adoption must connect the existing authority
  to real consumers and preserve Molt's checked-overflow behavior; copying more
  unused helpers would not establish enforcement.

These are assessment findings, not implementation or support claims. No Monty
code or dependency was imported by this review. Keep subsequent implementation
and proof state in the owning source, capability contract and conformance
authority, not a parallel Monty-specific backlog.

## Reuse requirements

1. Pin repository revision and source paths. Preserve the MIT copyright and
   permission notice in copied or substantial adapted portions and their
   distribution notices. The upstream notice names Pydantic Services Inc.,
   2026 to present; Molt's own license declaration does not replace it.
2. Check file-specific provenance and any third-party notices. Read the relevant
   upstream limitations, tests and dependencies at that same revision.
3. Map the routine to Molt's existing owner and migrate its sibling consumers.
   Do not import Monty's VM types, heap or policy machinery merely to obtain
   an isolated algorithm.
4. Prove the changed contract against the version-gated CPython oracle and the
   applicable native/WASM consumers. Include exception/cleanup behavior,
   overflow, Unicode/surrogates and resource failure where relevant.
5. For an optimization, measure the affected runtime, allocation, binary-size
   and build-cost tradeoffs. A permissive license or upstream speed claim is
   not local performance evidence.

Upstream collaboration is welcome, but publishing issues or contributions still
requires authorization. A shared external crate is justified by an actual
reusable boundary and maintenance case, not by this assessment alone.

## Pinned sources

- [License](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/LICENSE)
  and [README](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/README.md).
- [Subset and divergences](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/docs/limitations/index.md).
- [String implementation](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/crates/monty/src/types/str.rs).
- [Resource tracker](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/crates/monty-types/src/resource.rs)
  and [operation checks](https://github.com/pydantic/monty/blob/d5b7e0e0a20ac79c77823de3de3e7f0ef9f3083a/crates/monty/src/resource_checks.rs).
