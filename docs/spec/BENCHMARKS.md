# Molt Performance Benchmarks

Molt aims to bridge the gap between Python's developer productivity and C/Rust's runtime efficiency. The following benchmarks compare Molt's AOT-compiled binaries against CPython 3.12.

## 🚀 Execution Speed (Native macOS arm64)

| Benchmark | Category | CPython (s) | Molt (s) | Speedup |
| :--- | :--- | :--- | :--- | :--- |
| `fib(30)` | Recursive Calls | 0.2006 | 0.0056 | **36.1x** |
| `sum(10M)` | Tight Loop | 1.6196 | 0.0044 | **370.9x** |
| `struct(1M)` | Object Overhead | 0.3183 | 0.0044 | **72.7x** |

### Analysis
- **Tight Loops:** Molt achieves near-native speed by lowering Python loops to machine-code jumps and unboxed integer arithmetic.
- **Recursion:** Efficient function call ABI and minimal stack frame overhead result in significant gains.
- **Structification:** By using fixed-offset memory layouts instead of dynamic `__dict__` lookups, Molt eliminates the most expensive part of Python's object model.
- **Vectorization Targets:** Upcoming SIMD kernels focus on integer reductions and byte/str scans.

## 🎯 2026 Targets (Loop-Heavy)

| Benchmark | Target Speedup | Notes |
| :--- | :--- | :--- |
| `sum_ints(10M)` | 300x+ | SIMD reductions with guard+fallback. |
| `dot_ints(10M)` | 200x+ | Vectorized elementwise + reduction. |
| `bytes_find(100MB)` | 50x+ | SIMD scan + memchr fast path. |

## 📦 Binary Size & Startup

| Benchmark | Binary Size (KB) | Startup Time (ms) |
| :--- | :--- | :--- |
| `hello.py` | 1551.9 | < 1ms |

### Comparison with Other Languages

| Metric | CPython | Molt | Go | Rust |
| :--- | :--- | :--- | :--- | :--- |
| **Runtime Model** | Bytecode VM | AOT Native | AOT Runtime | AOT Static |
| **GC** | RC + Tracing | Biased RC | Tracing | Manual/RAII |
| **Startup** | Slow (30ms+) | Instant (<1ms) | Fast | Instant |
| **Binary Size** | N/A (requires interpreter) | ~1.5MB | ~2MB | ~300KB |
| **Concurrency** | GIL | No GIL (Tasks) | CSP (Goroutines) | Async/Await |

## 🛠 Methodology
- **Molt Target:** `aarch64-apple-darwin` (Release mode).
- **CPython:** Standard 3.12.x from Homebrew.
- **Environment:** MacBook Pro M2, 16GB RAM.
- **Tooling:** Automated via `tools/bench.py`.

---

*Last Updated: Friday, January 2, 2026 - 04:15 UTC*
