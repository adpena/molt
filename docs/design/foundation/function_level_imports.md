# Function-level imports in the closed-world build

Status: implemented for HF-557 (2026-10-29). Owners: the frontend import
lowering (`src/molt/frontend/lowering/import_lowering.py`,
`src/molt/frontend/visitors/statement_scope.py`), the named-call dispatch
(`src/molt/frontend/visitors/call_dispatch_named.py`) and the source-demand
authority (`src/molt/compiler_analysis/python_source_demands.py`).

## The rule

CPython executes an `import` or `from ... import` statement in a function body
when the body runs, not when the module loads. A body that never runs never
imports. Molt keeps that rule in three parts.

1. **The statement is the import.** An import in a function body lowers to the
   runtime import transaction (`molt_importlib_import_transaction`) at the point
   where it stands. When it runs and its module is not in the binary, the
   transaction raises `ImportError` (`ModuleNotFoundError` when no source
   exists), and the caller can catch it, as in CPython.
2. **The bound name is an ordinary binding.** A call through an imported name
   is an ordinary call on the bound value. The binding's import origin only
   selects a specialization, such as a guarded direct call into a module that
   the graph admitted. A name with no admitted origin gets a dynamic call. The
   frontend never refuses a module because a function in it imports from a
   module outside the graph. Before HF-557 the named-call dispatch refused such
   a call in generator and coroutine bodies (MOLT-FE007, "call to
   non-allowlisted function"), which stopped `test.support` from compiling.
3. **The graph admits the imports of bodies that can run.** Graph discovery
   scans an imported module at initialization depth. It scans a function body,
   and admits that body's imports, when the source-demand authority proves the
   body can run: a call, an escape (the function value leaves analyzable code,
   for example through `getattr` with a computed name) or an exposure. This is
   the HF-126 successor mechanism; this change does not alter it.

A relative import in a function body keeps its runtime package anchor: CPython
resolves it against the function's globals (`__package__`, `__spec__`) when it
runs. The binding's origin is the module that every possible anchor names,
computed by the same resolver as a module-level relative import over the
module's import-metadata flow. When the anchors disagree or need the runtime,
the binding has no origin. It is never the unanchored spelling: before HF-557,
`from .helper import compute` in package `pkg` recorded the origin `helper`, a
different top-level module. That wrong origin caused the MOLT-FE007 refusal and
aimed guarded direct calls at `helper.compute` instead of `pkg.helper.compute`.

No verified-subset exclusion applies to function-level imports
(`docs/spec/areas/compat/contracts/verified_subset_contract.md`), so no
compile-time diagnostic remains for them. `from ... import *` in a function body
stays a compile-time error because Python rejects it.

## Prior art

- **CPython.** `IMPORT_NAME` and `IMPORT_FROM` run when the frame executes them;
  the language reference defines `import` as a statement that finds, loads and
  binds at execution time ("The import statement", Python Language Reference
  7.11). A missing module raises `ModuleNotFoundError` at that statement.
- **PyInstaller.** Its module graph collects every import statement in a
  module, including those in function bodies, and classifies them as
  top-level, conditional, delayed (in a function) or optional (in `try`).
  Modules it cannot find are warnings, not errors; the frozen program raises
  `ImportError` only if it executes the import (`WARNFILE_HEADER` in
  `PyInstaller/building/build_main.py`, v6.11.1; "When things go wrong" in the
  PyInstaller manual).
- **Nuitka.** Every import statement, at any nesting depth, becomes an import
  node that locates its module at compile time
  (`ExpressionBuiltinImport._attemptFollow` in `nuitka/nodes/ImportNodes.py`).
  A found module is followed and compiled in. A `not-found` module stays an
  ordinary runtime import that raises `ImportError` when executed.
- **Codon.** Codon resolves modules at compile time and reaches modules it does
  not implement only through explicit `from python import` (CPython interop;
  "Differences with Python", docs.exaloop.io). It does not keep CPython's
  deferred `ImportError`.

Molt agrees with CPython, PyInstaller and Nuitka on the run-time rule: an
unexecuted import never fails. It deviates from PyInstaller and Nuitka on
admission. They follow every import they find; Molt follows a function-level
import only when its body can run, because each admitted module costs a full
frontend and backend compilation. HF-126 measured eager admission of
`unittest.__getattr__`'s import alone at 56 to 95 modules in every
`import unittest` program.

## Boundaries

- A module that a reachable body imports but that is absent from the binary
  raises `ImportError: module ... has no compiler-emitted body in this binary`.
  This is the closed-world boundary. Graph admission must make it reachable
  only for code that CPython would also fail to import.
- Explicit external roots (`MOLT_MODULE_ROOTS`, `--lib-path`, a respected
  `PYTHONPATH`) admit only the entry's direct imports unless
  `MOLT_EXTERNAL_STATIC_PACKAGES` names the package
  (`docs/spec/areas/compat/contracts/import_system_contract.md`). A transitive
  import inside such a package is not admitted, at module level or in a
  function body. This policy is separate from HF-557.

## Evidence

- `tests/differential/basic/function_import_absent_module.py`,
  `function_import_sibling_module.py` and `function_import_package_init.py`
  (with `function_import_sibling_pkg/` and `function_import_lazy_pkg/`) fail
  to build at base 0d4b89cb5a with MOLT-FE007 and match CPython 3.12 after the
  change.
- `tests/test_frontend_class_imports.py` pins the dynamic call for generator,
  coroutine, function and guarded-global bindings and the relative-import
  origin (`pkg.helper`, or none without an anchor).
- Graph admission does not change. Module graphs on macOS arm64, before and
  after, with the same module sets: a hello program 7 and 7 modules,
  `import unittest` 117 and 117, `import asyncio` 96 and 96. Six
  function-level relative imports exist in Molt's stdlib (`importlib.machinery`,
  `unittest`, `unittest.case`, `test.support.warnings_helper`); each now costs
  one more resolution over states that the module flow already holds.
- CPython's `test.support` now compiles. In `regrtest.import-system.native`,
  `test_pkgutil` and `test_zipimport` run and stop at `sysconfig`, a generated
  stub (HF-273). `test_importlib` reaches `test.support` from its package
  `__init__`, a transitive import from an external root, so the regrtest shim
  now admits the `test` package with `MOLT_EXTERNAL_STATIC_PACKAGES=test`; it
  then reaches the same `sysconfig` stub.
