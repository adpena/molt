# Molt `--target rust` Transpiler Backend

**Date:** 2026-03-07
**Status:** In Progress (VER-65 / VER-70)
**Author:** Claude Code

## Overview

Add Rust source code generation to the Molt compiler backend, enabling the pipeline:

```
Python 3.12+ → Molt frontend → SimpleIR → RustBackend → .rs file → rustc → native binary
```

This complements the existing targets:
- `--target luau` → Luau source (Roblox VM)
- `--target wasm` → WASM binary (browser/server)
- native → Cranelift object file

The Rust target is the only one that produces editable, human-readable **systems-grade** source.
It enables open-source redistribution of Molt-generated code in a universally compilable form.

## Architecture

### SimpleIR → Rust Mapping

The IR is a linear sequence of typed ops per function. The Rust backend lowers this to:

| IR concept | Rust output |
|---|---|
| Module-level code | `fn molt_main() { ... }` + `fn main() { molt_main(); }` |
| Python function | `fn name(args: Vec<MoltValue>) -> MoltValue { ... }` |
| Python variable | `let mut v0: MoltValue = ...;` |
| Python `None` | `MoltValue::None` |
| Python int | `MoltValue::Int(i64)` |
| Python float | `MoltValue::Float(f64)` |
| Python str | `MoltValue::Str(String)` |
| Python bool | `MoltValue::Bool(bool)` |
| Python list | `MoltValue::List(Vec<MoltValue>)` |
| Python dict | `MoltValue::Dict(Vec<(MoltValue, MoltValue)>)` |
| Python closure | `MoltValue::Func(Arc<dyn Fn(Vec<MoltValue>) -> MoltValue + Send + Sync>)` |
| `if` / `else` / `end_if` | `if molt_bool(&cond) { } else { }` |
| `loop_start` / `loop_end` | `loop { }` |
| `for_range` / `end_for` | `for v in molt_range_iter(start, stop, step) { }` |
| `for_iter` / `end_for` | `for v in molt_iter(&iterable) { }` |
| SSA phi nodes | hoisted `let mut` at function top, assigned in branches |

### MoltValue Type

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum MoltValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<MoltValue>),
    Dict(Vec<(MoltValue, MoltValue)>),  // ordered, linear scan
    Func(Arc<dyn Fn(Vec<MoltValue>) -> MoltValue + Send + Sync>),
}
```

Dict uses `Vec<(K,V)>` instead of `HashMap` to:
- Avoid the `Hash` constraint problem (floats can't implement `Hash` in Rust)
- Preserve insertion order (matches Python semantics)
- Keep generated code dependency-free

For typical Python algorithmic code, dict sizes are small and linear scan is negligible.

### Runtime Helpers (Conditional Prelude)

Only helpers actually referenced in the generated function bodies are emitted:

| Helper | Purpose |
|---|---|
| `molt_int(x)` | Coerce to i64 |
| `molt_float(x)` | Coerce to f64 |
| `molt_str(x)` | Python `str()` — handles None → "None", bool → "True"/"False" |
| `molt_bool(x)` | Python truthiness — empty list/dict/str/None/0 are falsy |
| `molt_repr(x)` | Python `repr()` — adds quotes around strings |
| `molt_len(x)` | Python `len()` |
| `molt_print(args)` | Python `print()` — space-joined molt_str of each arg |
| `molt_range_iter(start, stop, step)` | Returns `impl Iterator<Item=MoltValue>` |
| `molt_add(a, b)` | Polymorphic + (int+int, float+float, str+str, list+list) |
| `molt_sub`, `molt_mul`, `molt_div` | Polymorphic arithmetic |
| `molt_floor_div`, `molt_mod`, `molt_pow` | Floor div, modulo, power |
| `molt_eq`, `molt_ne`, `molt_lt`, … | Comparison → bool |
| `molt_neg` | Unary negation |
| `molt_not` | Logical not |
| `molt_get_item(obj, key)` | Index: list[i], dict[k] |
| `molt_set_item(obj, key, val)` | Assign: obj[k] = v |
| `molt_get_attr(obj, name)` | Attribute: obj.attr |
| `molt_list_append(list, val)` | list.append(val) |
| `molt_enumerate(t, start)` | enumerate() |
| `molt_zip(a, b)` | zip() |
| `molt_sorted(t)` | sorted() |
| `molt_reversed(t)` | reversed() |
| `molt_sum(t)` | sum() |
| `molt_any(t)`, `molt_all(t)` | any(), all() |
| `molt_map(f, t)` | map() |
| `molt_filter(f, t)` | filter() |
| `molt_dict_keys`, `molt_dict_values`, `molt_dict_items` | dict methods |

### Variable Hoisting

Same strategy as the Luau backend:
1. Scan for phi nodes following `end_if` — hoist phi output vars to function top
2. Scan for variables declared inside blocks but referenced outside — hoist those too
3. Emit `let mut var: MoltValue = MoltValue::None;` at function top
4. Replace `let mut var = ...` with `var = ...` inside blocks for hoisted vars

### Optimization Passes

Text-level passes run on the function bodies before prelude scanning:

1. **inline_int_literals** — replace `molt_add(MoltValue::Int(3), MoltValue::Int(4))` with `MoltValue::Int(3 + 4)` where both sides are literal
2. **fast_int_path** — when `op.fast_int == true`, emit raw `i64` arithmetic bypassing MoltValue
3. **strip_unused_mut** — detect vars never reassigned after declaration, remove `mut`
4. **dead_assignment_strip** — remove `var = ...; drop(var);` patterns

## File Changes

### New files
- `runtime/molt-backend/src/rust.rs` — `RustBackend` (~1200 lines)

### Modified files
- `runtime/molt-backend/src/main.rs` — wire `--target rust` / `is_rust` flag
- `runtime/molt-backend/src/lib.rs` — `pub use rust::RustBackend`
- `src/molt/cli.py` — `is_rust = target == "rust"`, emit mode, output path, daemon job

## Companion crate: `molt-rs`

Future: publish `molt-rs` on crates.io with just the `MoltValue` type and runtime helpers.
Users who want to integrate Molt-generated Rust can then:

```toml
[dependencies]
molt-rs = "0.1"
```

And the generated code uses `molt_rs::MoltValue` instead of the inline prelude.
This is VER-70 and will be done after the inline approach is proven.

## Non-goals

- **Idiomatic typed Rust**: The output uses `MoltValue` uniformly. Type inference/specialization to concrete Rust types is a future optimization.
- **Zero-copy**: All values are cloned on use. Lifetime analysis is deferred.
- **Async**: `call_async`, `block_on`, channels — not in scope for v1.
- **Classes**: Python class system (alloc_class, class_new, etc.) — stub emit only.
- **Exception handling**: `raise`, `try`/`except` — stub emit with `panic!()`.
