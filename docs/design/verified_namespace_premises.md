# Verified namespace premises

Status: implemented for tooling source closures (2026-10-15); user-program discovery and the frontend
are next. Owner finding HF-226.

## Problem

By default the binding flow (`src/molt/compiler_analysis/python_binding_flow.py`) assumes that any callback
(a call, an import, an iteration or context-manager callback) may rebind every global and canonical
member of the module under analysis. It applies `EXECUTES_ARBITRARY_PYTHON`, expires every module slot
through the taint epoch and invalidates every member. Only a fixed list of canonical imports
(`_CANONICAL_IMPORT_IDENTITIES`) escapes. After the first callback the analysis forgets two static facts:

1. **`TYPE_CHECKING` is False.** Graph discovery then compiles type-only imports. A program with
   `from typing import TYPE_CHECKING`, `import os`, `if TYPE_CHECKING: import decimal` compiles `decimal`.
2. **The module's `__package__`, `__spec__` and `__name__`.** A later relative import projects to
   `unknown_package`, and closures with `unknown_relative_sources="local_inventory"` cover every local
   module. The M35 frontend lowering scope covered all 1,409 `src/molt` files, so any edit invalidated the
   persisted lowering cache.

This default is stricter than Molt's language contract and inconsistent with Molt's own consumers:

- The [dynamic-semantics policy](../spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
  (spec 0216) admits scoped mutation and excludes unrestricted `exec`/`eval`, arbitrary runtime
  monkeypatching and unrestricted reflection.
- The frontend already assumes the narrower premise: `_module_stable_funcs`
  (`frontend/lowering/sema_state.py`) treats a module function as stable when only its own module
  rebinds it.
- The import flow (`python_imports.py`) trusted foreign code at calls ("Ordinary foreign code has its own
  global mapping") but widened metadata at iteration and context callbacks.

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

## The premise

Under spec 0216 a module's globals change outside its own statements only through:

- **its own code**: deferred code that declares a name `global`, and own-namespace item writes with
  literal keys (`globals().pop("_require_intrinsic", None)`, 177 molt stdlib sites);
- **named writes from other modules**: an attribute store, `setattr`/`delattr`, `__setattr__`, or a
  `__dict__`/`vars()` item write on a module reference (an import binding, its attribute chain, or a
  `sys.modules` entry);
- **the import system**, which binds every imported submodule as an attribute of its package.

Anything else (a module object handed to a generic writer, frame `f_globals` writes, `exec`/`eval` on
foreign globals, `gc` walks) is unrestricted reflection or monkeypatching, outside the contract. A module
whose own code reflects its namespace dynamically (an escaping `globals()`, a computed key, `exec`/`eval`
on its own globals, a star import) keeps the whole-namespace model for itself.

Canonical members follow the same rule: a callback changes `typing.TYPE_CHECKING`, `sys.modules`,
`sys.meta_path` and the other `PythonMember` facts only if some code in the graph writes them by name,
or mutates an import-hook object (`sys.modules`, `sys.meta_path`, `sys.path_hooks`,
`sys.path_importer_cache`).

## Design

1. **One summary authority.** `compiler_analysis/python_namespace_writes.py` summarizes a module in one
   AST pass (0.77 ms per file on the 461 tooling sources): the own names its deferred code rebinds
   (`None` on dynamic own reflection) and its named writes to other modules' namespaces, as
   `(receiver spelling, name, construct, line)`. It is conservative by spelling.
2. **The binding flow takes a premise.** `PythonBindingFlowPolicy.foreign_namespace_writes` lists the
   names other graph code writes into this module (`None`, the default, means any name);
   `callback_member_writes` lists the canonical members code in the graph writes (default: all). With a
   premise, the taint domain holds only the module's own callback-writable names plus the foreign
   names, so a callback leaves every other global, the module metadata and unwritten members intact.
   `python_canonical_member_writes` maps a summarized write to members through the same
   `_member_write_mask` the flow uses for its own direct stores.
3. **The import flow asks the binding flow.** `PythonBindingFacts.callbacks_rewrite_metadata` says
   whether any callback can rebind `__name__`, `__package__`, `__spec__` or `__path__`. Without it,
   every callback summary in the import flow leaves the metadata states unchanged.
4. **One graph projection.** `compiler_analysis/python_namespace_graph.py` owns
   `PythonNamespacePremise`, `PythonGraphNamespaceWrites` and `graph_python_namespace_writes`.
   It aggregates the existing summaries through the caller's module resolver; source capture, package
   children and graph discovery stay with the caller. `local_python_import_closure` walks the graph
   assuming no named foreign write, summarizes every reached module (cached in its analysis row), and
   walks again until the premise it assumed is the one it found. Writes only add names, so the premise
   grows monotonically. A package's premise always includes its child module names from the
   filesystem, so it never depends on which children the graph reaches.
5. **Dead relative imports are lexical edges.** When the import flow proves a relative import never
   runs (a statically false `TYPE_CHECKING` branch), the full lexical graph resolves it from the
   module's identity, as it keeps a dead absolute import. Before this, such a statement failed the
   executable-tool closure with an unknown anchor (also on main).

## Results

Lowering scope on the real tree (isolated `MOLT_CACHE`, Apple M-series):

| | main 42e2cf65f | this design |
|---|---|---|
| files in scope | 1,409 (whole-tree coverage) | 242 (import graph only) |
| fixpoint rounds | n/a | 1 |
| cold | 4.09 s | 4.02-4.14 s |
| warm | 0.14 s | 0.04-0.05 s |

An edit outside the 242 files no longer invalidates the persisted lowering cache.

## Remaining consumers

- **User-program discovery** (`cli/module_import_scanner.py`): compute the premise over the discovered
  program graph, molt stdlib included, so `TYPE_CHECKING` blocks after imports and calls stay out of
  compiled programs. Two tests pin the old model today:
  `test_collect_imports_prunes_type_checking_alias_branches` (`re`) and
  `test_molt_os_module_init_retains_reentrant_type_checking_branch`.
- **Frontend codegen**: pass the build graph's premise to the per-module lowering analysis, so codegen
  facts and `_module_stable_funcs` share one premise.

## Tests

- `tests/test_python_namespace_writes.py`: every recognized construct, reads that are not writes, and
  object writes that are not namespace writes.
- `tests/test_python_binding_flow.py`: facts survive calls, imports and loops under a premise; an own
  `global` writer, own reflection, a foreign name and graph-written members each keep the conservative
  answer; the member mapping shares the direct-store authority.
- `tests/cli/test_cli_python_source_closure.py`: type-only imports stay out; a named foreign write and a
  package child named like the guarded flag each bring the import back; cached rounds re-analyze
  nothing; a dead relative import is a lexical edge.
- `tests/cli/test_cli_lowering_cache_tooling_scope.py`: the real lowering scope needs no whole-tree
  coverage and excludes the backend and command layers.
