# Luau Backend — Known Bugs

Discovered during Vertigo algorithm compilation work (2026-03-08).
Repro artifacts: `tools/kdtree_3d.py`.

---

## Bug 1: Global variable access produces broken subscript/method expressions

**Location:** `molt-backend/src/luau.rs` — `module_get_global` path

**Symptom:** When a Python function accesses a module-level `global` variable,
molt emits:

```luau
molt_module_cache["varname"] or nil
```

If the next operation is a subscript (`[i]`) or method call (`.append()`),
Luau's precedence binds `[i]` to the `nil` side of the `or` expression:

```luau
-- Generated (broken):
molt_module_cache["varname"] or nil[1]   -- syntax error / nil index
molt_module_cache["varname"] or nil.append(x)  -- silent failure

-- Correct:
(molt_module_cache["varname"] or nil)[1]
(molt_module_cache["varname"] or nil).append(x)
```

**Fix:** Wrap the `module_get_global` expression in parentheses when it is
the left-hand side of a subscript or method call operation.

**Workaround:** Avoid `global` declarations; pass the value as a function
parameter instead.

---

## Bug 2: List `type_hint` not propagated through function parameters or subscript results

**Location:** `molt-backend/src/luau.rs` — `type_hint` propagation logic

**Symptom:** `type_hint="list"` is only set when a variable is assigned
directly from a `[]` literal in the same function scope. If a list is:
- Passed as a function argument, OR
- Obtained by subscripting another list (`pool[i]`)

…the `type_hint` is lost. Subsequent `.append()` calls then emit as method
calls (no Luau equivalent), silently failing at runtime instead of emitting
`table.insert()`.

**Example:**
```python
def build(pool: list[list[int]], i: int) -> None:
    node = pool[i]   # type_hint="list" lost here
    node.append(1)   # → emitted as node.append(1), not table.insert(node, 1)
```

**Fix:** Propagate `type_hint="list"` through:
1. Function parameter types when the caller passes a known-list variable
2. Subscript results when the receiver is `type_hint="list"` (i.e., a
   `list[list[T]]` subscript yields `type_hint="list"`)

**Workaround:** Use flat parallel arrays (`list[int]` at top level) instead
of nested lists. This avoids the subscript-of-list pattern entirely.

---

## Impact

Both bugs prevent list-heavy algorithms (KD-tree, A*, spatial hash) from
compiling correctly via `--target luau`. Pure scalar arithmetic (Catmull-Rom,
spring/verlet) compiles cleanly.

**Affected targets:** `--target luau` only. Native and WASM targets are unaffected
since they use Rust's type system rather than runtime type hints.

---

## Testing

Minimal repro:

```python
# global_subscript_repro.py
pool: list[list[int]] = [[1, 2, 3], [4, 5, 6]]

def get_first(i: int) -> int:
    global pool
    return pool[i][0]  # Bug 1: pool[i][0] → (pool_global or nil)[i][0] → parse error

# param_list_repro.py
def append_to(xs: list[int], v: int) -> None:
    xs.append(v)  # Bug 2: type_hint lost, emits xs.append(v) not table.insert(xs, v)

def caller() -> list[int]:
    result: list[int] = []
    append_to(result, 42)
    return result
```
