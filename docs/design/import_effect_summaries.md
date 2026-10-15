# Import effect summaries

Status: design (2026-10-15), owner finding HF-226. Not implemented.

## Problem

The binding flow (`src/molt/compiler_analysis/python_binding_flow.py`) treats every `import` statement as
arbitrary code that may rewrite the importing module's globals: it applies `EXECUTES_ARBITRARY_PYTHON`
and invalidates every member, except for a fixed list of canonical modules (`_CANONICAL_IMPORT_IDENTITIES`:
`__future__`, `typing`, `sys`, `builtins`, `importlib`, `inspect`, `_intrinsics`). After any other import the
analysis forgets two static facts:

1. **`TYPE_CHECKING` is False.** Graph discovery then compiles type-only imports. A program with
   `from typing import TYPE_CHECKING`, `import os`, `if TYPE_CHECKING: import decimal` compiles `decimal` as an
   explicit import; with the block before `import os`, it does not (measured on main 42e2cf65f).
2. **The module's `__package__`, `__spec__` and `__name__`.** A later relative import then projects to
   `unknown_package`, and closures that select `unknown_relative_sources="local_inventory"` cover every local
   module. `process_guard.py` and `cargo_execution_policy.py` use relative imports on purpose, so the M35
   frontend lowering scope covers all 1,409 `src/molt` files and any edit invalidates the lowering cache.

The model is sound: imported code can write `sys.modules[name].__dict__`. It is also stricter than the same
closures' treatment of absolute imports, which already assumes no finder redirects them.

## Prior art (primary sources, 2026-10-15)

| Tool | `TYPE_CHECKING` | Imports and importer globals |
|---|---|---|
| Nuitka | `typing` is a hard module; `TYPE_CHECKING` is a trusted constant bound to the import (`nuitka/HardImportRegistry.py`, `nodes/ImportNodes.py`) | Import nodes do not escape module variables; only unknown calls and `import *` do (`Variables.py`) |
| Meta Lifeguard (PEP 810 lazy-import safety) | evaluated False (`src/traits.rs`) | Per-module effect summaries flag imported-module assignment, `sys.modules` access, `builtins.__import__` overrides and `exec`; unproven modules are unsafe |
| Cinder strict modules | `typing` stub sets it False | Top-level code is simulated; writing another module's values is rejected (`StrictModuleModifyImportedValueException`) |
| mypy, pyright, Ruff, grimp, Gazelle | matched by spelling | no binding model |
| PyInstaller modulegraph, cx_Freeze, pydeps, Pants | includes type-only imports (PyInstaller #8860 open) | no binding model |

Every surveyed tool resolves relative imports from the module identity found at discovery time; mypy
rejects a file reachable under two names.

## Measured precision

A syntactic scan for namespace writes and hooks (module attribute, `__dict__` and `setattr` stores,
`f_globals`, `exec` with globals, `__import__` stores, trace, audit and import hooks) finds them in 4 of 460
tooling modules and 15 of 949 molt stdlib modules, even counting reads and non-module receivers. Most stdlib
hits are reads (`typing`, `collections` read `sys._getframe(1).f_globals` to name the caller) or function
`__dict__` updates (`functools.update_wrapper`).

## The soundness decision

A fully sound summary must treat any write through a receiver that might be a module as a namespace
write. `functools.update_wrapper(wrapper, wrapped)` writes `wrapper.__dict__` for an arbitrary `wrapper`, so
a sound analysis marks `functools`, and with it nearly every program, unclean: no precision is gained.

Precision needs the assumption Lifeguard and Nuitka make: **code writes another module's namespace only
through syntax that names a module** (an attribute store or `setattr` on a name bound by `import`,
`vars()`/`__dict__` of such a name, `sys.modules[...]` objects, frame `f_globals`, `exec` or `eval` with
explicit globals, `builtins.__import__` or import-hook installation, `importlib.reload`). The cost of a
violation differs by consumer:

- **Graph discovery (user programs):** a type-only module left out of the graph fails loudly with
  `ModuleNotFoundError` if a program rebinds `TYPE_CHECKING` through reflection. Acceptable, and diagnosable.
- **Source-dependency fingerprints (molt's own tree):** a violation could reuse a stale lowering cache
  silently. Molt controls this tree, so the assumption must be enforced there: a lint over `src/molt` that
  rejects the listed constructs outside declared, reviewed sites.

## Design

1. **Per-module summary.** The binding analysis already computes effect masks per statement. Add a
   module-level `namespace_write` fact: whether any code in the module performs one of the listed
   constructs, with each site recorded (module, line, construct) for diagnostics.
2. **Optimistic fixpoint over the closed graph.** Discovery assumes every import is namespace-clean,
   discovers the graph, computes summaries, and verifies. If any module in the graph is unclean, the
   importers of its import closure are re-analyzed with that module's imports treated as arbitrary, and the
   loop repeats; it is monotone (the unclean set only grows). Report each blocker by module and construct.
3. **Import statement rule.** An import whose target (resolved from the importer's discovered identity, one
   identity per file) is clean keeps the importer's globals, metadata and member facts, exactly as
   `_CANONICAL_IMPORT_IDENTITIES` imports do today. Unknown native code (C extensions) stays unclean.
4. **`typing.TYPE_CHECKING`.** Bind it as a constant of the hard module `typing`, keyed to the binding as
   Nuitka does, never by spelling.
5. **Cache identity.** The analysis key includes the clean/unclean facts the file's imports consumed, so a
   newly unclean dependency invalidates exactly its importers.
6. **Tooling lint.** Enforce the assumption on `src/molt` and `tools/` (the source-dependency consumers).

## Tests

- Both `decimal` programs: the type-only import stays out of the graph regardless of statement order.
- A program whose imported module performs `importer.TYPE_CHECKING = True` through an imported-module
  assignment keeps the type-only import (the summary marks it unclean).
- The M35 lowering-scope closure on the real tree excludes `cli/__init__.py` and the backend layer.
- The installed admission contract's closure stays within identity and schema modules.
