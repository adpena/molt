# Molt Runtime Spec

## 1. Core Principles
The Molt runtime is a minimal, high-performance library written in Rust. It provides the essential primitives for Python semantics while minimizing overhead.

## 2. Object Representation: NaN-Boxing
Molt uses 64-bit NaN-boxing for all objects. This allows small primitives to be stored inline without heap allocation.

### 2.1 The Bit Scheme (64-bit)
- **NaN Space**: `0x7FF0000000000000` to `0xFFFF000000000000`
- **Pointer (Heap)**: Bits 48-63 = `0x0001` (or similar tag). Payload is a 48-bit address.
- **Int (64-bit)**: If it fits in 48 bits, stored inline. Otherwise, a heap pointer to a `BigInt`.
- **Float**: Standard IEEE 754 double (non-NaN values).
- **Bool/None**: Specific bit patterns in the NaN space.

```rust
pub enum MoltObject {
    InlineInt(i64),   // 48-bit signed
    Float(f64),
    Boolean(bool),
    None,
    Pointer(*mut MoltHeader), // Strings, bytes, lists, dicts, etc.
}
```

## 3. Memory Management

### 3.1 RC + Incremental GC (Baseline)
- **Reference Counting (RC)**: The primary mechanism. Every heap object has a 32-bit RC in its header.
- **Biased RC**: Objects that are predominantly owned by one thread use biased RC to avoid atomic overhead.
- **Cycle Detection**: An incremental, non-blocking mark-and-sweep collector runs in the background. It only scans objects that have been "decremented but not freed" and are potentially part of a cycle.

### 3.2 Memory Management Roadmap
RC is predictable but adds per-write overhead. We plan to evaluate:
- **Generational tracing GC** for short-lived objects (lower average overhead, better cache locality).
- **Hybrid RC + tracing** (RC for deterministic release of FFI buffers; tracing for graph-heavy Python objects).
- **Region/arena allocation** for compiler-internal short-lived objects.

Determinism constraints: GC triggers must be driven by explicit byte/epoch budgets (not wall-clock), and we avoid user-visible finalizers.

See `docs/spec/0009_GC_DESIGN.md` for the concrete hybrid design and targets.

### 3.3 Header Layout (runtime-relevant)
```rust
struct MoltHeader {
    type_id: u32,
    ref_count: u32,
    poll_fn: u64,
    state: i64,
    size: usize,
}
```

## 4. Collections
- **Lists**: `MoltList` - Heap-managed `Vec<MoltObject>` storage with explicit length + capacity (growth supported).
- **Tuples**: `MoltTuple` - Immutable sequence stored as a `Vec<MoltObject>` and hashable for composite keys.
- **Bytes/Bytearray**: contiguous byte buffers stored inline after a length header. Bytearray methods return bytearray objects; bytes methods return bytes.
- **Strings**: UTF-8 buffers stored inline; `find/split/replace/startswith/endswith/count/join` use ASCII fast paths with codepoint indexing for non-ASCII.
- **Dicts**: `MoltDict` - Insertion-ordered key/value pairs plus a deterministic, open-addressing hash table for lookups.
    - Table uses stable hashing (no randomized seeds) to keep binaries deterministic.
    - `dict_keys`/`dict_values`/`dict_items` return view objects backed by the dict (not materialized lists).
    - Iteration uses an explicit iterator object that tracks the target collection and index.
    - Hot-path methods are exposed as intrinsics (e.g., `list.count/index`, `tuple.count/index`, `bytes/str.find`).
    - **Tier 0 (Structified)**: Objects of stable classes are lowered to a struct with no `__dict__`. Access is `*(base + offset)`.
    - **Tier 1 (Shape Dict)**: Uses a "Shape" pointer + a value array. If keys match the shape, access is indexed.
- **Ranges**: `MoltRange` - Lazy sequence storing `start/stop/step` inline. `len`, `iter`, and `index` are computed without materializing lists.
- **Slices**: `MoltSlice` - Inline `start/stop/step` object used by indexing and slicing ops.

## 5. Concurrency: No GIL
Molt does not have a Global Interpreter Lock.
- **Thread Safety**:
    - Primitives and immutable objects (Strings, Tuples) are safe to share.
    - Mutable objects (Lists, Dicts) use fine-grained locking or are restricted to a single thread by default (requiring explicit `MoltThread` boundaries).
- **Async**: Built on top of Rust's `Future` and `tokio`. Python `async/await` is lowered to Rust futures.

## 6. Exception Handling
- **Fast Path**: Most Molt functions return a `MoltResult<T>` which is a specialized `Result` type optimized for register passing.
- **Error Propagation**: The compiler inserts explicit checks: `if (res.is_err()) return res;`.
- **Zero-cost Unwinding**: Used only for `SystemExit` or deep recursions where propagation is too heavy.
