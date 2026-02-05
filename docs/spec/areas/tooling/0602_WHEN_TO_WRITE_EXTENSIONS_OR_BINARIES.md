# Should We Write Rust/C Extensions or Go Binaries to Accelerate Existing Libraries (e.g., Django)?
**Spec ID:** 0602
**Status:** Guidance (strategy + nuance)
**Audience:** product owners, performance engineers
**Goal:** Decide when it is worth building native components versus compiling with Molt or integrating existing engines.

---

## 0. The short answer
Yes, it can be worth it — but **not in the way most people mean**.

For Molt’s strategy:
- Prefer **Molt-native compilation + Molt Packages** (Rust/WASM) for core performance.
- Use **native components** selectively for:
  - codecs (JSON/MsgPack)
  - crypto
  - DB drivers
  - HTTP parsing
  - columnar kernels (Arrow/Polars-like)
- Avoid sinking years into rewriting large pure-Python frameworks (Django) in Rust unless you have a tight scope and a measurable win.

---

## 1. Django reality check (where time actually goes)
Django’s cost in production is usually dominated by:
- serialization/deserialization (JSON)
- ORM row hydration and conversions
- template rendering
- middleware layers
- DB I/O (often the true bottleneck)
- application business logic

Django itself is mostly Python glue. Rewriting “Django in Rust” rarely pays off compared to:
- faster DB access patterns
- faster serialization
- reducing per-request allocations
- compiling request-handling and business logic via Molt workers

---

## 2. What is worth “native acceleration” (high ROI)
### 2.1 Codecs and parsing (huge ROI)
- JSON parser/serializer
- URL parsing
- headers parsing
- MsgPack/CBOR

### 2.2 Database drivers (huge ROI)
- Molt-native async Postgres/MySQL clients
- prepared statement caching
- efficient row decoding into columnar/struct layouts

### 2.3 Columnar and dataframe kernels (huge ROI)
- filter/project/groupby/join kernels (vectorized)
- Arrow IPC and Parquet

### 2.4 Crypto and auth utilities (moderate ROI)
- password hashing
- JWT signing/verification

---

## 3. What is usually NOT worth rewriting
### 3.1 Whole frameworks (Django) as a rewrite
- enormous surface area
- behavior compatibility is hard
- users want compatibility, not a new framework
- performance wins often come from smaller subsystems

Instead:
- keep Django routing/auth/middleware
- offload heavy endpoints/jobs to Molt worker
- gradually move services to Molt-native stack where appropriate

---

## 4. Rust “extensions” vs Go binaries: which and when?
### 4.1 Rust components (recommended default)
Rust is ideal for:
- embedding into Molt binaries
- WASM packages
- low-level kernels and runtimes
- predictable memory and tail latency

### 4.2 C extensions (avoid as a product strategy)
C extensions:
- couple you to CPython ABI
- reintroduce the exact dependency pain Molt tries to eliminate

If you must build native extensions, prefer recompile against `libmolt` to avoid CPython ABI coupling.
If you must build CPython extensions (bridge/adoption), treat them as **temporary** and isolate them behind a stable IPC/WASM boundary.

### 4.3 Go binaries (great for process boundary, not for in-process kernels)
Go is excellent for:
- sidecar services
- standalone workers
- networking-heavy daemons
- quick cross-platform shipping

But for in-process kernels:
- Go’s GC and runtime footprint can hurt tail latency and binary size goals.
- Rust is a better match for Molt’s priorities.

---

## 5. Practical strategy aligned with Molt
### Phase 1: Integration-first (months)
- Use Polars and DuckDB as engines via Rust integration or IPC.
- Build `molt_worker` and `molt_accel` for Django and service adoption.
- Ship fast codecs and DB connectors as Molt Packages.

### Phase 2: Own hotspots (months–year)
- Replace the top 5–10 hottest kernels with Molt-native implementations.
- Maintain pandas oracle testing for semantics.

### Phase 3: Modern “core pandas” compatibility (year+)
- Expand API coverage with formal compatibility matrix.
- Keep object dtype and full alignment semantics as opt-in/slow tiers.

---

## 6. Decision rule (use this in planning)
Write a native component if:
- it is a top hotspot in real workloads, AND
- it has a stable spec/contract, AND
- it can be tested differential/property-based, AND
- it reduces deployment complexity (not increases it)

Avoid native rewrites if:
- surface area is huge
- performance gain is unclear
- it introduces ABI/toolchain pain

---

## 7. The takeaway
To make Molt transformative for web and pandas-heavy stacks:
- **do not rewrite Django**
- **do rewrite the hot subsystems** (codecs, DB clients, kernels)
- **do provide a migration wedge** (offload endpoints/jobs via Molt worker)
