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
    Pointer(*mut MoltHeader),
}
```

## 3. Memory Management

### 3.1 RC + Incremental GC
- **Reference Counting (RC)**: The primary mechanism. Every heap object has a 32-bit RC in its header.
- **Biased RC**: Objects that are predominantly owned by one thread use biased RC to avoid atomic overhead.
- **Cycle Detection**: An incremental, non-blocking mark-and-sweep collector runs in the background. It only scans objects that have been "decremented but not freed" and are potentially part of a cycle.

### 3.2 Header Layout (16 bytes)
```rust
struct MoltHeader {
    type_id: u32,
    rc: u32,
    gc_flags: u32,
    vtable_ptr: u32, // Offset into a global vtable array
}
```

## 4. Collections
- **Lists**: `MoltList` - A contiguous array of `MoltObject` (64-bit values).
- **Dicts**:
    - **Tier 0 (Structified)**: Objects of stable classes are lowered to a struct with no `__dict__`. Access is `*(base + offset)`.
    - **Tier 1 (Shape Dict)**: Uses a "Shape" pointer + a value array. If keys match the shape, access is indexed.

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
