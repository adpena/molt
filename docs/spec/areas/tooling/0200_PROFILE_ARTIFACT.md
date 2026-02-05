# Molt Profile Artifact (MPA) v0.1
**Status:** Draft
**Goal:** A portable artifact produced by running a program under CPython that captures dynamic facts useful for Molt’s semantic reduction and specialization.

## Design principles
- Observations are not proofs: use as hints or guard + deopt.
- Portable/reproducible: no pointer addresses; stable hashes.
- Tier-aware: Tier 0 mostly for hotspots; Tier 1 for speculation + guards.

## Format (JSON)
Recommended file: `molt_profile.json`

Top-level:
```json
{
  "molt_profile_version": "0.1",
  "created_at_utc": "2026-01-02T00:00:00Z",
  "python_implementation": "CPython",
  "python_version": "3.13.0",
  "platform": {"os":"linux|darwin","arch":"x86_64|arm64"},
  "run_metadata": {
    "entrypoint":"module:function|script.py",
    "argv":[],
    "env_fingerprint":"sha256:...",
    "inputs_fingerprint":"sha256:...",
    "duration_ms": 0
  },
  "modules": {},
  "symbols": {},
  "call_sites": [],
  "types": {},
  "containers": {},
  "hotspots": [],
  "events": [],
  "redactions": {}
}
```

## Collected facts (minimum useful set)
- Modules: kind (py/builtin/extension), file, module attribute mutations observed
- Symbols: qualified name, code hash, observed effects, observed exceptions
- Call sites: observed targets, arg/ret type tuples, exception counts
- Types: nominal identity + observed attrs + mutation markers
- Containers: dict key sets observed; list homogeneity; length ranges
- Hotspots: time + allocations ranking

## Collection mechanisms (recommended)
- `sys.setprofile` / `sys.settrace` for call graph + call-site targets
- `cProfile` for time
- Optional allocation sampling
- Stable IDs via sha256(file:line:col:expr) and sha256(co_code)

## Safety rules for consumption
- Every optimization derived from profile observations must be guarded (Tier 1) or rejected (Tier 0).
- Never assume absence of exceptions unless proven or guarded.
- Provide redaction: hash-only strings by default.

## Minimal workflow
1) `molt profile -- python -m app ...` → `molt_profile.json`
2) `molt build --pgo-profile molt_profile.json` → native binary
3) runtime optionally emits `molt_runtime_feedback.json` to refine future profiles (TODO(tooling, owner:tooling, milestone:TL2, priority:P2, status:planned): runtime feedback emission).
