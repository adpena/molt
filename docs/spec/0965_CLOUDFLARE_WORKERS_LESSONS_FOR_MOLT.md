# Cloudflare Workers Lessons for Molt (Including Python Workers via Pyodide/WASM)
**Document ID:** 0965
**Status:** Canonical Guidance (Strategic + Technical)
**Audience:** Molt runtime/compiler engineers, AI coding agents, WASM implementers, ecosystem leads
**Purpose:** Extract practical lessons from Cloudflare Workers—especially their **Python-on-Workers** approach (Pyodide + WASM + memory snapshots)—and translate them into Molt design decisions, specs, and implementation checklists. Also defines whether Molt could be a Workers runtime/alternative and how to handle **JavaScript FFI**.

---

## 0. Executive summary
Cloudflare Workers is a proof point that:
- **Isolate-based compute** can be massively multi-tenant and low-latency
- **WASM enables snapshotting** of runtime state to reduce cold start costs
- A practical edge runtime requires hard limits, strict I/O contracts, and a careful security model

Cloudflare’s Python Workers approach specifically demonstrates:
- deploy-time **import execution + WASM linear-memory snapshot**
- packaging workflows integrated with modern tooling (notably `uv`)
- a platform that can “feel like Python” while running in a constrained environment

Molt can learn from these operational strategies while pursuing a different goal:
> Molt = compiled semantics + schema-first boundaries + strict tiers
> Cloudflare Python Workers = CPython semantics in WASM for compatibility

---

## 1. What Cloudflare does (distilled model)
Cloudflare’s Python Worker pipeline (conceptual):
1) upload code + dependencies
2) create a V8 isolate and inject Pyodide
3) scan imports, execute them at deploy time
4) snapshot WASM linear memory
5) deploy snapshot so runtime avoids expensive imports

This is described in Cloudflare’s own docs and blog posts.
(See: “How Python Workers work”, “Bringing Python to Workers…”, and “Python Workers redux/advancements.”)

---

## 2. Steal / Adapt / Avoid table (high leverage)

| Topic | What Cloudflare does | Molt classification | What Molt should do |
|------|-----------------------|--------------------|---------------------|
| Cold starts | Deploy-time imports + snapshot | **STEAL** | Strict init phase + snapshot artifact for WASM and native |
| Packaging UX | Strong tooling story, `uv` integration | **STEAL** | Make `uv.lock` the canonical input; artifact ID = lock hash |
| Isolation | V8 isolates, multi-tenant | **ADAPT** | Support isolate-like sandboxing for “hosted Molt” / multi-tenant |
| Limits | CPU/memory constraints; graceful handling | **STEAL** | Make limits first-class in runtime APIs + metrics |
| Interop | JS runtime environment | **ADAPT** | Schema-first boundary, avoid object proxies |
| Compatibility goal | CPython in WASM | **AVOID** | Do not promise CPython compatibility in WASM |
| Security | Strong side-channel posture | **STEAL** | Treat runtime as hostile multi-tenant by default |

---

## 3. The #1 lesson to steal: snapshotting = “Strict Tier, operationalized”
Cloudflare’s deploy-time snapshot strategy is a direct operational instantiation of a concept Molt already wants:
- allow “mutation during init”
- freeze afterwards
- reuse the frozen state for fast startup

### 3.1 Molt artifact proposal: `molt.snapshot`
A snapshot is a build artifact that packages:
- compiled module(s) (WASM or native)
- schema registry (Schema IR IDs + versions)
- pre-initialized runtime state (init-only)
- optional precompiled codecs
- optional warmed caches (carefully scoped; deterministic only)

### 3.2 Rules
- snapshot must be deterministic and reproducible
- snapshot must not include secret material by default
- snapshot must include a compatibility stamp (ABI + schema versions)
- snapshot must allow “reset-to-clean” semantics

### 3.3 AI agent instruction
When implementing snapshots, always separate:
- **init phase** (allowed side effects)
- **run phase** (restricted, cancelable, bounded)

No “hidden init” at request time.

---

## 4. Limits are a feature (not a footnote)
Workers platforms succeed because they are explicit about constraints.

### 4.1 Molt runtime must make limits visible
- CPU budget (active compute) should be queryable in `ctx.limits`
- memory budget should be exposed
- runtime should surface cancellation reason (CPU, OOM, user cancel, disconnect)

### 4.2 Design constraint
If Molt does not make limits first-class:
- developers will write code that “works locally” and fails in production
- tail latency will degrade
- hosted/multi-tenant Molt becomes impossible

---

## 5. Could Molt be used for Workers or an alternate Workers implementation?
**Yes, plausibly—if we treat it as a long-term platform tier.**

### 5.1 Why it’s plausible
Molt’s core properties are aligned with what an edge runtime needs:
- deterministic execution tiers
- explicit boundaries (schemas)
- strong cancellation/backpressure
- ability to compile to WASM
- small artifacts and fast startup (via snapshots)

An edge runtime wants predictable, bounded compute. Molt is being designed to enforce that.

### 5.2 What would need to be true
To be a credible Workers runtime (or alternative), Molt must:
- run in a sandbox (WASM or isolate environment)
- have a stable ABI (see 0964)
- have strict limits and clear failure modes
- have a robust packaging story
- have excellent observability

### 5.3 How to include this in Molt’s roadmap/specs
Add an explicit “Edge/Workers tier” as a product target:
- **Tier: Molt Edge**
  - WASM-first
  - strict tier only (or strict-by-default)
  - schema-only boundary IO
  - snapshot-required deployment
  - banned features: dynamic imports, reflection-heavy code, monkeypatching

This is not a promise that Molt will replace Cloudflare Workers.
It is a statement that Molt could become a best-in-class runtime for Workers-like platforms.

### 5.4 Investor narrative hook
This positions Molt as:
- not just “faster Python”
- but a portable execution layer that can run:
  - servers
  - workers
  - browsers
  - potentially edge isolates

---

## 6. Should Molt include a JavaScript FFI?
**Yes—but with strict constraints.**

### 6.1 Why JS FFI matters
If Molt targets WASM and wants browser/server symmetry:
- browser APIs are JS
- Workers APIs are JS-ish (even when hosting other languages)
- storage/kv/crypto interfaces often originate from JS runtimes

A JS FFI is necessary for practicality.

### 6.2 The mistake to avoid
Do **not** implement “arbitrary object proxy interop” like:
- passing Python objects into JS freely
- passing JS objects into Molt freely

This becomes:
- slow
- nondeterministic
- hard to secure
- hard to version

### 6.3 The Molt JS FFI rule (hard)
> **JS FFI must be schema-first and capability-based.**

Meaning:
- all calls cross a boundary with a schema-defined payload
- return values are schema-defined
- JS APIs are exposed as explicit capabilities injected at init
- ownership and lifetime are explicit
- async boundaries are explicit (`await`/promises mapped to Molt tasks)

### 6.4 Recommended FFI API surface (v0.1)
Expose **only**:
- `ffi.call(capability_id, schema_id, payload_bytes) -> (schema_id, payload_bytes)`
- `ffi.stream_*` for streaming (optional later)
- `ffi.cancel(handle)` where needed

No dynamic evaluation (`eval`), no runtime reflection into host objects.

---

## 7. Concrete TODO list for Molt (actionable)
### 7.1 Specs to update / add
- (Existing) 0964 WASM ABI doc: add snapshot artifact references
- (Add) `Snapshot Artifact Spec v0.1` (format, determinism rules, security)
- (Add) `Molt Edge Tier` spec (constraints, capabilities, APIs)

### 7.2 Engineering steps (sequenced)
1) Implement Schema IR registry + IDs
2) Implement WASM ABI v0.1 call/poll/cancel
3) Add snapshot build step that freezes init state
4) Create browser demo that uses schema-only JS FFI capabilities
5) Add benchmark outputs: startup time, size, memory (per 0960)

---

## 8. AI AGENT RULES (MANDATORY)
When building anything related to Workers/WASM/FFI:
1) If it crosses a boundary, it must use schemas
2) No object proxies across JS/WASM boundary
3) No dynamic import behavior in edge tier
4) Snapshot is required for deploy-grade cold starts
5) Limits must be queryable and enforced

If an implementation violates any of the above, reject it.

---

## 9. Bottom line
Cloudflare’s Python Workers prove that:
- WASM snapshotting works at scale
- packaging UX matters as much as execution speed
- constraints are the product

Molt can adopt these lessons and become:
- a better “compiled contract runtime” for services and workers today
- and potentially a Workers-class runtime tier in the long run

> **Molt should not try to be CPython-in-WASM.
> Molt should be the best runtime for explicit contracts in constrained environments.**
