# Luau Backend Optimization Guide

Status: **Living document** | Last updated: 2026-03-12
Backend source: `runtime/molt-backend/src/luau.rs` (~5200 lines)
Formal models: `formal/lean/MoltTIR/Backend/Luau*.lean` (5 files), `formal/quint/molt_luau_transpiler.qnt`

---

## 1. Current Luau Emission Quality

Canonical generated OpIR support matrix:
`docs/spec/areas/compiler/luau_support_matrix.generated.md`. Update it with
`python3 tools/gen_luau_support_matrix.py --write` whenever Luau lowering moves,
and gate with `python3 tools/gen_luau_support_matrix.py --check`.

### 1.0 Modern Luau Target Baseline

Molt targets the current and forward Luau surface, relying on Luau's own broad
backward compatibility instead of adding Molt-side legacy Lua shims. The backend
may use modern Luau syntax and APIs when they improve correctness or performance:

- `--!strict` / `--!native` file directives and function-level `@native`.
- Luau type annotations for locals, parameters, returns, arrays (`{T}`), dictionaries (`{[K]: V}`), and casts (`::`).
- Luau expression forms already emitted by the backend, including if-expressions and compound assignment.
- Current table APIs: `table.create`, `table.find`, `table.clear`, `table.freeze`, `table.isfrozen`, `table.clone`, `table.move`, and `table.unpack`.
- Current buffer APIs for future bytes/bytearray/memoryview lowering where the Python surface can be represented faithfully.

Do not add legacy-Lua fallback syntax or Molt compatibility shims that obscure
the emitted program. If a Python feature cannot be represented faithfully on
current/future Luau, checked Luau emission must fail closed until the feature is
implemented against the modern Luau API surface.

### 1.1 Compilation Pipeline

The Luau backend (`LuauBackend` in `luau.rs`) transpiles Molt's `SimpleIR` to Luau source text in four phases:

1. **Function body emission** -- walks `FunctionIR` ops, emitting Luau statements per IR op
2. **Text-level optimization** -- 11 post-processing passes on the emitted source string
3. **Conditional prelude scan** -- emits only runtime helpers actually referenced in the body
4. **Assembly** -- combines prelude + optimized body, writes `--!strict` header

### 1.2 Python Construct Mapping

| Python construct | Luau emission | Quality |
|---|---|---|
| Integer arithmetic (`+`, `-`, `*`) | Direct operators (`+`, `-`, `*`) | Optimal |
| Float division (`/`) | Direct `/` | Optimal |
| Floor division (`//`) | zero-check + direct Luau `//` | Good -- no helper call |
| Modulo (`%`) | Direct `%` operator | Good -- matches Python for positive divisors |
| Power (`**`) | Direct `^` operator | Good -- inlined from `molt_pow()` |
| Bitwise ops | `bit32.band/bor/bxor/lshift/rshift` | Correct -- 32-bit limitation |
| Boolean `not` | `not` | Optimal |
| Comparison ops | Direct `<`, `<=`, `==`, `~=` etc. | Optimal |
| List literal `[]` | `{}` table constructor | Optimal |
| Dict literal `{}` | `{[k1]=v1, ...}` keyed table | Optimal |
| Set `set()` | `{}` table (values as keys mapped to `true`) | Correct |
| `range(start, stop, step)` | `molt_range()` helper or `for i = start, stop-1, step do` | Good |
| `for x in iterable` | `for _, x in ipairs(t) do` (via `lower_iter_to_for`) | Good |
| `list.append(x)` | `list[#list + 1] = x` when list-typed | Good -- no helper call |
| `dict[key] = val` | `dict[key] = val` | Optimal |
| `len(x)` | `molt_len(x)` helper | Correct |
| `print(...)` | `molt_print(...)` helper (Python-style formatting) | Correct |
| `str(x)` | `molt_str(x)` helper (Python-style `True`/`False`/`None`) | Correct |
| String concat (`+`) | Type-guarded `..` or `+` depending on `fast_int` hint | Acceptable |
| String methods (`.upper()`, `.split()`, etc.) | Mapped to `string.*` or `molt_string.*` | Partial |
| `if/elif/else` | Structured `if/else/end` plus checked goto elimination for remaining jump patterns | Partial -- complex branch propagation needs differential coverage |
| `while` loop | `while true do ... break end` | Correct |
| `for i in range(n)` | `for i = 0, n-1, 1 do` (via `for_range`) | Optimal |
| Function def/call | `local function f(...) ... end` / `f(...)` | Correct |
| Closures/upvalues | `__closure_*` slot variables | Correct |
| Tuple return | `return a, b, c` or `return table.unpack(t)` | Partial (Bug 5) |
| Exception `raise` | `error(msg)` | Minimal |
| Try/except | `pcall` lowering for admitted patterns | Partial -- checked output rejects remaining semantic stubs |
| Generator/yield | `coroutine.yield` / `coroutine.wrap` | Correct for listcomp/genexpr |
| Class/object | Empty table `{}` | Minimal -- no method dispatch |
| `import math` | `molt_math` prelude table | Correct |
| `import json` | `json` prelude (inline serializer) | Correct |

### 1.3 Post-Emission Optimization Passes

The backend applies 11 text-level optimization passes after initial emission (see `compile()`, lines 82-95):

| Pass | Function | Effect |
|---|---|---|
| 1 | `inline_single_use_constants` | Inlines `local vN = <literal>` when vN used once |
| 2 | `eliminate_nil_missing_wrappers` | Removes `nil -- [missing]` default-arg sentinels |
| 3 | `strip_unbound_local_checks` | Removes Python `UnboundLocalError` guards (dead in Luau) |
| 4 | `strip_dead_locals_dict_stores` | Removes frame introspection `__locals__` writes |
| 5 | `strip_undefined_rhs_assignments` | Removes dead closure-restore `vN = vM` where vM undefined |
| 6 | `propagate_single_use_copies` | `local vN = vM` where vN used once -> replace with vM (3 iters) |
| 7 | `strip_trailing_continue` | Removes `continue` before `end` (no-op) |
| 8 | `simplify_comparison_break` | Folds `local vN = a < b; if not vN then break end` -> `if a >= b then break end` |
| 9 | `optimize_luau_perf` | Inlines `molt_pow/molt_floor_div/molt_mod`, simplifies type-guard adds and index guards |
| 10 | `propagate_single_use_copies` (2nd) | Catches copies unlocked by pass 9 |
| 11 | `sink_single_use_locals` | Sinks `local vN = expr` into next-line consumer (5 iters) |
| 12 | `simplify_return_chain` | Folds `vN = expr; return vN` -> `return expr` |

IR-level passes applied before emission in `emit_function_body`:

| Pass | Function | Effect |
|---|---|---|
| `lower_early_returns` | Converts `store_index(retval) + jump(exit)` -> direct `ret` |
| `strip_dead_after_return` | Removes unreachable code after returns |
| `lower_iter_to_for` | Converts `iter + while + iter_next + get_item` -> `for_iter` |

### 1.4 Formal Verification Coverage

The Lean formalization (`formal/lean/MoltTIR/Backend/`) provides:

- **LuauSyntax.lean**: Luau AST definition (expressions, statements, functions, modules)
- **LuauEmit.lean**: Translation from MoltTIR IR to Luau AST, including index adjustment, operator mapping, builtin resolution
- **LuauSemantics.lean**: Evaluation model for Luau expressions/statements (total, deterministic evaluator)
- **LuauEnvCorr.lean**: Environment correspondence between IR SSA env and Luau string-named env, with injectivity proof
- **LuauCorrect.lean**: 20+ correctness theorems:
  - `index_adjust_correct`: 0-based + 1 = 1-based index
  - `emitExpr_correct`: Full expression emission semantic correctness (structural induction)
  - `emitBinOp_correct_{add,sub,mul,mod,eq,lt}`: Operator correspondence proofs
  - `emitUnOp_correct_{neg,not}`: Unary operator correspondence
  - `emitInstr_preserves_env`: Instruction emission preserves environment correspondence
  - `emitBuiltinCall_arity`: Builtin call arity preservation

The Quint model (`formal/quint/molt_luau_transpiler.qnt`) verifies:
- Index adjustment invariant (all emitted indices are 1-based)
- Prelude completeness (all referenced helpers are emitted)
- No nil callables (all builtin references resolve)
- Variable declaration tracking (all used variables are declared)

**Known proof gap**: `emitUnOp` maps `abs` to `neg` as an approximation (the real backend uses `math.abs`). The `emitExpr_correct` proof has a `sorry` for this case.

---

## 2. Luau-Specific Optimizations

### 2.1 Type Annotations

Luau uses gradual typing. **Type annotations directly influence native codegen quality**:

- Annotated function parameters enable the JIT to generate specialized code paths without runtime type guards
- Vector3-typed parameters trigger SIMD-optimized paths in the native compiler
- Incorrectly typed calls (e.g., passing `true` to a `string` param) cause de-optimization

**Current state**: The Molt backend emits type annotations on prelude helpers and maps user function parameter type hints through `python_type_to_luau`. Return types are not yet emitted.

**Recommendation**: Extend Molt's type inference propagation into return annotations and nested container annotations. For functions where all parameters and returns have known types (int, float, string, bool), emit:
```luau
local function user_func(x: number, name: string): number
```
This would enable native codegen to skip type guards on every function entry.

### 2.2 The `@native` Attribute

Luau's `@native` attribute requests native compilation for individual functions. It does **not** apply recursively to inner functions.

**Current state**: The backend emits `--!native` at file level, emits `@native` on several hot helpers, and emits `@native` before local user-defined functions that are not assigned through forward declarations. The remaining optimization problem is selectivity and native-code budget management, not absence of annotations.

**Recommendation**: Keep `@native` on hot user functions that:
1. Contain tight loops (for-range, while-true)
2. Are arithmetic-heavy (many numeric ops, no string/table construction)
3. Do not contain coroutine/yield operations (native codegen may not support these)

The `@native` attribute costs memory for compiled code (Roblox enforces a per-experience code size budget), so it should be selective rather than blanket.

### 2.3 Table Layout Hints

Luau tables have two parts: **array** (integer keys 1..n) and **hash** (everything else). The array part is faster for sequential access.

**Current state**: The backend emits many collection constructors as `{}` and uses direct indexed assignment for typed list append. Some helpers and repeat/range paths already use `table.create`; broader preallocation is still incomplete. Dicts use `[key] = val` (hash part).

**Optimization opportunities**:

1. **`table.create(n)` pre-allocation**: When list size is known at construction or bounded by a range, emit `table.create(n)` instead of `{}`. The backend already does this for some helpers (e.g., `molt_reversed` uses `table.create(len)`) but not for user code.

2. **Inline array construction**: For `list_repeat_range`, emit `table.create(count, val)` instead of a loop:
   ```luau
   -- Current: local v1 = {}; for __i = 1, count do table.insert(v1, val) end
   -- Better:  local v1 = table.create(count, val)
   ```

3. **Avoid remaining `table.insert()` in hot loops**: Prefer indexed assignment `result[n] = x; n += 1` where order and shifting semantics do not require `table.insert`.

### 2.4 Local Variable Optimization

Luau optimizes locals significantly better than upvalues. The Luau compiler performs:
- **Constant folding** across local variables
- **Upvalue mutation analysis**: 90%+ of upvalues are immutable, enabling direct stack references
- **Closure caching**: Identical closure expressions reuse the same object
- **Load-store propagation**: Multiple reads of the same upvalue slot are coalesced

**Current state**: The backend declares variables with `local` at function scope (hoisting via `hoisted_vars`). Closure slots use `__closure_*` variables. The `propagate_single_use_copies` pass eliminates many intermediate locals.

**Remaining issue**: The hoisting pass emits `local vN` declarations at function top for phi variables and scope-escaping variables. This creates many uninitialized locals that occupy register slots. Consider:
- Narrowing hoisted declarations to the nearest enclosing block
- Merging phi variables with their branch sources when possible

### 2.5 String Interning and Buffer Reuse

Luau interns short strings automatically. The backend's string emission is straightforward (`escape_luau_string` handles `\n`, `\r`, `\t`, `\0`, `\\`, `\"`).

**Optimization opportunity**: For string concatenation chains (`a .. b .. c .. d`), Luau builds intermediate strings. Use `table.concat` for 3+ concatenations:
```luau
-- Current: a .. " " .. b .. " " .. c
-- Better:  table.concat({a, " ", b, " ", c})
```

The `molt_print` helper already uses `table.concat(parts, " ")` for multi-arg prints.

### 2.6 Native Codegen: Which Ops Get JIT'd

From Roblox's native codegen documentation and the Luau source:

**Well-optimized in native mode**:
- Arithmetic on numbers (`+`, `-`, `*`, `/`, `%`, `^`)
- Comparison operators
- Local variable access
- `for i = start, stop, step do` numeric loops
- `bit32.*` operations (guard-based codegen, ~30% faster than interpreter)
- `math.*` builtins (sin, cos, floor, etc.)
- Table array-part access with integer keys
- String length (`#s`)

**Not natively compiled / interpreter fallback**:
- `pcall`, `xpcall` (exception handling)
- `coroutine.*` operations
- `string.gsub` with function callbacks
- Metatables / `__index` chains
- Dynamic dispatch (`obj[computed_key]`)
- `table.sort` with custom comparators (the sort itself is native, the comparator callback falls back)

**Implication for Molt**: The current exception stub (`error()`) and coroutine-based generators will not benefit from `@native`. Focus native annotation on arithmetic-heavy and loop-heavy user functions.

---

## 3. Python to Luau Semantic Gaps

### 3.1 Integer Overflow

**Python**: Arbitrary-precision integers. `2**100` is exact.
**Luau**: IEEE 754 doubles. Safe integer range is `|n| < 2^53` (9,007,199,254,740,992).

The Lean formalization explicitly acknowledges this: "Molt only compiles programs that use integers within the safe-integer range" (LuauSemantics.lean, line 13). Beyond 2^53, integer arithmetic silently loses precision.

**Current handling**: The backend emits `const_bigint` as `tonumber("N") or 0`, which truncates silently. No runtime overflow check exists.

**Recommendation**: For programs that use large integers (cryptography, combinatorics), emit a compile-time warning or error. For standard programs, document the 2^53 limit.

### 3.2 String Indexing

**Python**: 0-based, `s[0]` is first character. `s[-1]` is last.
**Luau**: 1-based, `string.sub(s, 1, 1)` is first character.

**Current handling**: The backend adjusts numeric indices with `+ 1` (proven correct in `index_adjust_correct`). For string indexing, `ord` maps to `string.byte(s, 1)`. Negative indexing is not handled -- would need `#s + idx + 1` adjustment.

### 3.3 None vs nil

**Python**: `None` is a singleton object. `x is None` tests identity.
**Luau**: `nil` is the absence of a value. `x == nil` tests equality.

**Current handling**: `const_none` emits `nil`. `is` comparison emits `==`. This is semantically correct for `None` but incorrect for general `is` comparisons (Python `is` tests object identity, not value equality). The backend maps both `is` and `eq` to `==`.

### 3.4 Boolean Representation

**Python**: `True`/`False` are instances of `int` subclass. `True + True == 2`.
**Luau**: `true`/`false` are not numbers. `true + true` is a type error.

**Current handling**: `molt_bool(x)` truthiness function handles Python semantics (empty table/string/0/nil/false are falsy). Known boolean operands in arithmetic lower through inline Luau if-expressions (`if b then 1 else 0`) to preserve Python's `bool`-as-`int` behavior without helper calls or allocation. Remaining work: add end-to-end CPython-vs-Lune coverage for mixed bool/int/float arithmetic and unsupported object overload paths.

### 3.5 Floor Division and Modulo Edge Cases

**Python**: `//` and `%` use floor division (round toward negative infinity).
**Luau**: `%` uses floor semantics inherited from Lua, matching Python for all cases.

**Current handling**: The backend emits direct `%` operator (line 849). The `molt_mod` helper uses `a - math.floor(a / b) * b` for correctness but `optimize_luau_perf` inlines it to `%`. This is correct -- Luau's `%` uses floor semantics.

### 3.6 Dictionary Ordering

**Python 3.7+**: Dicts preserve insertion order.
**Luau**: `pairs()` iteration order is not guaranteed to match insertion order.

**Current handling**: No special handling. Programs relying on dict insertion order will behave differently in Luau.

---

## 4. Calling Convention

### 4.1 Current Convention

The Molt IR uses a callargs-tuple convention for dynamic dispatch:

1. Build args table: `callargs_new` -> `callargs_push_pos` (repeated) -> `callargs_push_kw`
2. Call via `call_bind`: `func(table.unpack(args_tuple))`

For direct calls, the convention is simpler: `func(arg1, arg2, arg3)`.

**Overhead of callargs path**: Each `callargs_push_pos` emits a `table.insert()` call. The `call_bind` op emits `table.unpack()` to spread args. This means:
- N allocations for N positional args (table grows)
- 1 `table.unpack` call (creates N stack values from table)
- GC pressure from the temporary args table

### 4.2 Builtin Function Wrappers

The `builtin_func` op wraps Luau functions in closures that unpack args tuples:
```luau
local v42 = function(a, ...) return math.max(table.unpack(a)) end
```

This adds one closure allocation per builtin reference plus unpack overhead per call. For hot builtins (`len`, `print`, `int`, `str`), this is significant.

### 4.3 Optimization Opportunities

1. **Direct call lowering**: When the frontend can statically resolve the callee (most cases), emit direct `f(a, b, c)` instead of going through callargs. The `call` op already does this for `s_value` function names.

2. **Eliminate wrapper closures**: For builtins called through `call_func`, emit the mapped call directly instead of creating a wrapper closure. For example, `len(x)` through the builtin path currently creates a closure `function(a, ...) return molt_len(a[1]) end` -- the closure is never reused.

3. **Inline small builtins**: `molt_len(x)` is `if type(x) == "string" then #x end; if type(x) == "table" then #x end; return 0`. For known-table or known-string args, inline to just `#x`.

---

## 5. Runtime Library

### 5.1 Prelude Helpers

The backend conditionally emits 20+ runtime helpers. Each is only included if referenced:

| Helper | Purpose | Native? |
|---|---|---|
| `molt_range` | Python `range()` -> array | Yes (`@native`) |
| `molt_len` | `len()` with type dispatch | No |
| `molt_int` | `int()` conversion | No |
| `molt_float` | `float()` conversion | No |
| `molt_str` | `str()` with Python formatting | No |
| `molt_bool` | Python truthiness | No |
| `molt_repr` | `repr()` with quote wrapping | No |
| `molt_floor_div` | Floor division helper | No (inlined by optimizer) |
| `molt_pow` | Power helper | No (inlined by optimizer) |
| `molt_mod` | Floor modulo helper | No (inlined by optimizer) |
| `molt_enumerate` | `enumerate()` | No |
| `molt_zip` | `zip()` | No |
| `molt_sorted` | `sorted()` via `table.sort` | No |
| `molt_reversed` | `reversed()` | Yes (`@native`) |
| `molt_sum` | `sum()` | Yes (`@native`) |
| `molt_any` | `any()` | No |
| `molt_all` | `all()` | No |
| `molt_map` | `map()` | Yes (`@native`) |
| `molt_filter` | `filter()` | No |
| `molt_print` | `print()` with Python formatting | No |
| `molt_dict_keys/values/items` | Dict view operations | No |

### 5.2 Module Bridges

| Module | Bridge | Completeness |
|---|---|---|
| `math` | `molt_math` table mapping to `math.*` | Good (14 functions + constants) |
| `json` | Inline serializer (`luau_json_prelude.luau`) | Serialize only, no parse |
| `time` | `molt_time` mapping to `os.clock` / `task.wait` | Minimal |
| `os` | `molt_os` stub (getcwd, getenv, path.join) | Minimal |
| `string` | `molt_string` (format, join, split) | Partial |

### 5.3 Missing Runtime Support

The following Python surfaces are not fully admitted in checked Luau output. If
they still lower to semantic nil/stub markers, checked emission rejects them:
- `open()` / file I/O (Roblox sandbox prohibits filesystem access)
- malformed or unsupported `isinstance()` / `issubclass()` forms
- `super()` / class inheritance
- `property` / `classmethod` / `staticmethod`
- `getattr()` / `setattr()` / `delattr()` with dynamic names
- `async` / `await`

---

## 6. Roblox Platform Specifics

### 6.1 Memory Limits

- **Native code budget**: Roblox enforces a per-experience limit on native-compiled code size. When reached, remaining functions fall back to interpreter. Use `debug.dumpcodesize()` to monitor.
- **Luau VM heap**: No hard per-script limit documented, but total experience memory is bounded (~3-4 GB on client, varies by platform). Table-heavy transpiled code should pre-allocate with `table.create()`.
- **String interning**: Short strings are automatically interned. Long strings (>40 bytes) are not. Avoid generating many unique long string constants.

### 6.2 Script Size Limits

- No documented hard limit on `.luau` source size, but large scripts increase load time and bytecode compilation cost.
- The `--!strict` type checking mode adds compilation overhead proportional to source size.
- **Recommendation**: For large transpiled programs, consider splitting into multiple ModuleScripts with `require()`.

### 6.3 Security Sandbox

Roblox's Luau sandbox restricts:
- **No filesystem access**: `io.*`, `os.execute`, `loadfile` are absent
- **No network access**: No raw sockets or HTTP from scripts (use `HttpService` API)
- **No `loadstring`**: Dynamic code loading is disabled (matches Molt's no-eval policy)
- **No `debug.*` mutation**: `debug.setmetatable`, `debug.setupvalue` etc. are removed
- **No `os.exit`**: Process control is unavailable

The Molt backend's sandbox alignment is good -- Molt already forbids dynamic code execution and unrestricted reflection, matching Roblox's restrictions.

### 6.4 Roblox API Integration

Transpiled Luau runs in Roblox Studio/runtime context where:
- `task.wait(n)` replaces `time.sleep(n)` (the backend maps this correctly)
- `print()` outputs to the Roblox output console
- `game`, `workspace`, `Players` etc. are available as globals but not modeled by the backend
- `Instance.new()`, `:GetService()` etc. are the primary Roblox APIs -- these are opaque to Molt and would need explicit FFI bridging

---

## 7. Known Bugs and Limitations

See `docs/architecture/luau-backend-known-bugs.md` for historical repros that
still need fresh checked-build and CPython-vs-Lune classification. Do not treat
historical workaround patterns as accepted support lanes.

---

## 8. Optimization Roadmap

### P0 -- Correctness (blocks production use)
- Generate and gate a Luau support matrix for every emitted `OpIR` kind.
- Add fresh CPython-vs-Lune regressions for historical global/module, nested-list, math-attribute, elif, and tuple-return repros.
- Keep checked emission fail-closed for all unsupported markers and semantic stubs.

### P1 -- Performance (measurable impact)
- Add native-code budget tracking for `@native` user functions with loops/arithmetic.
- Emit Luau return annotations from Molt type inference (number, string, boolean returns).
- Replace remaining avoidable `table.insert(t, v)` with `t[#t+1] = v` or counter-based `t[n] = v; n += 1` in emitted code.
- Pre-allocate tables with `table.create(n)` when size is known (range-based loops, list comprehensions)
- Eliminate builtin wrapper closures for statically-resolved calls

### P2 -- Code Quality (smaller output, better readability)
- Narrow hoisted variable declarations to nearest enclosing block
- Merge phi variables with branch sources
- Use `table.concat` for 3+ string concatenations
- Emit `elseif` instead of nested `else if ... end end`
- Strip dead `molt_func_attrs` side-table when no dunder attrs are used

### P3 -- Platform Integration
- Split large transpiled output into ModuleScript chunks
- Emit `debug.dumpcodesize()` calls for native code budget monitoring
- Bridge Roblox `Instance` API for transpiled code that creates game objects
- Support `HttpService` as a capability-gated module bridge

---

## References

### Internal
- `runtime/molt-backend/src/luau.rs` -- backend implementation
- `runtime/molt-backend/src/luau_json_prelude.luau` -- JSON serializer
- `formal/lean/MoltTIR/Backend/LuauSyntax.lean` -- Luau AST
- `formal/lean/MoltTIR/Backend/LuauEmit.lean` -- translation functions
- `formal/lean/MoltTIR/Backend/LuauSemantics.lean` -- evaluation model
- `formal/lean/MoltTIR/Backend/LuauCorrect.lean` -- correctness proofs
- `formal/lean/MoltTIR/Backend/LuauEnvCorr.lean` -- environment correspondence
- `formal/quint/molt_luau_transpiler.qnt` -- Quint invariant model
- `tests/luau/test_molt_luau_correctness.py` -- differential tests (Lune vs CPython)
- `tools/check_luau_static.py` -- static analysis checker
- `tools/benchmark_luau_vs_cpython.py` -- benchmark harness
- `docs/architecture/luau-backend-known-bugs.md` -- bug tracker

### External
- [How we make Luau fast](https://luau.org/performance/) -- Luau performance internals
- [Native Code Generation](https://create.roblox.com/docs/luau/native-code-gen) -- Roblox native codegen docs
- [@native attribute RFC](https://rfcs.luau.org/syntax-attribute-functions-native.html) -- function-level native compilation
- [Luau sandbox](https://luau.org/sandbox/) -- embedding and security model
- [Luau bytecode format](https://deepwiki.com/luau-lang/luau/4.2-bytecode-format) -- bytecode internals
- [Roblox memory usage](https://create.roblox.com/docs/studio/optimization/memory-usage) -- memory monitoring
