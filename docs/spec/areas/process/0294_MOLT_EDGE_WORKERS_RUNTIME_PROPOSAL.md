# Molt Edge And Workers Runtime Proposal
**Document ID:** 0294
**Status:** Proposed
**Audience:** Molt runtime/compiler engineers, WASM implementers, product/platform leads
**Purpose:** Formalize a concrete direction for making Molt a Workers-class runtime that can replace Pyodide-style deployments where bounded compatibility, cold-start discipline, and capability-first host integration matter more than full CPython-in-WASM behavior.

---

## 0. Executive summary
Molt should add an explicit Edge/Workers product tier with these defining properties:

- WASM-first deployment profile
- snapshot-oriented startup pipeline
- small capability-based virtual filesystem
- schema-first host interop
- Cloudflare Worker compatibility as a first production target

Molt should not copy Emscripten's full filesystem/runtime model.

The right move is:

- steal Cloudflare's deploy-time init plus snapshot discipline
- exploit Worker-native VFS/Web APIs where they exist
- preserve Molt's current "no ambient authority, no hidden host fallback, no arbitrary object proxying" rules
- implement only the filesystem semantics that unlock real compatibility and notebook/app workloads

In short:

> Molt Edge should be a purpose-built compiled runtime for constrained environments, not a browser-side Unix emulator.

---

## 1. Problem statement
Molt already treats WASM parity as a strategic target, and the repo now has:

- linked wasm outputs
- Node/WASI and browser host harnesses
- DB host adapters
- WebSocket-backed socket support in browser lanes
- explicit Cloudflare/Pyodide design guidance

What is still missing is a unified product and architecture decision for the next step:

- How much filesystem should Molt expose?
- How should browser APIs and Worker APIs enter the runtime?
- What should be standardized across browser, WASI, and Worker hosts?
- How should Molt compete with Pyodide-class deployments without inheriting Emscripten's entire compatibility stack?

Without a crisp answer, wasm parity work risks becoming an accretion of host-specific shims instead of a coherent runtime tier.

---

## 2. Research summary

### 2.1 Cloudflare Workers
Cloudflare's Python Workers model demonstrates three high-leverage ideas:

- deploy-time import execution
- linear-memory snapshotting for cold-start reduction
- strict host-controlled I/O surfaces

Cloudflare Workers also now exposes a Worker-native virtual filesystem for `node:fs` and the Web File System API, with standard mount points such as `/bundle`, `/tmp`, and `/dev`.

Takeaway for Molt:

- snapshotting is worth copying
- Worker-native filesystem semantics are worth targeting
- a second, larger compatibility filesystem layered over the Worker VFS is not automatically a win

### 2.2 Pyodide
Pyodide proves that:

- users will tolerate constraints if packaging and interop feel good
- browser Python needs a viable filesystem/package story
- Emscripten compatibility buys ecosystem breadth, but at the cost of larger runtimes, more JS glue, and interpreter-centric semantics

Takeaway for Molt:

- copy the UX lessons
- do not copy the interpreter-first compatibility model

### 2.3 Emscripten
Emscripten's filesystem stack is excellent for making POSIX-ish C/C++ code believe it owns a machine.

That same strength is also the main reason not to copy it wholesale for Molt:

- broad libc/POSIX assumptions
- larger JS glue/runtime surface
- more room for semantic drift between browser, WASI, and Worker hosts

Takeaway for Molt:

- copy only the smallest set of semantics that materially improves parity

### 2.4 marimo
marimo is relevant as a product lesson, not as proof that Pyodide has already been displaced.

The useful lesson is:

- rethink the notebook/app abstraction
- prioritize startup, portability, and UX
- decouple "good browser notebooks" from "full local Python semantics"

Takeaway for Molt:

- the opportunity is not "be Emscripten but faster"
- the opportunity is "be the best runtime for explicit, portable Python-derived compute in constrained hosts"

---

## 3. Proposal

### 3.1 Introduce a formal product tier: `Molt Edge`
`Molt Edge` is a deployment tier for:

- Cloudflare Workers and Worker-like isolate platforms
- browser-hosted wasm modules
- small server-side sandboxed wasm workloads

Its contract is:

- strict-by-default
- capability-gated host I/O
- snapshot-aware deployment
- deterministic unless capabilities explicitly permit nondeterminism

### 3.2 Add a minimal virtual filesystem
Molt should standardize a small VFS that exists across browser, Worker, and WASI hosts.

Required mounts:

- `/bundle` - read-only packaged assets, modules, manifests
- `/tmp` - ephemeral writable temp space
- `/dev/stdin`, `/dev/stdout`, `/dev/stderr` - pseudo devices

Optional mounts:

- `/state` - host-backed persistent storage when the host provides durable semantics
- additional capability-scoped mounts only when explicitly granted

This VFS should be capability-oriented and intentionally incomplete.

### 3.3 Keep web APIs first-class host capabilities
Web APIs should not be forced through fake files by default.

The following should remain explicit host services:

- `fetch`
- WebSocket / stream sockets
- DB adapters
- KV/object/cache services
- crypto, timers, randomness

Filesystem and web APIs should coexist, not collapse into one another.

### 3.4 Standardize snapshots as deployment artifacts
Molt should define a `molt.snapshot` artifact that captures:

- initialized module graph
- deterministic cached state
- capability manifest
- mount plan
- compatibility/version hashes

Snapshotting is the cold-start lever that most directly answers the Pyodide-in-Workers replacement goal.

### 3.5 Make Cloudflare Workers the first production edge target
Cloudflare Workers should be the first concrete target because it offers:

- a real multi-tenant edge runtime
- explicit startup and resource constraints
- host Web APIs
- a platform VFS that Molt can map onto cleanly

The architecture should remain portable, but Cloudflare is the right forcing function for v0.1.

---

## 4. Why not copy Emscripten wholesale
Copying Emscripten's whole model is the wrong optimization target for Molt.

### 4.1 It optimizes the wrong promise
Emscripten is designed to maximize compatibility for native code expecting POSIX-ish behavior.

Molt is explicitly not trying to promise:

- full CPython-in-WASM semantics
- ambient host access
- arbitrary legacy package compatibility

### 4.2 It adds cost where Molt needs discipline
A full Emscripten-style runtime increases:

- runtime size
- JS glue complexity
- semantic surface area

That cost competes directly with Molt's goals around:

- cold starts
- determinism
- auditable capability boundaries
- reproducibility

### 4.3 It weakens product clarity
If Molt grows toward "browser Unix with Python-shaped semantics," it becomes less clear why a user should pick it over Pyodide.

Molt wins when it is:

- smaller
- faster to start
- more explicit
- easier to reason about operationally

---

## 5. Target architecture

### 5.1 Core layers
1. Compiler lowers supported Python semantics into Molt runtime intrinsics and wasm-compatible host calls.
2. Runtime exposes a target-independent VFS contract and capability contract.
3. Host adapters map that contract onto browser APIs, Worker APIs, or WASI.
4. Snapshot artifacts freeze init-time state for fast startup.

### 5.2 Host profile matrix

| Host | Filesystem baseline | Host services baseline | Notes |
|---|---|---|---|
| Browser | `/bundle`, `/tmp`, optional `/state` via OPFS | `fetch`, WebSocket, storage adapters | No blocking I/O; storage remains capability-gated |
| Cloudflare Workers | `/bundle`, `/tmp`, `/dev` via Worker VFS | `fetch`, WebSocket-compatible streams where available, platform services | First production edge target |
| WASI | preopened dirs mapped into Molt mounts | sockets/DB/process only when host exposes them | Best server-side portability baseline |

### 5.3 Capability principles
- No ambient authority.
- Filesystem access is not implied by module load.
- Persistent storage must be explicit and host-specific.
- Unsupported features must raise, never silently emulate through hidden host fallbacks.

---

## 6. Non-goals
This proposal does not authorize:

- full Emscripten/POSIX emulation
- arbitrary JS object proxying
- unrestricted package `pip install` compatibility in edge/browser targets
- implicit dynamic imports
- hidden CPython fallback paths

---

## 7. Phased rollout

### Phase 0 - Canonicalization
- Land proposal, spec, and MEP documents.
- Define the Cloudflare Worker target profile and minimal VFS contract.

### Phase 1 - Minimal runtime support
- Add VFS mount abstraction and capability plumbing.
- Support `/bundle`, `/tmp`, and stdio pseudo-devices.
- Add parity tests for minimal filesystem semantics.

### Phase 2 - Snapshot artifact
- Implement `molt.snapshot` generation and validation.
- Add snapshot-required deployment lane for edge/Workers targets.
- Benchmark cold starts against current wasm lanes and Pyodide baselines.

### Phase 3 - Worker host integration
- Implement Cloudflare Worker host adapter.
- Map Worker VFS and host Web APIs into Molt capability surfaces.
- Add real Worker integration tests and docs.

### Phase 4 - Optional persistence and storage services
- Add `/state` only for hosts with explicit durable semantics.
- Expose KV/R2/D1-style services as schema-first host capabilities, not generic object proxies.

---

## 8. Acceptance criteria
This proposal is successful when all of the following are true:

- Molt has a documented Edge/Workers tier with explicit constraints.
- Molt defines a small, portable VFS instead of relying on ad hoc host bindings.
- Cloudflare Workers can host Molt with a first-class target profile.
- Snapshot artifacts materially improve cold starts.
- Web APIs remain schema-first capabilities, not arbitrary proxy bridges.
- The resulting runtime is meaningfully smaller and more operationally explicit than a Pyodide/Emscripten stack aimed at broad compatibility.

---

## 9. Recommended immediate workstreams
- Write the binding spec for Edge/Workers VFS and host capabilities.
- Establish a PEP-style decision record for the new product tier.
- Create a Linear epic that decomposes runtime, wasm host, tooling, and validation workstreams.
- Gate future browser/Worker storage work against the minimal VFS contract instead of adding more ad hoc host shims.

---

## 10. Sources consulted (checked 2026-03-11)
- Cloudflare Python Workers: [https://developers.cloudflare.com/workers/languages/python/how-python-workers-work/](https://developers.cloudflare.com/workers/languages/python/how-python-workers-work/)
- Cloudflare `node:fs` for Workers: [https://developers.cloudflare.com/workers/runtime-apis/nodejs/fs/](https://developers.cloudflare.com/workers/runtime-apis/nodejs/fs/)
- Cloudflare VFS / Web File System API changelog: [https://developers.cloudflare.com/changelog/post/2025-08-15-nodejs-fs/](https://developers.cloudflare.com/changelog/post/2025-08-15-nodejs-fs/)
- Pyodide filesystem docs: [https://pyodide.org/en/stable/usage/file-system.html](https://pyodide.org/en/stable/usage/file-system.html)
- Emscripten filesystem overview: [https://emscripten.org/docs/porting/files/file_systems_overview.html](https://emscripten.org/docs/porting/files/file_systems_overview.html)
