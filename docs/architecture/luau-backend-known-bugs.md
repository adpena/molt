# Luau Backend — Historical Bug Ledger

Discovered during Vertigo algorithm compilation work (2026-03-08).
Repro artifacts: `tools/kdtree_3d.py`.

Current status: this file records historical repros. It is not the canonical
support matrix. As of 2026-04-23, checked Luau emission rejects unsupported
markers by default, user functions receive `@native`/parameter annotations
where available, list-parameter append lowering has regression coverage, and
goto/label comment emission has regression coverage. Remaining entries below
must be treated as repro candidates until covered by fresh CPython-vs-Lune tests.

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

**Current action:** Re-verify with a targeted CPython-vs-Lune regression. Do
not document a workaround as an accepted support path.

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

**Current status:** Function parameter list hints are covered by
`test_param_type_hint_list_propagation` in `runtime/molt-backend/src/luau.rs`.
Nested-list propagation still requires fresh end-to-end differential coverage.

---

---

## Bug 3: `math.floor` (and other stdlib attribute accesses) not resolved

**Location:** `molt-backend/src/luau.rs` — module attribute resolution for `math`

**Symptom:** `math.floor(x)` inside a function emits a `MODULE_GET_ATTR` call
through the module cache mechanism rather than mapping directly to Luau's
built-in `math.floor`. The generated code calls into the molt runtime module
system and may return nil or error if the `math` module binding isn't present
in the Luau environment.

**Fix:** Add a direct-call mapping for `math.*` functions to their Luau
equivalents (`math.floor`, `math.sin`, `math.cos`, etc.) in the Luau backend's
stdlib direct-call table, bypassing the module cache path.

**Current action:** Re-verify direct `math.*` calls through the current module
bridge and add CPython-vs-Lune coverage for each admitted math attribute.

---

## Bug 4: `if/elif/else` chains with nil-reset pattern — goto emitted as comment

**Location:** `molt-backend/src/luau.rs` — control flow emission for if/elif/else

**Symptom:** When molt emits `if/elif/else` chains that need a goto-based
fallthrough (to implement Python's fall-through semantics for elif branches
that set variables), it emits the goto as a Luau comment `-- goto label`
rather than a real control flow construct. This causes variables that should
be set in later `elif` branches to retain their initial nil values, producing
silent logic errors rather than compile errors.

**Example:** A 6-way if/elif/else that assigns `i1`, `j1`, `k1`, `i2`, `j2`, `k2`
based on coordinate ordering (the simplex skew-tetrahedron selection in 3D
gradient noise) should set all 6 variables in every branch. If the goto
comment bug fires, only the first matching branch's assignments are visible.

**Fix:** Implement proper Luau `do/break` blocks or label/goto pairs for
elif chains instead of emitting goto as a comment.

**Current status:** Goto/label emission as comments has regression coverage in
`runtime/molt-backend/src/luau.rs`; complex multi-branch value propagation still
requires fresh differential coverage.

---

## Bug 5: Tuple returns produce nil in callers

**Location:** `molt-backend/src/luau.rs` — multiple return value handling

**Symptom:** Python functions returning tuples (e.g., `return x, y, z`) emit
correct Luau multi-return syntax, but callers that destructure the result
(`a, b, c = f()`) sometimes receive nil if the function went through the
module cache path or if the return passes through an intermediate variable.

**Current action:** Add explicit tuple/multi-return differential tests. Checked
Luau output must fail closed if a tuple path cannot be emitted faithfully.

---

## Impact

Bugs 1–5 prevent list-heavy or control-flow-rich algorithms (KD-tree, A*,
spatial hash, simplex noise) from compiling correctly via `--target luau`.
Pure scalar arithmetic with simple if/else (Catmull-Rom, spring/verlet)
compiles cleanly — the Catmull-Rom spline (82 lines) compiled to 540 lines
of valid Luau with zero errors.

**Affected targets:** `--target luau` only. Native and WASM targets are unaffected
since they use Rust's type system rather than runtime type hints.

**Current validation posture:** support claims require fresh checked-build,
static-analysis, and CPython-vs-Lune evidence. Historical working patterns are
not accepted as workarounds for unsupported semantics.

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
