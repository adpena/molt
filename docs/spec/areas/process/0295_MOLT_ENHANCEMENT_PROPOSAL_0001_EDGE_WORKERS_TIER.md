# MEP-0001: Molt Edge/Workers Tier
**Document ID:** 0295
**Status:** Draft
**Type:** Standards Track / Runtime
**Created:** 2026-03-11
**Requires:** 0400, 0401, 0964, 0965, 0968
**Audience:** Molt maintainers, runtime engineers, wasm/host integrators
**Purpose:** Provide a PEP-style decision record for adopting a first-class Edge/Workers tier with a minimal virtual filesystem, snapshot-oriented deployment, and capability-first web/Worker integration.

---

## Abstract
This MEP proposes a new Molt product/runtime tier named `Molt Edge`.

`Molt Edge` targets:

- browser-hosted wasm
- Cloudflare Workers and Worker-like runtimes
- constrained server-side wasm hosts

The proposal standardizes:

- a minimal mountable virtual filesystem
- snapshot-oriented deployment artifacts
- schema-first host capability boundaries
- Cloudflare Workers as the first concrete production edge target

This MEP explicitly rejects adopting Emscripten's full filesystem/runtime model as Molt's default wasm architecture.

---

## Motivation
Molt wants wasm parity and better-than-Pyodide outcomes for representative workloads, but it also has hard project constraints:

- no hidden host fallback
- determinism by default
- capability gating
- small artifacts and clear operational behavior

At the same time, Pyodide-class deployments continue to prove market demand for:

- Python-derived logic in browsers and edge hosts
- notebook/app workloads that need some filesystem affordances
- strong UX around packaging, startup, and interop

The missing piece is a clear, stable contract for how Molt should behave in those environments.

---

## Rationale

### 1. Snapshotting is the biggest leverage point
Cloudflare's Python Workers model shows that deploy-time import execution and linear-memory snapshots are one of the best cold-start optimizations available for constrained runtimes.

Molt should copy this operational pattern.

### 2. A small VFS unlocks real parity without bloating the runtime
Many workloads want:

- package/resource reads
- temp files
- stdio-like sinks

They do not require a full browser-side Unix environment.

### 3. Web APIs should remain explicit capabilities
`fetch`, WebSocket, KV, D1, R2, Cache, timers, and crypto are not best modeled as fake files by default.

Molt remains clearer and safer when those services are explicit host capabilities.

### 4. Full Emscripten compatibility is not Molt's product promise
Molt's opportunity is not "slightly different Pyodide."
Molt's opportunity is "compiled, portable, explicit Python-derived execution for constrained hosts."

---

## Specification

### 1. New target tier
Molt SHALL define a new product/runtime tier named `Molt Edge`.

Properties:

- wasm-first deployment
- strict-by-default capability model
- snapshot-aware startup path
- target profiles for browser, WASI, and Worker hosts

### 2. Minimal VFS
Molt SHALL expose a minimal VFS with the following required mount points:

- `/bundle`
- `/tmp`
- `/dev/stdin`
- `/dev/stdout`
- `/dev/stderr`

Optional mounts MAY be exposed only via explicit capability grants, including:

- `/state` for persistent host-backed storage

### 3. Host capability policy
Molt SHALL keep non-filesystem host services as explicit capability surfaces, including:

- HTTP/fetch
- stream sockets / WebSocket-backed sockets
- database host calls
- time / randomness
- storage services that do not naturally map to path semantics

### 4. Snapshot artifact
Molt SHALL define a `molt.snapshot` artifact for edge-class deployments.

The artifact SHALL capture:

- initialized runtime state
- module/package manifest
- schema registry compatibility stamp
- mount plan
- capability manifest
- deterministic build metadata

### 5. Cloudflare-first host profile
Molt SHALL treat Cloudflare Workers as the first concrete `Molt Edge` production target.

This target SHALL map Molt's minimal VFS and host capability contracts onto Worker-native facilities instead of introducing a second, full compatibility layer by default.

---

## Backwards compatibility
This MEP is additive.

It does not remove current wasm targets or current host harnesses.

It does require future wasm host work to align to the standardized Edge/Workers contract rather than continuing to add target-specific storage and host-API shims without a common abstraction.

---

## Security implications
- No ambient filesystem or network authority is introduced.
- Persistent storage remains explicit and host-defined.
- Snapshot artifacts must exclude secrets unless explicitly requested and documented.
- Host boundaries remain schema-first and capability-based.
- Rejecting arbitrary JS object proxies reduces confused-deputy and lifetime/ownership bugs.

---

## Reference implementation plan
1. Land the Edge/Workers VFS and host-capability spec.
2. Add runtime mount abstraction and capability checks for `/bundle`, `/tmp`, and stdio.
3. Implement snapshot artifact generation.
4. Add Cloudflare Worker host adapter and integration tests.
5. Add cold-start, size, and parity gates against representative Pyodide/wasm baselines.

---

## Rejected alternatives

### 1. Copy full Emscripten runtime/filesystem behavior
Rejected because it expands runtime size, JS glue, and semantic scope toward a compatibility goal Molt does not actually want to promise.

### 2. Do not add a VFS at all
Rejected because too many realistic workloads need at least packaged resource reads, temp files, and stdio sinks.

### 3. Model all web/Worker APIs as files
Rejected because many services are better expressed as schema-first host capabilities than as path-oriented abstractions.

### 4. Treat Workers as just another WASI host
Rejected because Worker platforms have distinct resource limits, deployment models, and Web/API surfaces that deserve an explicit target profile.

---

## Acceptance criteria
This MEP is accepted when:

- the associated Edge/Workers spec is canonicalized
- the runtime exposes the minimal VFS
- snapshot artifacts exist and are benchmarked
- Cloudflare Worker target support is implemented
- wasm/browser/Worker documentation, parity tests, and host adapters align to the new contract

---

## References
- 0294: `Molt Edge And Workers Runtime Proposal`
- 0965: `Cloudflare Workers Lessons For Molt`
- 0968: `Molt Edge/Workers VFS And Host Capabilities`
