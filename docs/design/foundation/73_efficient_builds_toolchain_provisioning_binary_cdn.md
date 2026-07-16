# 73 — Efficient 8GB Builds, Auto Toolchain Provisioning, and a Precompiled Binary CDN

Status: binding architecture directive, 2026-07-06 (operator).

This document turns three operator directives into non-negotiable Molt
engineering obligations, each with a self-protecting gate. It exists because a
long session repeatedly hit — and naively mis-attributed — build heaviness and
missing extension toolchains as "host limits," when they are **dumb naive Molt
implementations**. The obligation shape: one authority, one cached artifact,
one provisioning path, one released binary, each proven by a gate that fails
closed.

## Root doctrine (the reframe)

- **Molt must build on an 8GB machine.** A WASM build that takes ~45 minutes and
  thrashes on a 32GB box is a naive implementation, not a hardware limit. Memory
  and time are correctness budgets, gated — not excuses.
- **Molt owns the ecosystem toolchain.** When a program imports a library that
  ships C/Cython/Fortran extensions (numpy, scipy, …), Molt is responsible for
  knowing, provisioning, and configuring the toolchain that library's build
  needs. The user runs `molt build`; Molt does the rest. "Set MOLT_WASI_SYSROOT
  by hand" and "stage scipy._cyutility manually" are provisioning defects.
- **Recompiling a complex library locally is the fallback, not the default.**
  Once Molt proves a library's source-recompile works and is bit-parity-tested,
  the tested artifact is published and fetched, not rebuilt on every machine.

## R73.1 — Efficient, 8GB-capable builds (one shared runtime-artifact cache)

**The naive root:** `molt_runtime.wasm` (the entire Molt runtime crate compiled
to wasm32) is a FIXED artifact: it depends only on runtime source, target, ABI,
and profile, NOT on the user program. Earlier developer flows put ordinary
builds under per-session Cargo targets, so every fresh session/agent could
recompile the runtime crate cold (~45 min), and the memory-guard timeout could
quarantine incremental state — an infinite cold-rebuild loop. The live DX
resolver now keeps ordinary builds on the persistent selected-root target and
reserves `target/sessions/<id>` for explicit isolation lanes; the runtime cache
below remains the cross-session artifact authority.

**Obligation:**
1. `molt_runtime.wasm` is built ONCE and reused across sessions, cached
   content-addressed on `(runtime_source_hash, target, abi, profile)` in the
   SHARED cache (MOLT_CACHE / C:\Molt\.molt_cache), NOT the per-session target
   dir. A fresh session finds and reuses the warm runtime instead of recompiling.
   The app.wasm (per-program) stays session-scoped.
2. Cargo parallelism is bounded to available memory (cap `--jobs` /
   `codegen-units` by a memory budget), so a wasm build fits an 8GB ceiling
   instead of thrashing. Default to a memory-derived job count, not `num_cpus`.
3. Timeout-quarantine must never destroy the shared runtime cache; the shared
   artifact survives a per-session build kill.

**Gate (self-protect):** a test that (a) two different `MOLT_SESSION_ID`s resolve
the SAME shared runtime.wasm cache entry for identical runtime source+target
(cross-session reuse), and (b) a "peak-RSS ceiling" build test asserts a witness
WASM build's total tracked RSS stays under an 8GB budget. A regression that makes
the runtime rebuild per session, or blows the memory ceiling, fails the gate.

## R73.2 — Auto toolchain provisioning for extension ecosystems

**The naive root:** the witness stalled for hours because Molt did not know
scipy.ndimage's Cython extensions need Cython 3.1 (with `--generate-shared` /
standalone), a WASI sysroot, meson, a wasm-capable compiler, and numpy headers —
and did not auto-provision them. Extension staging (`scipy._cyutility`) failed
because the toolchain path was manual.

**Obligation:** Molt derives, provisions, and configures the full toolchain a
package's extension build requires, from the package's own build metadata
(`pyproject`/`meson.build`/`setup.py`/Cython directives), with no manual env:
- Every verified extension set declares one pinned project dependency group.
  Its producer environment is immutable and content-addressed by the group name
  plus ordered requirements, full `uv.lock`, base Python, and uv identity under
  canonical Molt custody. Provisioning is serialized, staged privately with
  frozen-lock `uv sync`, resolution-validated, attested, and atomically
  published. No ambient `.venv`, editable project install, or worktree path can
  become build-environment authority.
- `molt extension produce-set` automatically provisions a missing address and
  re-executes the same typed request under its attested Python with the invoking
  worktree source as the only `PYTHONPATH` authority, safe-path mode enabled,
  and ambient Python-home/user-site injection disabled. A stale published
  address fails closed; it is never mutated beneath a concurrent build.
- Detect the build backend + generators (Cython version + flags incl.
  shared-utility vs standalone; f2py; meson/ninja; setuptools).
- Provision the cross toolchain (WASI sysroot, wasm compiler/zig, target libs)
  into the operator-managed toolchain root; validate versions; fail closed with
  a precise, actionable diagnostic if a required tool cannot be provisioned.
- Regenerate the extension C from source when the shipped `.c` carries a
  dependency Molt can't satisfy (e.g. re-run Cython **standalone** so a
  `_ni_label` embeds its utilities and drops the `scipy._cyutility` shared-util
  import — the bounded bypass), or build the shared-utility module as its own
  admitted extension. One custody path, no host fallback, no fake module.

The source producer and its uv/Meson/Cython/Ninja/LLVM toolchain are developer,
maintainer, and source-rebuild dependencies. Shipped end-user binaries consume
verified artifacts and do not require those tools; they enter this path only on
an explicit local source rebuild or registry miss that policy permits to build.

**Gate (self-protect):** a test that `molt build` on a program importing scipy
(distance_transform_edt/gaussian_filter/label) auto-provisions Cython + the wasm
sysroot and produces the required extension artifacts with NO manual env vars,
or fails closed naming the exact missing tool + how to provision it.

## R73.3 — Precompiled Molt-binary registry / CDN for complex libraries

**Obligation:** as Molt proves a complex library's source-recompile works and
passes bit-parity vs the upstream reference, publish the tested `.molt.wasm`
extension artifacts (+ manifests, keyed on library version × target × abi ×
Molt version) to a content-addressed registry/CDN. `molt build` fetches the
tested artifact instead of recompiling locally when one exists; it recompiles
only on a cache/registry miss or `--rebuild`. The recompile path remains the
validator that gates publication (a library is only published after its recompile
is proven parity-green — this doc's own witness kernel is the first such proof:
numpy + scipy.ndimage → bit-identical parity).

**Gate (self-protect):** a published artifact must carry its parity-proof
provenance (the oracle row that validated it); a fetch must verify content hash +
abi/version compatibility and fall back to recompile (never silently run a
mismatched binary). No artifact is served that wasn't recompile-proven.

## Sequencing

R73.1 first (it unblocks every heavy build, including the witness, on an 8GB
budget). R73.2 second (it makes ecosystem extensions "just work" and is what the
current witness `scipy._cyutility` blocker needs). R73.3 third (it removes the
recompile cost for users once R73.1/R73.2 make the recompile itself reliable and
proven). All three are gated; a naive regression on any of them fails closed.

See `docs/design/foundation/71_wasm_webgpu_numeric_acceleration.md` (capability
lowering), `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`, and the pact witness
(`collab/pact/`) which is the first recompile-parity proof feeding R73.3.
