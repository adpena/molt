# Rust Transpilation Backend Design
**Spec ID:** RUST_BACKEND
**Status:** Draft (design-targeting)
**Owner:** compiler + runtime
**Date:** 2026-03-12

---

## 0. Context and Existing State

Molt already has a working Rust transpilation backend at `runtime/molt-backend/src/rust.rs` (3042 lines). This backend transpiles `SimpleIR` into standalone Rust source files using a dynamically-typed `MoltValue` enum that mirrors Python's runtime type system. Every variable is `MoltValue`, every use clones, and all operations dispatch through match arms at runtime.

This document defines the path from the current **correct-first dynamic** backend to a **performance-optimized, type-specialized** Rust backend that produces idiomatic Rust leveraging Molt's type inference, escape analysis, and specialization infrastructure.

### Current backend capabilities (v0, `rust.rs`)
- `MoltValue` enum: `None | Bool(bool) | Int(i64) | Float(f64) | Str(String) | List(Vec<MoltValue>) | Dict(Vec<(MoltValue, MoltValue)>) | Func(Arc<dyn Fn>)`
- Conditional prelude emission (only helpers referenced by the function body)
- Phi hoisting, alias tracking, closure slot pre-declaration
- Op coverage: constants, arithmetic, comparisons, control flow (if/else/loop/for), calls, list/dict/tuple ops, string methods, closures, classes (dict-based), target-version `sys` bootstrap, and module-cache get/set/delete for emitted import bootstrap IR
- Wired into the CLI `--target rust` source-emission path and `molt-backend` behind the `rust-backend` feature
- Exceptions remain fail-fast/structural where full Python exception propagation is outside the current source-backend surface
- No type specialization -- all variables are `MoltValue`, all uses clone

### Landscape of prior art
- **Depyler** (paiml/depyler): Python-to-Rust transpiler with semantic verification. Pipeline: Python AST -> HIR -> Type Inference -> Rust AST -> CodeGen. Claims 75-85% energy reduction. Supports type-directed transpilation and memory safety analysis but targets a limited Python subset.
- **pyrs** (konchunas/pyrs): Syntax converter, not a compiler. Produces unidiomatic Rust requiring manual edits.
- **py2many**: Multi-target transpiler (Rust, C++, Julia, etc.) from Python. Broad but shallow.
- **LPython**: AOT Python compiler targeting LLVM/C++/WASM. Type-annotated subset. Novel ASR intermediate representation shared with LFortran.
- **RustPython**: Python interpreter in Rust. Has experimental JIT but no AOT transpilation.
- **PyO3/maturin**: Rust-Python interop for writing native extensions. Not a transpiler but the standard for Rust<->Python FFI.
- **Cython**: Python-to-C transpiler. No Rust target. Relevant as architectural precedent for typed-subset AOT compilation.

Molt's position is unique: we already have a working IR pipeline (Python AST -> HIR -> TIR -> SimpleIR), a working Cranelift native backend, a working WASM backend, a working Luau backend, and a working (but unspecialized) Rust backend. The goal is not to build from scratch but to evolve the existing Rust backend through progressive type specialization.

---

## 1. Design Philosophy

### 1.1 What "Python-to-Rust" means for Molt

The Rust backend serves three distinct use cases, in priority order:

1. **Auditable artifact**: Generate human-readable Rust that developers can inspect, review, and understand. When a Molt-compiled binary misbehaves, the Rust source is the debugging surface. Readability is a hard requirement.

2. **Portable compilation target**: Generated Rust compiles with `rustc` on any tier-1 Rust platform without requiring the Molt runtime. The output is a self-contained crate. This enables deployment to environments where shipping a custom runtime is impractical.

3. **Gradual migration bridge**: Generated Rust can be used as a starting point for manual Rust rewrites. PyO3 bindings enable incremental adoption -- transpile a module, validate it, then call it from the remaining Python codebase.

### 1.2 Non-goals

- **Byte-for-byte CPython equivalence**: Molt already explicitly breaks dynamic behaviors (monkeypatching, eval/exec, unrestricted reflection). The Rust backend inherits these scope limits.
- **Full Python**: Only the Molt-supported subset of Python (3.12+) is targeted.
- **Replacing the native backend**: Cranelift remains the primary compilation backend for production binaries. The Rust backend is complementary.

### 1.3 Readability vs performance tradeoff

The backend operates in two modes:

- **`--emit-rust readable`** (default): Prioritize clarity. Named variables, explicit types, no unsafe, liberal use of `.clone()` where ownership is ambiguous. This is the v0 behavior, enhanced with type annotations.
- **`--emit-rust optimized`**: Prioritize performance. Elide clones via escape analysis, use `&str` instead of `String` where lifetimes permit, inline small functions, emit `#[inline]` annotations. May use `unsafe` at documented boundaries.

---

## 2. Type Mapping

### 2.1 Primitive types

| Python type | Rust type (dynamic/v0) | Rust type (specialized) | Notes |
|---|---|---|---|
| `int` | `MoltValue::Int(i64)` | `i64` | Overflow to `i128` or `num_bigint::BigInt` when detected by TIR |
| `float` | `MoltValue::Float(f64)` | `f64` | IEEE 754, matches CPython |
| `bool` | `MoltValue::Bool(bool)` | `bool` | |
| `None` | `MoltValue::None` | `()` or `Option<T>` return | See section 2.4 |
| `str` | `MoltValue::Str(String)` | `String` / `&str` | Owned by default; borrowed when escape analysis proves local-only use |
| `bytes` | `MoltValue::Str(String)` (v0 stub) | `Vec<u8>` | v0 conflates with str; specialized form separates them |

### 2.2 Container types

| Python type | Rust type (dynamic/v0) | Rust type (specialized) | Notes |
|---|---|---|---|
| `list` | `Vec<MoltValue>` | `Vec<T>` when element type is uniform | Falls back to `Vec<MoltValue>` for heterogeneous lists |
| `list[int]` | `Vec<MoltValue>` | `Vec<i64>` | Type hint drives specialization |
| `dict` | `Vec<(MoltValue, MoltValue)>` (v0) | `HashMap<K, V>` | v0 uses linear-scan pair list; specialized uses HashMap |
| `dict[str, int]` | `Vec<(MoltValue, MoltValue)>` | `HashMap<String, i64>` | |
| `tuple` (fixed) | `Vec<MoltValue>` (v0) | `(T1, T2, ..., Tn)` | Rust native tuple for known-length tuples |
| `tuple` (var-length) | `Vec<MoltValue>` | `Vec<T>` | When used as homogeneous sequence |
| `set` | not in v0 | `HashSet<T>` | |
| `frozenset` | not in v0 | `HashSet<T>` (immutable binding) | |

### 2.3 Callable types

| Python construct | Rust type (v0) | Rust type (specialized) |
|---|---|---|
| Module-level function | `fn name(args: &mut Vec<MoltValue>) -> MoltValue` | `fn name(p1: T1, p2: T2) -> R` |
| Closure | `Arc<dyn Fn(&mut Vec<MoltValue>) -> MoltValue>` | `impl Fn(T1, T2) -> R` or boxed `Box<dyn Fn>` |
| Method | dict-based dispatch | `impl StructName { fn method(&self, ...) -> R }` |
| Lambda | same as closure | same as closure |

### 2.4 None and Option semantics

Python's `None` maps differently depending on context:

- **Function return that always returns a value or None**: `-> Option<T>`
- **Function that returns None to signal "no return value"**: `-> ()`
- **Variable that may be None**: `Option<T>`
- **None as a sentinel in a collection**: Keep `MoltValue::None` (no specialization)

The TIR's type facts determine which mapping applies. When type facts are unavailable or `Any`, the variable stays as `MoltValue`.

### 2.5 BigInt handling

Python's `int` is arbitrary precision. Molt's TIR tracks integer width requirements:

- **Proven to fit i64** (most programs): emit `i64`
- **Proven to fit i128** (rare): emit `i128`
- **Unbounded or unknown**: emit `num_bigint::BigInt` with `From<i64>` construction for literals

The backend emits a `Cargo.toml` dependency on `num-bigint` only when BigInt is used.

---

## 3. Ownership and Borrowing

### 3.1 Current state (v0)

Every `MoltValue` use calls `.clone()`. This is correct but expensive. The v0 backend explicitly documents this tradeoff in its header comment: "Variables are universally `MoltValue` and cloned on every use. This is correct-first -- type specialization and borrow elision are future passes."

### 3.2 Escape analysis integration

Molt's TIR already runs escape analysis for the native backend. The Rust backend reuses this analysis to determine ownership:

| Escape classification | Rust ownership | Example |
|---|---|---|
| **Local-only, single use** | Move (no clone) | `x = compute(); return x` -> `let x = compute(); x` |
| **Local-only, multiple reads** | Borrow (`&T`) | `print(x); print(x)` -> `println!("{}", &x); println!("{}", &x)` |
| **Escapes to callee** | Move or clone | `items.append(x)` -> `items.push(x)` (move if last use) |
| **Escapes to return** | Move | `return x` -> `x` (ownership transferred to caller) |
| **Escapes to closure capture** | Clone into closure, or `Rc<T>` | Depends on mutation analysis |
| **Shared mutable** | `Rc<RefCell<T>>` | Multiple references with mutation |

### 3.3 Reference counting to Rc/Arc mapping

Molt's runtime uses manual reference counting (NaN-boxed values with RC headers). In the Rust backend:

- **Single-threaded context**: `Rc<T>` for shared ownership
- **Multi-threaded context** (async, threading): `Arc<T>`
- **No sharing detected**: direct ownership (no Rc/Arc wrapper)

The backend defaults to `Rc` and upgrades to `Arc` only when the TIR marks a value as crossing a thread boundary (e.g., passed to `threading.Thread`, used in `async` task).

### 3.4 Move semantics for last-use variables

When the TIR can prove a variable's last use, the backend emits a move instead of a clone:

```rust
// Python: x = expensive_computation(); return x
// v0 (clone-everything):
let mut x: MoltValue = expensive_computation();
return x.clone();

// v1 (last-use move):
let x = expensive_computation();
x  // moved, no clone
```

This is the highest-impact single optimization for the Rust backend, as it eliminates the majority of clones in typical programs.

### 3.5 Mutable borrows

Python's call-by-object-reference semantics require care:

```python
def modify(lst):
    lst.append(42)

items = [1, 2, 3]
modify(items)
print(items)  # [1, 2, 3, 42]
```

In specialized Rust, `modify` takes `&mut Vec<i64>`. The backend tracks which parameters are mutated and emits `&mut` borrows accordingly. Non-mutated parameters use `&T`.

---

## 4. Error Handling: Exceptions to Result

### 4.1 Strategy

Python exceptions map to Rust's `Result<T, MoltError>`:

```rust
#[derive(Debug, Clone)]
pub enum MoltError {
    ValueError(String),
    TypeError(String),
    IndexError(String),
    KeyError(String),
    AttributeError(String),
    RuntimeError(String),
    StopIteration(Option<Box<MoltValue>>),
    // ... one variant per Python exception class
    Custom { kind: String, message: String },
}
```

### 4.2 Translation rules

| Python construct | Rust translation |
|---|---|
| `raise ValueError("msg")` | `return Err(MoltError::ValueError("msg".into()))` |
| `try: ... except ValueError as e: ...` | `match (|| -> Result<_, MoltError> { ... })() { Ok(v) => v, Err(MoltError::ValueError(e)) => { ... }, Err(e) => return Err(e) }` |
| `try: ... finally: ...` | Scope guard pattern using `defer!` macro or explicit drop guard |
| Implicit exception (e.g., `d[key]` for missing key) | `d.get(&key).ok_or_else(|| MoltError::KeyError(...))?` |

### 4.3 The `?` operator

Functions that can raise exceptions return `Result<T, MoltError>`. The `?` operator propagates errors naturally:

```rust
fn process(data: &str) -> Result<i64, MoltError> {
    let parsed = parse_value(data)?;  // propagates ParseError
    let result = compute(parsed)?;     // propagates ComputeError
    Ok(result)
}
```

### 4.4 Exception chaining

Python's `raise X from Y` maps to a `MoltError` wrapper that carries the cause chain. The `__context__` / `__cause__` distinction is preserved via an optional `cause: Option<Box<MoltError>>` field.

### 4.5 Panic boundaries

`panic!()` is reserved for Molt compiler bugs (unreachable states). User-visible errors always use `Result`. The generated `main()` function wraps the entry point in a catch-unwind for clean error reporting.

---

## 5. Async: Python async/await to Rust async/await

### 5.1 Runtime selection

Generated async Rust uses tokio as the executor, matching Molt's existing async runtime choice:

```rust
#[tokio::main]
async fn main() {
    molt_main().await;
}
```

### 5.2 Translation rules

| Python construct | Rust translation |
|---|---|
| `async def f():` | `async fn f() -> Result<T, MoltError>` |
| `await expr` | `expr.await?` |
| `async for x in aiter:` | `while let Some(x) = aiter.next().await { ... }` |
| `async with ctx:` | Scope guard with async setup/teardown |
| `asyncio.gather(*coros)` | `tokio::join!(c1, c2, ...)` or `futures::future::join_all(vec)` |
| `asyncio.create_task(coro)` | `tokio::spawn(coro)` |
| `asyncio.sleep(n)` | `tokio::time::sleep(Duration::from_secs_f64(n)).await` |

### 5.3 Channels

Python's `asyncio.Queue` maps to `tokio::sync::mpsc`:

```rust
let (tx, mut rx) = tokio::sync::mpsc::channel(capacity);
```

This aligns with Molt's existing `ChanNew`/`ChanSendYield`/`ChanRecvYield` IR ops.

### 5.4 Generator/coroutine translation

Python generators use Rust's `async-stream` or manual state machines:

```rust
// Python: yield x
// Rust (using async-stream):
use async_stream::stream;

fn gen_range(n: i64) -> impl Stream<Item = i64> {
    stream! {
        for i in 0..n {
            yield i;
        }
    }
}
```

For synchronous generators, the backend emits a state-machine struct implementing `Iterator`:

```rust
struct FibIterator { a: i64, b: i64 }
impl Iterator for FibIterator {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        let result = self.a;
        let next = self.a + self.b;
        self.a = self.b;
        self.b = next;
        Some(result)
    }
}
```

---

## 6. Generics: Type Hints to Rust Generics

### 6.1 Monomorphization (default)

Python type hints map to concrete Rust types via monomorphization. When a function is called with `list[int]`, the backend emits a version specialized for `Vec<i64>`:

```python
def sum_list(items: list[int]) -> int:
    total = 0
    for x in items:
        total += x
    return total
```

```rust
fn sum_list(items: &[i64]) -> i64 {
    let mut total: i64 = 0;
    for x in items {
        total += x;
    }
    total
}
```

### 6.2 Generic functions (when type varies across call sites)

When the same function is called with different type arguments, the backend emits a Rust generic:

```rust
fn identity<T: Clone>(x: T) -> T {
    x.clone()
}
```

This is gated by the TIR's interprocedural analysis. Without sufficient type information, the function stays dynamic (`MoltValue` parameters).

### 6.3 Trait mapping

| Python protocol | Rust trait |
|---|---|
| `__iter__` / `__next__` | `Iterator` |
| `__len__` | Custom `Lengthy` trait or direct method |
| `__eq__` / `__hash__` | `Eq + Hash` |
| `__str__` / `__repr__` | `Display` / `Debug` |
| `__add__` / `__mul__` etc. | `Add` / `Mul` etc. from `std::ops` |
| `__enter__` / `__exit__` | `Drop` + scope guard |
| `__getitem__` / `__setitem__` | `Index` / `IndexMut` |

---

## 7. Class Translation

### 7.1 Basic classes

```python
class Point:
    def __init__(self, x: float, y: float):
        self.x = x
        self.y = y

    def distance(self) -> float:
        return (self.x ** 2 + self.y ** 2) ** 0.5
```

```rust
#[derive(Clone, Debug)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn distance(&self) -> f64 {
        (self.x.powi(2) + self.y.powi(2)).sqrt()
    }
}
```

### 7.2 Inheritance

Python single inheritance maps to Rust composition + trait delegation:

```rust
struct Animal { name: String }
struct Dog { base: Animal, breed: String }

impl Dog {
    fn name(&self) -> &str { &self.base.name }
}
```

Multiple inheritance is not supported (Molt explicitly breaks this dynamic behavior). Diamond inheritance patterns are rejected at compile time.

### 7.3 Dynamic attribute access fallback

When the TIR cannot determine a fixed struct layout (e.g., `setattr` on an instance with unknown attributes), the class falls back to the v0 dict-based representation:

```rust
struct DynamicObj {
    type_name: String,
    attrs: HashMap<String, MoltValue>,
}
```

---

## 8. Unsafe Boundaries

### 8.1 Where unsafe is unavoidable

| Boundary | Reason | Mitigation |
|---|---|---|
| FFI calls to C libraries | `extern "C"` requires unsafe | Wrap in safe abstractions; document preconditions |
| Raw pointer manipulation for NaN-boxing interop | Only needed when interfacing with Molt native runtime | Isolate in `molt_ffi` module |
| `transmute` for type punning | Float/int bit reinterpretation | Use `f64::to_bits()` / `f64::from_bits()` instead where possible |
| Uninitialized memory for performance-critical buffers | `MaybeUninit` for large array allocation | Scope to specific hot paths; default to safe initialization |

### 8.2 Unsafe policy

- **Readable mode**: No `unsafe` in generated code. Performance penalty accepted.
- **Optimized mode**: `unsafe` permitted at documented boundaries. Each `unsafe` block includes a `// SAFETY:` comment explaining the invariant.
- Generated code is `#[forbid(unsafe_code)]` by default in readable mode.

---

## 9. Integration: Generated Rust as a Crate

### 9.1 Output structure

```
molt_output/
  Cargo.toml          # Generated, with conditional deps
  src/
    main.rs           # Entry point: fn main() { molt_main(); }
    lib.rs            # pub fn molt_main() + all transpiled functions
    types.rs          # MoltValue enum + helpers (when not using molt-rs crate)
    error.rs          # MoltError enum
  molt-rs/            # Optional: shared runtime crate (for multi-module projects)
    Cargo.toml
    src/lib.rs
```

### 9.2 Cargo.toml generation

```toml
[package]
name = "molt-generated"
version = "0.1.0"
edition = "2021"

[dependencies]
# Conditional: only included when the transpiled code uses them
num-bigint = { version = "0.4", optional = true }
tokio = { version = "1", features = ["full"], optional = true }

[features]
default = []
bigint = ["num-bigint"]
async-runtime = ["tokio"]
```

### 9.3 PyO3 bindings for gradual migration

When `--emit-rust pyo3` is specified, the backend wraps exported functions with PyO3 attributes:

```rust
use pyo3::prelude::*;

#[pyfunction]
fn sum_list(items: Vec<i64>) -> i64 {
    // ... transpiled body
}

#[pymodule]
fn molt_generated(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sum_list, m)?)?;
    Ok(())
}
```

This enables the workflow:
1. Transpile a Python module to Rust
2. Build with maturin: `maturin develop`
3. Import from Python: `from molt_generated import sum_list`
4. Validate correctness against the original Python
5. Gradually migrate callers

### 9.4 Multi-module support

For multi-file Python projects, each Python module becomes a Rust module:

```
my_project/
  main.py      -> src/main.rs
  utils.py     -> src/utils.rs
  models.py    -> src/models.rs
```

Cross-module imports (`from utils import helper`) become Rust `use` statements (`use crate::utils::helper`).

---

## 10. Performance Expectations

### 10.1 Tiered performance model

| Tier | Description | Expected speedup over CPython | Status |
|---|---|---|---|
| **v0 (dynamic)** | All `MoltValue`, universal clone | 1-3x | Implemented (`rust.rs`) |
| **v1 (type-specialized)** | Primitive types unboxed, containers specialized | 5-20x | Design (this doc) |
| **v2 (ownership-optimized)** | Escape analysis, move semantics, borrow elision | 10-50x | Design (this doc) |
| **v3 (LLVM-optimized)** | Generated Rust compiled with `rustc -O` (LLVM backend) | 20-100x | Depends on v2 |

### 10.2 Comparison with Molt's native backend

The native Cranelift backend will generally be faster than transpiled Rust for two reasons:
1. Cranelift compilation is a single step; transpiled Rust requires Rust -> LLVM -> machine code (two optimization passes, slower build).
2. The native backend uses NaN-boxed values with custom calling conventions; transpiled Rust uses standard Rust ABIs.

However, the Rust backend has advantages:
1. **LLVM optimizations**: `rustc` applies LLVM's full optimization suite, which Cranelift does not match for loop-heavy numeric code.
2. **Ecosystem integration**: Generated Rust can link against the entire Rust crate ecosystem without FFI overhead.
3. **Auditability**: Developers can read, modify, and extend the generated code.

### 10.3 Benchmark targets

For the Molt benchmark suite (`bench/`), the specialized Rust backend (v2+) should achieve:

- **Numeric kernels** (fibonacci, mandelbrot, nbody): within 2x of hand-written Rust
- **String processing**: within 3x of hand-written Rust (GC pressure from String allocation)
- **Container-heavy** (dict lookups, list comprehensions): within 5x of hand-written Rust
- **Async I/O**: equivalent to hand-written tokio code (I/O-bound, not compute-bound)

---

## 11. Implementation Roadmap

### Phase 1: Wire up and stabilize v0 (weeks 1-2)
- [ ] Add `pub mod rust;` to `runtime/molt-backend/src/lib.rs`
- [ ] Wire `RustBackend` into CLI: `molt build --emit-rust <file.py>`
- [ ] Add `compile_checked` rejection of stub ops
- [ ] Differential testing: compare transpiled Rust output against CPython for `tests/differential/basic`
- [ ] Fix bytes/str conflation (separate `MoltValue::Bytes(Vec<u8>)` variant)

### Phase 2: Type-specialized emission (weeks 3-6)
- [ ] Read TIR type facts in the Rust backend
- [ ] Emit specialized function signatures when all params/return have known types
- [ ] Emit `i64`/`f64`/`bool`/`String` instead of `MoltValue` for specialized variables
- [ ] Emit `Vec<T>` / `HashMap<K,V>` for typed containers
- [ ] Emit `Option<T>` for nullable values
- [ ] Mixed mode: specialized variables coexist with `MoltValue` via `.into()` / `From` impls

### Phase 3: Ownership optimization (weeks 7-10)
- [ ] Integrate escape analysis results into the backend
- [ ] Emit moves for last-use variables (eliminate clones)
- [ ] Emit `&T` / `&mut T` borrows for non-escaping parameters
- [ ] Emit `Rc<T>` / `Arc<T>` only when sharing is proven necessary
- [ ] Lifetime annotations for functions with borrowed parameters

### Phase 4: Error handling and async (weeks 11-14)
- [ ] Emit `Result<T, MoltError>` instead of panic-on-error
- [ ] Translate try/except to match on Result
- [ ] Emit async functions with tokio
- [ ] Translate generators to Iterator impls

### Phase 5: PyO3 and crate emission (weeks 15-16)
- [ ] Generate `Cargo.toml` with conditional dependencies
- [ ] PyO3 wrapper generation for `--emit-rust pyo3`
- [ ] Multi-module transpilation with cross-module `use` statements
- [ ] `maturin` integration test

---

## 12. Appendix: Op Coverage Matrix

The following table maps SimpleIR op families to their Rust backend implementation status.

| Op family | v0 status | v1 target |
|---|---|---|
| Constants (const, int_const, const_float, const_str, const_bool, const_none, const_bytes, const_bigint) | Implemented | Emit typed literals |
| Variable access (load_local, store_local, load, store, closure_load, closure_store, phi) | Implemented | Emit typed bindings |
| Arithmetic (add, sub, mul, div, floor_div, mod, pow, neg, not) | Implemented | Emit native ops for typed values |
| Bitwise (band, bor, bxor, lshift, rshift) | Implemented | Direct i64 ops (already native) |
| Comparisons (eq, ne, lt, le, gt, ge, is, is_not, in, not_in) | Implemented | Emit typed comparisons |
| Control flow (if, else, end_if, loop_start, loop_end, for_range, for_iter, break, continue, label, jump) | Implemented | No change needed |
| Calls (call, call3, call_method, call_builtin) | Implemented | Emit direct calls for known functions |
| List ops (list_new, list_append, list_extend, index, store_index, len, slice) | Implemented | Emit Vec<T> ops |
| Dict ops (dict_new, dict_set, dict_get, dict_keys, dict_values, dict_items) | Implemented | Emit HashMap ops |
| Tuple ops (tuple_new, tuple_get) | Implemented | Emit native tuple |
| String ops (format, join, split, strip, replace, startswith, endswith, find, upper, lower) | Implemented | Emit String methods |
| Class ops (class_new, attr_get, attr_set, isinstance, method_call) | Partial (dict-based) | Emit struct + impl |
| Exception ops (raise, try_start, try_end, except, check_exception) | Stub | Emit Result<T, E> |
| Async ops (async_call, await, task_new, chan_new, chan_send, chan_recv) | Not implemented | Emit tokio async |
| Generator ops (yield, yield_from, gen_close) | Not implemented | Emit Iterator impl |
| Print/IO (print, print_to) | Implemented | No change needed |

---

## 13. Appendix: Worked Example

### Input Python
```python
def fibonacci(n: int) -> int:
    if n <= 1:
        return n
    a, b = 0, 1
    for _ in range(2, n + 1):
        a, b = b, a + b
    return b
```

### v0 output (current)
```rust
fn fibonacci(args___: &mut Vec<MoltValue>) -> MoltValue {
    let mut n: MoltValue = args___.get(0).cloned().unwrap_or(MoltValue::None);
    if molt_le(&n, &MoltValue::Int(1)) {
        return n.clone();
    }
    let mut a: MoltValue = MoltValue::Int(0);
    let mut b: MoltValue = MoltValue::Int(1);
    for __iter_v in 2..=(molt_int(&n)) {
        let temp = molt_add(a.clone(), b.clone());
        a = b.clone();
        b = temp;
    }
    b.clone()
}
```

### v1 output (type-specialized)
```rust
fn fibonacci(n: i64) -> i64 {
    if n <= 1 {
        return n;
    }
    let mut a: i64 = 0;
    let mut b: i64 = 1;
    for _ in 2..=n {
        let temp = a + b;
        a = b;
        b = temp;
    }
    b
}
```

### v2 output (ownership-optimized)
Identical to v1 for this example (primitives are Copy, no ownership concerns). The difference emerges with heap-allocated types (String, Vec, HashMap).
