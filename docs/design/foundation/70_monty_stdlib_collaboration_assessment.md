<!-- Foundation decision doc 70. Assessment of Monty (Pydantic) for stdlib/Rust code
reuse + collaboration. Verdict from a 2026-06-24 research sweep. Governed by
DESIGN_DOCTRINE.md + the zero-silent-divergence parity rule (CLAUDE.md). -->

# 70 — Monty (Pydantic): stdlib/Rust reuse + collaboration assessment

> **Verdict: do NOT vendor Monty stdlib code. The asymmetry runs in molt's favor —
> molt's parity-tested Rust stdlib is the asset; Monty is the potential beneficiary.**
> Borrow upstream *crate choices*, not Monty code; pursue a molt→Monty collaboration,
> not a dependency.

## What Monty is
`github.com/pydantic/monty` (MIT, Pydantic Services Inc., **v0.0.18**, experimental): a
from-scratch **secure Python bytecode VM in Rust** for running LLM-generated agent code
in-process (µs startup, KB snapshots; `open`/`eval`/`exec` simply absent). Ruff frontend
(`ruff_python_parser`), reference-counted **tagged-enum `Value`** + paged `Heap` +
Bacon–Rajan cycle collection. Targets Python 3.14. A fundamentally different product
from molt (an AOT compiler chasing exact CPython ≥3.12 parity, NaN-boxed values).

## Why off-the-shelf reuse is not viable (three independent reasons)
1. **Semantic-contract mismatch (fatal).** Monty *explicitly does not commit to exact
   CPython parity* — it maintains a 24-file `limitations/` directory of **deliberate,
   possibly permanent** divergences (`round(x, n)` rejects float `n`; `sorted` forces
   keyword-only `key`; two host callables with the same `__name__` compare identity-equal;
   genexprs materialize to lists; module `__annotations__` always empty; …). Importing
   their code imports their divergences — a direct violation of molt's #1 rule.
2. **Object-model coupling (fatal for direct reuse).** Every stdlib fn is bound to
   `VM<'_, impl ResourceTracker>` / `ArgValues` / `RunResult<Value>` / paged `Heap` with
   a **tagged-enum `Value` (not NaN-boxed)** — architecturally incompatible. You re-port,
   not reuse.
3. **Maturity / coverage.** v0.0.18: no classes, no `match`, no context managers; **~10
   stdlib modules** (`math`, `re` (on `fancy-regex`), `json` (on `jiter`), `datetime` (on
   `chrono`/`speedate`), `os`, `pathlib`, `sys`, `typing`, `gc`, `asyncio`). molt already
   ships **100+** parity-tested stdlib modules in Rust.

## Decision
1. **Do NOT vendor Monty stdlib code.** Coupling + designed divergences make it a parity
   liability; molt is far ahead on coverage anyway.
2. **Borrow upstream *crate choices*, not code, where molt has gaps** — evaluate `jiter`
   (JSON), `speedate` (date parsing), `num-bigint`, and the **Ruff** frontend directly
   from their MIT/Apache upstreams, on molt's own parity terms. **Avoid `fancy-regex`** for
   `re` (not CPython-`sre`-faithful → parity risk).
3. **Clean MIT path** if a single isolated, semantically-CPython-faithful routine is ever
   worth lifting: retain the notice, read that module's `limitations/*.md` first as the
   expected-divergence map, and run it through molt's **differential oracle** before
   landing. In practice "borrow then verify" is costly enough that re-implementing
   against the oracle is usually cheaper.
4. **Pursue the molt→Monty direction.** A jointly-owned, **VM-agnostic `pystdlib-core`
   crate** (pure CPython-exact algorithms generic over a thin value/host trait, molt's
   differential oracle as the shared parity gate) is *viable but multi-quarter*. Seed the
   relationship first with low-risk parity issues against Monty's `limitations/` and by
   offering molt's differential test corpus for a module they ship — gauge receptiveness
   before any shared-crate investment. (Outward-facing contributions are owner-gated; get
   explicit sign-off before filing.)
5. **Revisit only if** Monty adopts an exact-parity stance, or a joint `pystdlib-core`
   stands up with molt's oracle as the gate.

## Useful as a reference (not a dependency)
Subprocess-pool isolation, resource-tracker accounting, the Ruff frontend choice, and the
`limitations/` discipline are good architecture references. Monty is an excellent
secure-sandbox product; it is simply not a stdlib *source* for molt's exact-parity AOT
runtime today.

*Sources: github.com/pydantic/monty (LICENSE, README, CLAUDE.md, crates/, src/modules/,
limitations/, issues); pydantic.dev/articles/pydantic-monty; Simon Willison 2026-02-06;
Talk Python ep. 541. Full research artifact in the 2026-06-24 session transcript.*
