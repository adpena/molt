# Molt Edge/Workers VFS And Host Capabilities
**Document ID:** 0968
**Status:** Draft (implementation-targeting)
**Audience:** runtime engineers, wasm host implementers, platform/tooling engineers
**Purpose:** Define the binding v0.1 contract for Molt's Edge/Workers tier: target profiles, virtual filesystem semantics, snapshot artifacts, and capability-first host integration for browser, Worker, and WASI hosts.

This document extends and sharpens:

- 0400 WASM Portable ABI
- 0401 WASM Targets And Constraints
- 0964 Molt WASM ABI Browser Demo And Constraints
- 0965 Cloudflare Workers Lessons For Molt

---

## 0. Design principles
- Schema-first host boundaries remain mandatory.
- Determinism is the default; nondeterminism requires explicit capabilities.
- The VFS exists to unlock parity, not to emulate a whole operating system.
- Browser APIs and Worker APIs remain first-class host capabilities.
- Cloudflare Workers is the first production edge target, but the contract must remain portable.

---

## 1. Host profiles

### 1.1 `wasm_browser`
Host: browser main thread, worker, or equivalent JS runtime.

Required host properties:

- explicit import surface
- no blocking I/O assumptions
- capability-mediated access to Web APIs

### 1.2 `wasm_wasi`
Host: WASI runtime or wasm sandbox with WASI-like preopens and host calls.

Required host properties:

- stable WASI import availability or explicit compatible substitutes
- capability-aware directory/file exposure

### 1.3 `wasm_worker_cloudflare`
Host: Cloudflare Worker runtime.

Required host properties:

- Worker-native Web APIs
- Worker virtual filesystem / Web File System support when enabled by platform/runtime configuration
- explicit platform resource limits

This is the canonical v0.1 edge profile.

---

## 2. VFS overview
Molt SHALL expose a mount-oriented VFS with a fixed semantic contract across hosts.

The VFS is defined in terms of mount roles, not host implementation details.

### 2.1 Required mounts

#### `/bundle`
Purpose:

- read-only packaged modules
- read-only resources
- manifests, embedded assets, static metadata

Rules:

- MUST be read-only
- MUST be available at process/module startup
- MUST be deterministic for a given artifact hash

#### `/tmp`
Purpose:

- temporary files
- scratch data
- transient compiler/runtime spill if explicitly allowed by the host profile

Rules:

- MUST be writable if `fs.tmp.write` is granted
- MUST be treated as ephemeral
- MUST NOT be assumed durable across requests, isolates, or restarts

#### `/dev/stdin`, `/dev/stdout`, `/dev/stderr`
Purpose:

- pseudo-device compatibility
- logging and stream sink/source normalization

Rules:

- `stdout` and `stderr` MUST map to host logging or stream sinks
- `stdin` MAY be absent or empty on hosts that do not provide request-body streaming through file semantics

### 2.2 Optional mounts

#### `/state`
Purpose:

- durable host-backed persistent storage

Rules:

- MUST be absent unless the host explicitly provides persistent semantics
- MUST be capability-gated
- MUST document durability, isolation, and quota behavior per host profile

No other mount names are standardized in v0.1.

---

## 3. Filesystem capabilities

### 3.1 Capability names
- `fs.bundle.read`
- `fs.tmp.read`
- `fs.tmp.write`
- `fs.state.read`
- `fs.state.write`

### 3.2 Capability rules
- Access MUST fail explicitly when the required capability is absent.
- Missing capability failures MUST not silently fall back to in-memory emulation unless the host profile documents that mount as intrinsically in-memory.
- Capability checks MUST be visible in diagnostics and parity tests.

### 3.3 v0.1 minimum grants by host

| Host profile | Required default grants | Optional grants |
|---|---|---|
| `wasm_browser` | `fs.bundle.read` | `fs.tmp.*`, `fs.state.*` |
| `wasm_wasi` | host-defined | host-defined |
| `wasm_worker_cloudflare` | `fs.bundle.read` | `fs.tmp.*` |

Cloudflare v0.1 assumes no durable `/state` mount by default.

---

## 4. Filesystem operations surface
The runtime MUST support the subset of filesystem behavior needed for:

- packaged resource reads
- temp file creation/read/write/delete
- directory listing for standardized mounts
- stat-style metadata sufficient for `pathlib`, `importlib.resources`, and related shims

The runtime does NOT promise:

- full POSIX behavior
- advisory locking
- device/ioctl semantics beyond stdio
- unrestricted file descriptor duplication semantics

Unsupported operations MUST raise explicit capability or availability errors.

---

## 5. Host mapping rules

### 5.1 Browser mapping
Recommended mapping:

- `/bundle` -> packaged assets supplied by host loader
- `/tmp` -> in-memory temp filesystem
- `/state` -> optional OPFS-backed mount when the host explicitly enables it
- stdio -> console/log stream adapters

Browser-only note:

- web services such as `fetch`, WebSocket, Cache Storage, IndexedDB, and OPFS remain separate host capabilities even when some of them can back filesystem mounts

### 5.2 Cloudflare Worker mapping
Required mapping for v0.1:

- `/bundle` -> Worker bundle mount
- `/tmp` -> Worker temp mount when platform/runtime enables write support
- `/dev/*` -> Worker virtual device semantics when exposed by the platform

Rules:

- Worker-native `fetch` remains the canonical HTTP capability
- Worker platform storage products MUST NOT be exposed as generic path trees by default
- D1/R2/KV/Cache integrations SHOULD be expressed as explicit schema-first host services

### 5.3 WASI mapping
Recommended mapping:

- `/bundle` -> preopened read-only directory
- `/tmp` -> preopened temp directory or runtime-managed temp area
- `/state` -> optional preopened durable directory

Rules:

- preopens MUST be capability-scoped
- path traversal outside granted roots MUST be rejected

---

## 6. Web and Worker host capabilities
The following host services are standardized as explicit capabilities, not file emulations.

### 6.1 Network / HTTP
- `http.fetch`
- `net.connect`
- `net.listen` only on hosts that genuinely support server sockets

### 6.2 Streaming sockets
- `socket.stream`

Browser and Worker hosts MAY map stream sockets onto WebSocket-backed transport when raw sockets are unavailable.

### 6.3 Database
- `db.read`
- `db.write`

These use the schema/MsgPack/Arrow contracts already defined by Molt's DB IPC docs.

### 6.4 Platform storage services
Reserved capability families:

- `kv.read`, `kv.write`
- `object.read`, `object.write`
- `cache.read`, `cache.write`

These MUST remain explicit host services in v0.1.

### 6.5 Nondeterministic services
- `time.now`
- `rand.bytes`
- `crypto.sign`
- `crypto.verify`

---

## 7. Snapshot artifact: `molt.snapshot`

### 7.1 Purpose
`molt.snapshot` captures post-init runtime state for faster startup in edge and Worker deployments.

### 7.2 Required fields
- `snapshot_version`
- `abi_version`
- `target_profile`
- `module_hash`
- `schema_registry_hash`
- `mount_plan`
- `capability_manifest`
- `determinism_stamp`
- `init_state_blob`

### 7.3 Snapshot rules
- Snapshot generation MUST occur after deterministic init only.
- Secrets MUST NOT be captured by default.
- Snapshot validity MUST be tied to wasm/module/runtime compatibility hashes.
- Hosts MAY reject snapshots that exceed policy limits.

### 7.4 Cloudflare deployment model
For `wasm_worker_cloudflare`, the intended lifecycle is:

1. package code and resources
2. perform init/import work
3. freeze deterministic init state
4. deploy wasm + snapshot + manifest
5. restore snapshot during isolate startup

---

## 8. Import and packaging rules
- Imports from packaged modules/resources SHOULD resolve via `/bundle`.
- Resource-oriented APIs SHOULD prefer `/bundle` and `importlib.resources`-style readers over ad hoc host fetches.
- Dynamic package installation is out of scope for v0.1 edge/Worker targets.
- Host package/resource injection MUST be explicit and hash-addressed.

---

## 9. Conformance requirements

### 9.1 Filesystem parity tests
Every host profile MUST run a parity suite covering:

- package/resource reads from `/bundle`
- temp file create/read/write/delete in `/tmp`
- directory listing and basic metadata
- explicit capability denials

### 9.2 Snapshot tests
Edge/Workers targets MUST verify:

- snapshot determinism
- snapshot restore correctness
- cold-start delta reporting

### 9.3 Platform integration tests
`wasm_worker_cloudflare` MUST add real or emulated host integration coverage for:

- `fetch`
- `db.read` / `db.write` where configured
- `/bundle`
- `/tmp`
- cancellation and structured error surfacing

---

## 10. Performance gates
The Edge/Workers runtime MUST track:

- wasm artifact size
- snapshot size
- cold start without snapshot
- cold start with snapshot
- p50/p95 request latency for representative workloads

Relative comparisons against Pyodide baselines SHOULD be maintained for representative categories, but Molt's acceptance gate is primarily based on Molt's own reproducible benchmark suites.

---

## 11. Security and determinism
- No ambient authority.
- Every mount is explicit.
- Every host capability is explicit.
- Deterministic mode MUST be the default for snapshot generation.
- Worker/browser integrations MUST not introduce hidden JS object proxy semantics.

---

## 12. Non-goals
This spec does not require:

- full POSIX parity
- full Emscripten filesystem parity
- arbitrary `pip install` support in browser/Worker targets
- generic JS object interop
- unrestricted persistent filesystem support on all hosts

---

## 13. Source notes (checked 2026-03-11)
External references that informed this v0.1 contract:

- Cloudflare Python Workers: [https://developers.cloudflare.com/workers/languages/python/how-python-workers-work/](https://developers.cloudflare.com/workers/languages/python/how-python-workers-work/)
- Cloudflare `node:fs`: [https://developers.cloudflare.com/workers/runtime-apis/nodejs/fs/](https://developers.cloudflare.com/workers/runtime-apis/nodejs/fs/)
- Cloudflare Web File System API announcement: [https://developers.cloudflare.com/changelog/post/2025-08-15-nodejs-fs/](https://developers.cloudflare.com/changelog/post/2025-08-15-nodejs-fs/)
- Pyodide filesystem docs: [https://pyodide.org/en/stable/usage/file-system.html](https://pyodide.org/en/stable/usage/file-system.html)
- Emscripten filesystem overview: [https://emscripten.org/docs/porting/files/file_systems_overview.html](https://emscripten.org/docs/porting/files/file_systems_overview.html)
