# Type Facts Artifact: `ty` Integration for Molt Optimization
**Spec ID:** 0920
**Status:** Draft (design + implementation plan)
**Audience:** Compiler engineers, tooling engineers

## Purpose
Define how Molt consumes static type analysis (via `ty`, a Rust-based Python type checker)
to produce a **Type Facts Artifact (TFA)** used for specialization, representation lowering,
and guard elimination.

## Core Idea
Types do not make Python fast by themselves.
They become valuable only when the compiler uses them to:
- specialize hot paths
- remove dynamic checks
- choose unboxed representations
- reduce deoptimization frequency

The TFA is the machine-readable contract that enables this.

## Inputs
- Python source type hints (PEP 484/526)
- `.pyi` stubs
- typeshed
- project configuration (strict vs permissive)
- optional runtime profiling hints (future) (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): runtime profiling hints in TFA).

## Output: Type Facts Artifact
A deterministic artifact emitted by `molt check`:
- `type_facts.json` (or MsgPack)
- keyed by source hash, lockfile hash, target triple, compiler version

### Example facts
- symbol kind (function/class/value)
- declared type
- inferred type
- trust level: advisory | guarded | trusted
- justification (why trusted)

### Schema (v1, JSON)
```json
{
  "schema_version": 1,
  "created_at": "2026-01-04T02:22:19.734771+00:00",
  "tool": "molt-check+ty",
  "strict": false,
  "modules": {
    "example": {
      "globals": {
        "nums": { "type": "list[int]", "trust": "guarded" }
      },
      "functions": {
        "foo": {
          "params": { "x": { "type": "int", "trust": "guarded" } },
          "locals": { "total": { "type": "int", "trust": "guarded" } },
          "returns": { "type": "int", "trust": "guarded" }
        }
      }
    }
  }
}
```

### Consumption Rules
- `molt build --type-facts <path>` loads TFA facts for specialization.
- `--type-hints=check` consumes `guarded` or `trusted` facts with runtime guards.
- `--type-hints=trust` consumes only `trusted` facts (no guards).

## Trust Model
- **Advisory:** hints only, no semantic changes
- **Guarded:** optimized with runtime guards + deopt
- **Trusted:** no guards; only allowed in strict tier

## Compiler Usage
- MIR specialization
- unboxed locals and fields
- struct-like layouts for closed shapes
- reduced guard density
- element-type-driven fast paths (e.g., `list[int]` enables `VecSumIntTrusted` reductions)

## Tooling
- `molt check` generates TFA
- `molt build` consumes it
- cacheable and reproducible
- when `ty` is available, `molt check` runs it as a validator before trusting facts
- current `ty` CLI does not expose inferred types; TFA is sourced from annotations

## Non-Goals
- Full soundness of Python typing
- Global inference for dynamic programs
- IDE replacement

The TFA exists purely to unlock performance safely.
