# Typed identity, state machines and formalization

**Status:** standing engineering rule (owner, 2026-10-29: "Typing and state
machines and formalization are our friends"). AGENTS.md states it in one line
and points here. Every subsystem design applies it before code.

## Why

Most open findings trace to one of three missing structures. A missing type
lets a stale consumer compile (string op kinds, name-matched exceptions,
untyped optimization gates). A missing state machine lets a reader see a
half-finished state (HF-583: image selection read a provisioning in flight).
A missing proof lets a rewrite or a pass drift from the semantics (HF-214
unchecked rewrites, HF-573 passes that never fire, HF-572's prose exit
condition). Each structure turns a class of defects into a compile error, an
unrepresentable state or a failed check.

## Typed identity

One declarative source generates a closed type; exhaustive Rust matches and
`Literal` types under `ty` reject a stale consumer. A source-scraping audit is
a bridge: retire it when the type enforces the same fact.

| Identity | Source | State |
| --- | --- | --- |
| Op kinds | `runtime/molt-ir/src/tir/op_kinds.toml` | Rust `OpKind` (HF-197 step 4a) and Python `FrontendOpKind`, `WireOpKind` (step 4b) landed |
| Runtime functions | [Runtime-call contract](77_architecture_convergence.md#generated-runtime-call-contract-hf-485-hf-384-hf-571) | Boxed `RuntimeFn` and staged `RuntimeCall` generated; remaining families/imports follow the contract's stages |
| Exceptions | `runtime/exception_kinds.toml` | `BuiltinExc`, `ModuleExc`, `ExcKind` landed (HF-207); remaining edges HF-560 |
| Op facts and attribute keys | op-kind registry | open (HF-202) |
| Decorators | binding flow | open (HF-203) |
| Findings | ledger, `tests/findings/finding_ids.py` | `OpenFindingId`, `FixedFindingId` landed |
| Capability tiers | `runtime/host_capabilities.toml` | generated tier table landed |
| Environment variables | `src/molt/environment_registry.toml` | registry landed |
| Storage classes | storage authority | planned (HF-582) |
| Optimization gates | gate names its owning `OpenFindingId` | proposed (family P) |

Prior art: MLIR ODS generates op classes; Cranelift's meta crate generates
`Opcode`; CPython generates `Include/internal/pycore_opcode_metadata.h` and
`Lib/_opcode_metadata.py` from `Python/bytecodes.c`; rustc matches closed MIR
enums.

## Typed state machines

A lifecycle moves through explicit states by exhaustive, atomic, recorded
transitions. An invalid state cannot be represented; a reader sees only a
published state; a transition happens under the lock that owns the state.

| Lifecycle | States today | Owner |
| --- | --- | --- |
| Lane | coordination record status | `tools/molt_dev.py lane`, family R |
| Landing request | queue result files | landing queue, family R |
| Development image | provisioning record, selection record | HF-583 |
| Process custody | guard records | V1-12, family R |
| Generator and coroutine frames | typed suspension slots | `docs/design/typed_suspension_state_machines.md` |
| Runtime initialization | frozen at Ready | runtime |
| Storage retention | none yet | HF-582 |
| Release | release exit, H0, publication | `tools/release_exit_gate.py` |

Prior art: Rust's typestate pattern and exhaustive enums; Erlang/OTP
`gen_statem`; Harel statecharts; TLA+ specifications model-checked for
distributed protocols; CPython 3.11+ frame states (`FRAME_CREATED`,
`FRAME_SUSPENDED`, `FRAME_EXECUTING`, `FRAME_COMPLETED`, `FRAME_CLEARED`).

## Formalization

A semantics-bearing transformation states its preconditions as checked facts
and proves itself against the formal model, not by example alone.

- Semantics: the Lean model under `formal/lean`.
- Passes: translation validation (`tools/translation_validate.py`) checks a
  rewrite's input and output against that model.
- Scope: the verified subset
  (`docs/spec/areas/compat/contracts/verified_subset_contract.md`) bounds what
  the model must cover.
- Search: differential fuzzing against CPython (HF-95) derives programs from
  the subset and reduces each failure to a typed finding test.

Prior art: CompCert's verified compiler passes, Alive2's translation validation
for LLVM, CakeML, seL4, and Csmith and YARPGen for differential fuzzing.

## How a design applies this

Before code, a design names: the closed types it adds or extends and their
source; each lifecycle it touches, as a state table with its transitions and
lock; and the precondition or semantic property each transformation must keep,
with the check that proves it. Prior art per findings family is in
[findings_prior_art.md](../findings_prior_art.md).
