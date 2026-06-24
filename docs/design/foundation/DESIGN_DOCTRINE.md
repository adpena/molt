<!-- Foundation doctrine. The cross-cutting design principles every plan (21a-21e, 50-59,
and all future work) must serve. Authored 2026-06-24. Binding. -->

# Design Doctrine — God-Files Are Killers; Design Pythonista-Rustacean

This doctrine governs the entire 100-year-plan portfolio. Every blueprint (the decomposition
21a-21e, the perf/compat/throughput/DX/UX/demos/fact-plane plans 50-59, and all future work)
must be checkable against BOTH principles below.

## 1. God-files are dev-velocity AND compiler killers (release-blocking, not cleanup)

A god-file (a large file mixing many concerns) is a DUAL killer, not a style nit:

- **Dev-velocity killer.** It is the #1 ownership-collision source — every dev/agent who
  touches the subsystem touches the same file, forcing merge contention and serialized work
  (observed repeatedly this program). Its edit blast-radius is the whole file: a change to one
  concern risks every other. It is unreviewable at size. It is the single biggest brake on
  parallel development.
- **Compiler killer.** A giant FUNCTION is rustc's ATOMIC codegen unit — a ~22K-line
  `compile_func_inner` is ONE serial codegen unit that `codegen-units=256` cannot split,
  serializing MIR-build + borrow-check + LLVM (see 21a; the FILE-split was refused precisely
  because it buys ~0 here — only the FUNCTION-split parallelizes codegen). A giant CRATE forces
  whole-crate recompiles, defeating incremental builds (a TIR-pass edit rebuilding all 5
  backends — see 21b). The blast-radius serializes the build pipeline.

**Therefore:** god-file elimination is RELEASE-BLOCKING. The `structural_audit` ratchet
(`god_files`, `max_god_file_lines`) enforces it monotonically. The decomposition program is the
spine, and each split targets a specific killer: **21a function-split** → the codegen-unit
killer; **21b crate-split** (molt-ir ← molt-passes ← molt-lower, per-backend crates) → the
incremental-build killer; **21c/21d package-splits** (frontend mixins, cli/ package) → the
ownership-collision killer; **21e satellite dedup** → the dual-maintenance killer. Every other
plan in the portfolio must REDUCE god-files (or, with explicit justification, not grow them).

> Metric note (see 59): the current `god_files` COUNT metric penalizes correct decomposition
> (cohesive `fc/` family handlers, `cli/` and `visitors/` modules each count as "god-files"),
> so the ratchet can never green even when decomposition is correct. 59 must fix this by
> crediting cohesive decomposition products / tracking max + concern-mixing rather than raw
> count-over-threshold — WITHOUT hiding real kitchen-sink debt and WITHOUT re-pinning the
> baseline. A god-file is a CONCERN-MIXING file, not merely a large cohesive one.

## 2. Design as a Pythonista Rustacean

molt is **Python's semantics + ergonomics delivered with Rust's systems rigor.** Every design
must satisfy BOTH lenses — they compose, they are not in tension:

- **Pythonista lens.** Exact CPython >=3.12 semantics (the parity contract, edge/corner cases,
  all backends — see 52). Pythonic ergonomics + UX: errors a Python developer understands
  (CPython-parity tracebacks, and better — see 57); the Python mental model preserved; it is
  *still Python*, just faster. A feature that is fast but semantically divergent or un-Pythonic
  FAILS. The user is a Pythonista who must feel at home.
- **Rustacean lens.** Fix the REPRESENTATION, not the symptom (the compression ladder — see 51:
  RC→ownership/borrow, dispatch→class-identity/version, boxing→Repr precision, loops→induction/
  range/lane). Zero-cost abstractions. Ownership/borrow soundness made STRUCTURAL, not hoped-for
  (the ownership lattice — memory safety is a proof, not a wish; see 55). One authority per
  invariant; exhaustive-match-gated so drift is uncompilable (the fact plane — see 59). A
  feature that is Pythonic but representation-sloppy or unsound FAILS.

**The synthesis (the whole project):** Python's dynamism made statically knowable via Rust-grade
IR FACTS — class identity/version, Repr precision, ownership state, lifetime boundaries — so the
Pythonista keeps their exact semantics and ergonomics while the Rustacean delivers C/native
performance and provable safety. Dynamism is not erased; it is *represented precisely enough* to
compile away its cost when the facts allow, and to fall back soundly when they don't. Every arc
plan is judged on whether it advances both lenses at once.

## How to apply (the checklist every plan + PR answers)
1. Does it reduce (or at least not grow) concern-mixing god-files? Which killer does it retire?
2. Does it preserve exact CPython semantics AND feel Pythonic to use?
3. Does it fix a REPRESENTATION (add an IR fact that retires a class), not patch a symptom?
4. Is the invariant carried by ONE generated/checkable authority (drift uncompilable)?
5. Is memory safety structural (a lattice proof), not hoped-for?
6. Is the win measured across the full matrix (target × backend × profile vs CPython/PyPy/Codon)?
