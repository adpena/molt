# P0 #50 — `@dataclass` × class-body re-lower interaction (diagnosis)

## Symptom
On the #50 tree, a `@dataclass` whose body contains control flow (so the body
takes the block-execution path the re-lower added) loses its generated dunders:

```python
from dataclasses import dataclass
@dataclass
class Cfg:
    n: int = 3
    total: int = 0
    for _i in range(3):       # <- control flow forces block-exec
        total = total + _i
c = Cfg()
print(c.n, c.total)           # CPython: 3 3            molt: 3 3   (ok)
print(repr(c))                # CPython: Cfg(n=3, total=3)
                              # molt:    <__main__.Cfg object at 0x...>   WRONG
print(c == Cfg(3, 3))         # CPython: True
                              # molt:    TypeError: Cfg() takes no arguments  WRONG
```

A *straight-line* `@dataclass` (no control flow in the body) still works because
the 4th commit (`2b8f8ed33`) gates the class-ns scope push on `body_needs_block`.
But that commit only masked the symptom for the straight-line case — it did NOT
fix the underlying interaction, which surfaces whenever a dataclass body actually
needs block execution. This is the asymmetric/partial-fix trap: the field
defaults still compute correctly (`3 3`), but `__init__` / `__repr__` / `__eq__`
are never installed.

## Root cause (exact file:line)
`src/molt/frontend/visitors/classes.py`, `visit_ClassDef`:

- The re-lower (#50) now reads `ClassFacts.block_exec_class_nodes` for the class
  AST node and, when true, forces `dynamic_build = True` so the class body
  executes as a real block over a heap namespace dict.
- The `@dataclass` runtime application — the emission that calls
  `dataclasses.dataclass(cls, init=..., repr=..., eq=..., ...)` to install
  `__init__` / `__repr__` / `__eq__` / `__hash__` / frozen guards / `__match_args__`
  onto the finished class object — lives **entirely inside the `if not dynamic_build:`
  branch** (the static "outlined CLASS_DEF" path), at lines ~3365-3443.
- The `else` branch (`dynamic_build` path, lines ~3075-3207 + ~3445-3525) builds
  the class via the metaclass call (`type(name, bases, ns)`) and merges layout, but
  **never emits the `dataclasses.dataclass(cls, ...)` call**. So a dataclass routed
  through `dynamic_build` gets a bare class with no generated dunders.

Therefore: `body_needs_block` (control flow / `del` / non-Name assign target in the
class body) → `dynamic_build=True` → the dataclass-application emission is skipped →
`Cfg() takes no arguments` + default-object `repr`.

The `@dataclass` transform is construction-method-agnostic: `_molt_apply_dataclass`
operates on a *finished* `cls` object via `setattr` / `cls.x = ...`, reading
`cls.__annotations__` (gathered at compile time and published into the namespace).
It does not care whether the class object came from a static outlined `CLASS_DEF`
or from a dynamic metaclass call.

## Fix
Hoist the dataclass-application emission out of the `not dynamic_build` gate into a
single helper `_emit_dataclass_application(node, class_info, class_val) -> MoltValue`
and invoke it on the finished `class_val` in BOTH the static and dynamic branches,
after the class object exists and is published, and before any `other_decorators`
wrap it (matching CPython: `@dataclass` is the innermost decorator). This keeps the
#50 re-lower win intact (dataclass bodies with control flow now execute as blocks
AND get their dunders) with one code path that publishes the dataclass transform.

## Acceptance (== CPython 3.14)
- `dc_with_controlflow.py` → `3 3` / `Cfg(n=3, total=3)` / `True`.
- Full dataclass corpus (basic, methods, frozen, eq/repr, inheritance, field
  defaults) byte-identical.
- The existing class-body corpus (for/if/while/del/with/try/nested/decorator/
  global-fallback/exception/comprehension) unchanged.
- No regression on #45 / #51 / #52 / class_prepare_* / metaclass_*.
