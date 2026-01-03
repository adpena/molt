# FAQ: Why Not Just Use Numba, PyPy, or Cython?
**Audience:** web developers, Django users, infrastructure engineers
**Purpose:** Explain why Molt targets typical web workloads that other Python acceleration tools do not meaningfully address.

---

## Short Answer
Yes — **Molt is specifically designed to be great at the kinds of workloads that Numba, PyPy, and Cython do not handle well**:

- ORM-heavy request handlers
- JSON / MsgPack serialization
- request routing and middleware
- authentication and authorization
- database I/O with high concurrency
- background jobs and workers
- tail-latency–sensitive APIs

These workloads dominate real-world Django and Python web services, and they are *not* numeric-kernel-shaped.

---

## What “Numba-shaped” really means
Numba excels when:
- code is mostly numeric loops
- data lives in NumPy arrays
- control flow is simple and static
- functions can be compiled in isolation
- memory access is predictable

Typical web workloads look nothing like this.

Web requests are:
- I/O bound (DB, network)
- allocation-heavy (dicts, objects, strings)
- control-flow heavy (auth, permissions, branching)
- concurrency-sensitive
- latency-sensitive at the tail (P99/P999)

Numba is not built to optimize *systems-style* workloads.

---

## Where Molt *is* a good fit (and why)

### 1. ORM and database access
**Problem today**
- Django ORM is synchronous and object-heavy
- async wrappers still pay Python object and scheduling costs
- DB pool contention causes cascading latency spikes

**Why Molt helps**
- async-first DB layer
- structured concurrency (tasks/channels)
- cancellation-aware queries
- typed decoding (no per-row Python object tax)
- fewer threads, more predictable latency

This is a *runtime-level* improvement, not a function-level one.

---

### 2. Serialization (JSON, MsgPack, Arrow)
**Problem today**
- JSON encode/decode dominates CPU in APIs
- Python serializers allocate aggressively
- Cython/Numba help only if you rewrite code in special subsets

**Why Molt helps**
- native codecs integrated into the runtime
- zero/low-copy data paths
- structured payloads instead of dict soup
- Arrow IPC for bulk paths

You speed up *every request*, not just one function.

---

### 3. Request routing, auth, middleware
These are:
- branchy
- string-heavy
- allocation-heavy
- concurrency-sensitive

Numba cannot optimize them meaningfully.
Cython can, but at high maintenance and build cost.

Molt compiles the *whole handler path* under a reduced, verifiable semantic model.

---

### 4. Concurrency and tail latency
**This is the biggest differentiator.**

- CPython concurrency relies on threads, the GIL, or complex async patterns
- Numba/Cython do not change the concurrency model
- PyPy may help throughput but not predictability

Molt provides:
- Go-style tasks and channels
- structured cancellation
- bounded queues and backpressure
- stable P99/P999 latency under load

This is where production systems live or die.

---

## Tool-by-tool comparison

### Numba
**Great for:** numeric kernels, simulations, ML preprocessing
**Weak for:** web services, ORMs, serialization, routing, auth

Numba accelerates *math*, not *systems code*.

---

### Cython
**Great for:** small, well-defined hotspots
**Tradeoffs:**
- complex build pipelines
- CPython ABI coupling
- harder debugging
- limited concurrency impact

Cython is a scalpel, not a runtime strategy.

---

### PyPy
**Great for:** some pure-Python workloads
**Limitations:**
- ecosystem friction (CPython-only wheels)
- still interpreter-based
- no new deployment story
- concurrency model unchanged

PyPy helps sometimes, but it doesn’t redefine what Python is good at.

---

## What Molt does differently
Molt is not “faster Python” in the traditional sense.

It is:
- a compiler + runtime
- with explicit semantic tiers
- designed for systems workloads
- optimized for services and pipelines
- built for simple deployment (single binary, WASM option)

Molt targets the *shape of modern backend systems*, not just inner loops.

---

## The takeaway
If your workload is:
- numeric and array-heavy → Numba is excellent
- one tiny hotspot → Cython might help
- pure Python and simple → PyPy might help

If your workload is:
- Django / FastAPI / web services
- ORM-heavy
- serialization-heavy
- concurrency-heavy
- tail-latency–sensitive

→ **Molt is the right tool to build.**

---

## Suggested positioning line
> “Numba accelerates math.
> Cython accelerates functions.
> Molt accelerates services.”
