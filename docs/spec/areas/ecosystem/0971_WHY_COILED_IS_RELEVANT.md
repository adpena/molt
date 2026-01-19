# Why Coiled Is Relevant to Molt (Without Being a Dependency)
**Status:** Guidance / Inspiration
**Audience:** Molt contributors and users evaluating the ecosystem
**Goal:** Explain what Coiled is, what it is *not*, and what Molt can learn from it (especially for adoption and product design).

---

## 0. What Coiled is (high-level)
Coiled is a **Python-first experience for running distributed compute** (centered on Dask) with an emphasis on:
- spinning up compute “somewhere else”
- syncing environments
- making “scale out” feel like a one-liner

It succeeds largely because it reduces **operational friction**, not because it changes Python’s runtime semantics.

> Molt should view Coiled as a case study in “make the hard thing feel like an import.”

---

## 1. What Coiled is *not* (important for Molt scope)
Coiled is not:
- a Python compiler
- a CPython replacement
- a language runtime
- a web server framework
- a general mechanism for speeding up ordinary request handlers

Even though Coiled can “make Python workloads go faster” by adding more machines, it’s a different lever than Molt’s:
- **Coiled lever:** distribute execution
- **Molt lever:** change runtime + compilation + concurrency model

---

## 2. The real overlap: adoption mechanics
The most valuable thing to learn from Coiled is not “Dask.” It’s **product shape**.

### 2.1 The “one import → compute appears” pattern
Coiled is effective because:
- users can try it quickly
- the first success happens early
- the developer does not have to redesign their whole system

For Molt, the analogous wedge is:
- `pip install molt_accel`
- `@molt_offload(...)`
- your endpoint/job runs in a Molt worker and you see the win

This is exactly how you convert “curious” into “adopter.”

### 2.2 Environment replication as a superpower
Coiled’s success is tightly tied to:
- reproducibility
- environment sync
- “it works on my machine” → “it works on the cluster”

For Molt, environment replication becomes:
- reproducible builds for the Molt worker binary
- deterministic dependency resolution (uv-friendly)
- build artifacts keyed by lockfiles + platform targets
- optional caching (local + CI)

If Molt nails this, adoption friction drops dramatically.

### 2.3 A job-oriented mental model
Coiled helps users think in “jobs” and “clusters.”
Molt’s second killer demo (“background jobs without Celery”) needs the same clarity:
- job definitions
- retries
- timeouts
- cancellation
- progress reporting

Coiled demonstrates that UX matters as much as raw capability.

---

## 3. Concrete “steal this” checklist for Molt
### 3.1 “Try it in 5 minutes”
- scaffold generator: `molt init demo-django-offload`
- one-command local run: `./molt_worker & python manage.py runserver`
- one-command benchmark: `./bench/run_local.sh`

### 3.2 Artifact identity and caching
- lockfile hash → artifact id
- store build metadata (compiler version, MIR version, target triple)
- support “download prebuilt worker for my target” when possible

### 3.3 Opinionated defaults
- MsgPack default for IPC payloads
- strict timeouts by default
- bounded queues by default
- visible backpressure

### 3.4 Great failure modes
- “worker missing export” is a readable error
- “schema mismatch” suggests a fix
- “capability denied” explains why

---

## 4. What Molt can do that Coiled cannot
Where Molt becomes fundamentally different:
- **single-binary services** (no interpreter env)
- **Go-class concurrency** and stable tail latency
- **explicit semantic contracts** that enable optimization
- **WASM targets** for portable modules
- **typed decoding + columnar pipelines** without Python object tax

Coiled is operational acceleration.
Molt is *language + runtime acceleration*.

---

## 5. Bottom line
Coiled is relevant because it proves:
- developers will adopt a powerful system if the first win is easy
- environment replication is worth building early
- “offload compute” is a product category with demand

Molt should learn from Coiled’s **adoption design**, not copy its architecture.
