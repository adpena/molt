<!-- Ecosystem-compatibility gap audit (background RECON/AUDIT agent, 2026-06-05). Read-only; codebase = sole source of truth. -->
<!-- Mission: the planning-grade audit for the five-year plan's ECOSYSTEM COMPATIBILITY lane. Sibling/successor to docs 16 (stdlib/GPU) and 17 (first ecosystem recon). -->
<!-- Policy amendment A (binding): ecosystem scope = FULL Python library ecosystem, dynamism-bounded. The conformance manifest must classify DYNAMISM FEATURES so library compat is DERIVABLE. -->

# Ecosystem Compatibility Gap Audit (Dynamism-Bounded, Planning-Grade)

> **Honesty header — read first.** The mission brief states this is "the only
> user-named lane with NO committed audit." That is **not accurate**: a prior
> recon, `docs/design/foundation/17_ecosystem-third-party-compat-gap-audit.md`,
> already exists and covers package resolution, the 34-package import suite, the
> C-extension policy, and a blockers inventory. **This document (24) supersedes
> 17** by adding the three planning artifacts 17 lacks — the **dynamism-feature
> classification scheme** (so library compatibility is *derivable*, per policy
> amendment A), a **25-package paper triage with file:line-grounded verdicts**,
> and a **fail-closed ratchet design** modeled on the existing satellite-parity
> guard. It also **corrects three claims in 17 that the codebase contradicts**
> (flagged inline as ⚠️CORRECTS-17). Doc 17 should be marked "superseded by 24"
> in a follow-up; it is not deleted here (read-only audit) but its top-10 arc
> table is folded into and re-grounded by this document.
>
> **The single biggest surprise (both directions):** doc 17 claims "zero
> ecosystem proof" for native extensions and the 0215 spec states "no CPython
> ABI compatibility," yet the tree contains a **~6,250-line working CPython
> binary-ABI bridge** (`runtime/molt-cpython-abi/`) that dlopen-loads *real,
> unmodified* CPython C extensions, implements ~150 stable-ABI functions
> including `PyArg_ParseTuple`/`METH_VARARGS|O|KEYWORDS|FASTCALL`, is wired into
> the runtime import path behind a `cext_loader` feature, and has a passing
> integration test that compiles a `.c`, dlopens it, and calls `PyInit_*`. The
> Y1.5 ABI ladder is **already half-built and undocumented in the policy layer.**

---

## EXECUTIVE SUMMARY

The ecosystem-compatibility lane is **substantially thicker than doc 17 and the
project-status docs imply, and thinner than a five-year plan needs in exactly
two structural places**: (1) there is **no measurement substrate** — no derivable
manifest, no ratchet, no functional-coverage gate — so the real progress is
*invisible and unprotected against regression*; and (2) the two C-extension
tracks (libmolt-recompile vs. CPython-ABI-dlopen) **contradict each other in the
policy docs** and neither is exercised against a real PyPI package with committed
results.

What is genuinely built and load-bearing:

- **Package resolution / static import graph** (`src/molt/cli.py`): venv +
  `.molt-venv` + vendor + PEP-420 namespace + wheel-tag/checksum validation.
  Mature.
- **Class-level dynamism runtime** (`runtime/molt-runtime/src/`): metaclass
  `__prepare__` + MRO winner resolution + conflict detection, `__init_subclass__`
  dispatch, `__set_name__`, descriptors, `__slots__`, class/instance
  `__getattr__`. This is the hard part of supporting attrs/pydantic/SQLAlchemy
  class machinery and **most of it is done** (⚠️CORRECTS-17, which calls
  metaclass support "likely unsupported, not documented").
- **dataclasses without `exec`** (`src/molt/stdlib/dataclasses.py`): CPython
  builds `__init__` by `exec`-ing a generated source string; molt synthesizes the
  same dunders structurally and rebuilds the class via `types.new_class` +
  an `_exec_body` namespace callback — a **typed-shim around CPython's controlled
  codegen**. This is the template for the whole "compatible-via-typed-shim" tier.
- **typing runtime**: `get_type_hints` is implemented
  (`src/molt/stdlib/typing.py:1260`), `@runtime_checkable` exists (`:873`),
  `inspect.signature` is intrinsic-backed
  (`src/molt/stdlib/inspect.py:48,97`). (⚠️CORRECTS-17, which lists
  `get_type_hints` as "pending.")
- **importlib.metadata entry-points**: intrinsic-backed
  (`src/molt/stdlib/importlib/metadata/__init__.py:43`), so plugin discovery has
  a runtime path.
- **The CPython-ABI dlopen bridge** (`runtime/molt-cpython-abi/`): see surprise
  above.

What is missing for five-year planning (the real gaps):

1. **No conformance manifest / dynamism taxonomy** → library compatibility is
   not derivable; every verdict today is tribal knowledge.
2. **No ecosystem ratchet** → the satellite-parity guard pattern
   (`tools/check_satellite_parity.py`) exists and is perfect for this, but is not
   applied to packages.
3. **Module-level `__getattr__` (PEP 562) appears unsupported** — a single
   missing feature that blocks the lazy-top-level-import idiom used by
   numpy/scipy/tensorflow/`rich`-style packages. High unlock leverage.
4. **Both extension tracks lack a committed real-package proof** (numpy/msgpack/
   cryptography class), and the **policy docs contradict the code** (0215 says
   "no CPython ABI compat"; the bridge does exactly that).
5. **Generator/`yield`** remains the largest shared blocker (consistent with docs
   16/17 and the generator-fusion keystone).

---

## LANE A: CURRENT-STATE MAP (with file:line evidence)

### A.1 Package resolution and static import graph

| Capability | Location | State | Notes |
|---|---|---|---|
| Module-root discovery order | `src/molt/cli.py:4627` `_resolve_module_roots` | PRESENT | MOLT_MODULE_ROOTS → project/cwd/`src` → vendor → PYTHONPATH (opt-in) → `--lib-path` → active venv site-packages → `.molt-venv` |
| venv site-packages discovery | `src/molt/cli.py:1299` `_molt_venv_site_packages` | PRESENT | Unix `lib/python*/site-packages` + Windows `Lib/site-packages`; `MOLT_HERMETIC_MODULE_ROOTS=1` disables auto-detect |
| Path-parts resolution (pkg vs module) | `src/molt/cli.py:3758` `_resolve_module_path_parts` | PRESENT | `__init__.py` package detection; dotted-chain resolution |
| Resolution cache | `src/molt/cli.py:3778` `_ModuleResolutionCache` | PRESENT | roots/resolve/namespace/source/AST caches; rebuild-safe |
| Module graph discovery | `src/molt/cli.py:861` `_discover_module_graph` | PRESENT | AST-parse → resolve → expand dotted chains |
| Namespace packages (PEP 420) | `src/molt/cli.py:3858` `has_namespace_dir`, `:4939` stub emit | PRESENT | auto-generated `namespace_*.py` stubs at build time |
| Runtime-import-support detection | `src/molt/cli.py:5487` `_module_graph_needs_runtime_import_support` | PRESENT | decides static vs. runtime import boundary per module |
| Wheel resolution + tag/checksum | `src/molt/cli.py:2730`–`2776` | PRESENT | `_wheel_filename_tags`, `molt_abi*` matching, `wheel_sha256` |
| Vendor dirs | `src/molt/cli.py:1280` `_vendor_roots` | PRESENT | `{root}/vendor/packages`, `{root}/vendor/local` |
| sdist build/extraction | — | **ABSENT** | no sdist handling found; pre-built wheels or source trees only |
| `.pth` / editable installs | — | **ABSENT** | `.pth` files not processed (site.py has the parser at `:169` but the build-time graph does not run it); editable installs must resolve to real paths |

**Verdict:** resolution is mature; the gaps (sdist, `.pth`) are decisions to make,
not bugs.

### A.2 Class-level dynamism runtime (the hard part — mostly done)

| Feature | Evidence | State |
|---|---|---|
| Metaclass `__prepare__` + winner resolution + conflict error | `runtime/molt-runtime/src/builtins/types.rs:4757`–`4901` (reads `metaclass` kw, promotes to most-derived base metaclass, raises "metaclass conflict…", calls `__prepare__`) | PRESENT |
| `__init_subclass__` dispatch | `runtime/molt-runtime/src/object/ops.rs:9886` `dispatch_init_subclass_hooks`, gated into class-def (`:9870`,`:9899`) | PRESENT |
| `__set_name__` on descriptors at class creation | `runtime/molt-runtime/src/object/ops.rs:9780` `class_apply_set_name`; `attributes.rs:3148` `class_set_name_bits` | PRESENT |
| Descriptors (`__get__`/`__set__`/`__delete__`) | `runtime/molt-runtime/src/builtins/attributes.rs:4273,4377,4504` (`__set__` lookup), property/classmethod/staticmethod intrinsic boot | PRESENT |
| `__slots__` | `runtime/molt-runtime/src/builtins/types.rs` (slot layout) | PRESENT |
| Class/instance `__getattr__` / `__getattribute__` | `runtime/molt-runtime/src/builtins/attributes.rs:429,499,953,2402,2559,2699,2950`; `attr.rs:718` | PRESENT |
| Module-level `__getattr__` (PEP 562) | searched `ops.rs`, `platform.rs`, frontend; **no module-level dispatch found** (all `__getattr__` hits are class/instance) | **ABSENT** ⚠️ |
| `abc.register` virtual subclass | `src/molt/stdlib/abc.py:147` (`_abc_registry`); ABCMeta intrinsic boot in `runtime/.../builtins/abc.rs` | PRESENT (registry path) |

This row table is the single most important "significant progress" finding: the
**class-construction dynamism surface that frameworks lean on is largely
implemented in the runtime**, which is why attrs/click/jinja2/werkzeug import and
(partly) run today.

### A.3 dataclasses: controlled codegen without `exec` (the shim template)

CPython's `dataclasses` literally `exec()`s a generated `__init__` source string.
molt cannot (`exec` is excluded). Instead `src/molt/stdlib/dataclasses.py`:

- Walks `cls.__annotations__` and synthesizes `__init__`/`__repr__`/`__eq__`/
  `__hash__`/order dunders **structurally**, installing them via `setattr`
  (module docstring `:1`–`:11`; `_molt_apply_dataclass` `:541`).
- For `slots=True` and for `make_dataclass`, rebuilds the class via
  `types.new_class(name, bases, {}, _exec_body)` where `_exec_body(ns)` populates
  the namespace from a snapshot dict (`:510`–`:515`, `:883`). This routes through
  the **real metaclass** and the runtime's class-creation path (A.2) rather than
  a string `exec`.
- Backed by intrinsics for the heavy operations:
  `molt_dataclasses_make_dataclass`, `_fields`, `_asdict`, `_replace`, `_eq`,
  `_hash_fn`, `_field_flags`, `_post_init` (`:22`–`:43`).

**This is the canonical "compatible-via-typed-shim" pattern**: a library feature
that *requires* a dynamism feature molt forbids (here, `exec`-codegen) is made
compatible by re-expressing the *intent* through a supported primitive
(structural synthesis + `types.new_class`). The taxonomy below is built so this
pattern is named and reusable, not rediscovered per library.

### A.4 typing / introspection runtime

| Feature | Evidence | State |
|---|---|---|
| `get_type_hints` | `src/molt/stdlib/typing.py:1260` | PRESENT (resolves via `__globals__`/module dict; forward-ref string eval is the open edge — see probe list) |
| `@runtime_checkable` | `src/molt/stdlib/typing.py:873`, `:879` | PRESENT (guarded to protocol classes) |
| `Protocol` / `_ProtocolMeta` | `src/molt/stdlib/typing.py` (Protocol machinery present) | PARTIAL (status doc + 17 note structural-subtyping checks incomplete) |
| `inspect.signature` | `src/molt/stdlib/inspect.py:48` `_signature_from_intrinsic`, `:97` `signature`, `:85` bound-method bind | PRESENT (AOT-derived from compiled function metadata) |
| `__annotations__` access | runtime attr path | PRESENT |
| `inspect` (full module) | `src/molt/stdlib/inspect.py` (332 lines — thin) | PARTIAL (signature/Parameter/Signature; source-introspection like `getsource` not viable in AOT) |

### A.5 Dynamic-execution policy boundary (the hard "no")

| Construct | Evidence | Behavior |
|---|---|---|
| `eval` / `exec` | `src/molt/stdlib/builtins.py:300`–`315` `_dynamic_execution_unavailable` | raise `RuntimeError("MOLT_COMPAT_ERROR: …outside the verified subset")` |
| `compile` | `src/molt/stdlib/builtins.py` `molt_compile_builtin` | validation-only (parser-backed code objects), not executable |
| `sys.breakpointhook` | `src/molt/stdlib/sys.py:441`, `_sys_impl.py:309` | `MOLT_COMPAT_ERROR` |
| `doctest` | `src/molt/stdlib/doctest.py:15` | `MOLT_COMPAT_ERROR` (requires eval/exec/compile) |
| Compat error machinery | `src/molt/compat.py:15`–`128` `CompatibilityIssue`/`CompatibilityError`/`CompatibilityReporter` | structured feature/tier/impact/location/replace; `MOLT_COMPAT_WARNINGS=0` suppresses warnings |

The compat machinery is **the right substrate for the taxonomy**: it already
carries `feature`, `tier` (`native|guarded|bridge|unsupported`), `impact`, and a
human `replace:` hint. The taxonomy below should be expressed *through* this
struct, not parallel to it.

### A.6 C-extension: TWO tracks, contradictory docs

**Track 1 — libmolt recompile (declared "primary"):**
`docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md`. `molt extension
build`/`audit` implemented; emits `.whl` tagged `py3-molt_abi<major>-<platform>`
+ `extension_manifest.json`; capability + ABI-tag + checksum gating at build and
**load** time; validation in `src/molt/cli.py:2663`–`2820`
`_validate_extension_manifest`. Spec §1: *"Extensions must be recompiled; no
CPython ABI compatibility."* Status: Partial. **No committed real-package
result.**

**Track 2 — CPython binary-ABI dlopen bridge (declared "explicit bridge lane,
not primary"):** `runtime/molt-cpython-abi/` (crate `molt-lang-cpython-abi`,
~6,258 lines src). This **directly contradicts** the 0215 "no CPython ABI
compatibility" line and doc 17's "zero ecosystem proof."

| Component | Evidence | State |
|---|---|---|
| ABI types (`PyObject`, `PyTypeObject`, `PyMethodDef`, METH flags) | `src/abi_types.rs:144`–`150` (`METH_VARARGS=0x1`, `METH_KEYWORDS=0x2`, `METH_O=0x8`, `METH_FASTCALL=0x80`); `repr(C)` layout for 3.12 stable ABI | PRESENT |
| ~150 stable-ABI functions | `src/api/{refcount,numbers,sequences,mapping,strings,modules,errors,typeobj,object,abstract_number,abstract_sequence,abstract_mapping,buffer,capsule}.rs` (`api/mod.rs` index) | PRESENT (>95% of real extension surface per crate doc) |
| `PyArg_ParseTuple` / `…AndKeywords` | C variadic shim `shims/pyarg_variadic.c:84,92`; force-loaded into cdylib (`build.rs:33`–`60`); Rust side `src/api/errors.rs:214`–`237` | PRESENT |
| `PyObject_Call` / `tp_call` dispatch | `src/api/object.rs:462`–`490`; callability via `tp_call` `src/api/typeobj.rs:107`–`115` | PRESENT |
| Object bridge `*PyObject ↔ MoltHandle` (SIMD tag lookup) | `src/bridge.rs` (SSE4.1/NEON, `lib.rs:29`–`37`) | PRESENT |
| dlopen extension loader | `src/loader.rs:88` `load_cpython_extension` (libloading; `PyInit_<name>`; `MOLT_EXTENSION_PATH` allowlist; never probes host Python) | PRESENT (feature `extension-loader`) |
| Runtime wiring | `runtime/molt-runtime/src/builtins/platform.rs:3652` `cext_loader_dlopen` (gated `feature="cext_loader"`, non-wasm) → init bridge, register hooks, dlopen, copy module `__dict__` into namespace; second site `:7013` | PRESENT (feature-gated OFF by default) |
| Runtime hooks vtable (avoids circular dep) | `runtime/molt-runtime/src/cpython_abi_hooks.rs` (622 lines) `register_cpython_hooks` | PRESENT |
| Test coverage | `runtime/molt-cpython-abi/tests/` — 12 files (~85KB): `cext_integration.rs` compiles `_testmolt.c` (`add` via `PyArg_ParseTuple "ii"`, `reverse` via `"s#"`, `sum_list` via `"O"`), dlopens, calls `PyInit__testmolt`; plus `test_{bridge,exceptions,mapping,modules,numbers,object_protocol,refcount,sequences,strings,type_operations}.rs` | PRESENT (unit/integration) |

**Track 2 is the Y1.5 ABI lane, ~70% built at the primitive level, gated off,
and unproven against a real published wheel (numpy/cryptography).** It is the
biggest single piece of undocumented "significant progress" in the lane.

**ctypes:** `src/molt/stdlib/ctypes/` is a real port (`__init__.py` 177 lines +
`_endian`, `_layout`, `util`, `wintypes`, `macholib/`), **0** NotImplementedError
sites in `__init__.py`. Depth of actual FFI/dlopen behavior is a compile-probe
item (below). wasm dynamic linking unsupported.

### A.7 The existing measurement substrate (and its honest limits)

- `tests/runtime_compat/test_runtime_compat.py` (454 lines) + `scripts/` (34
  scripts): CPython → molt build → molt run, stdout-compared (tolerant of
  version/whitespace). Memory-guarded; timeouts 30/120/30s.
- ⚠️**CORRECTS-17:** doc 17 says these are "import + basic `hasattr` only — no
  functional testing." **That is wrong for several scripts.** `test_tenacity.py`
  exercises a real `@tenacity.retry(...)` decorator with a 3-attempt retry loop
  and an exception path; `test_attrs.py` constructs an `@attr.s(auto_attribs)`
  class and reads instance fields. The suite is **mixed** import-smoke +
  light-functional, *not* uniformly import-only. The honest limitation is
  different: it is **not derivable, not ratcheted, not in CI, and has no
  per-package verdict classification** — failures are neither aggregated nor
  attributed to a dynamism feature.

**Net current-state verdict.** The lane has strong *primitives* (resolution,
class dynamism, dataclass shim, two extension tracks, a compat-error vocabulary,
a working ABI bridge) and **no planning-grade connective tissue** (no taxonomy,
no derivable manifest, no ratchet, no real-package proof). The five-year risk is
not "we can't support libraries" — it is "we cannot *say which* libraries we
support, *why*, or detect when we break one."

---

## LANE B: THE DYNAMISM-FEATURE CLASSIFICATION SCHEME

Per binding policy amendment A, library compatibility must be **derivable** from a
classification of the *dynamism features a library requires* against what molt
supports. The three verdict classes are:

- **compatible** — the feature is supported natively (evidence in tree).
- **compatible-via-typed-shim** — the feature is forbidden in raw form, but molt
  re-expresses its *intent* through a supported primitive (e.g., dataclasses'
  `exec`-codegen → structural synthesis). The shim is a first-class, tested
  artifact.
- **incompatible-by-design** — the feature is an explicit verified-subset
  exclusion (exec/eval/compile, runtime monkeypatching, unrestricted
  reflection). The verdict **must name the exact excluded feature.**

A library's overall verdict is the **min** over the features it *actually
requires at the import + supported-call-path it exercises*. (A library that
*can* use `eval` but doesn't on the used path is still compatible — the scope is
"required dynamism," not "any dynamism present in the source.")

### B.1 Dynamism-feature table (the taxonomy)

| # | Dynamism feature | Verdict class | Evidence / shim / excluded-feature | Used by (examples) |
|---|---|---|---|---|
| D1 | Decorators (static + parametrized + stateful) | **compatible** | runtime call/closure path; `test_tenacity.py` proves a real retry decorator runs | nearly all |
| D2 | Descriptors (`__get__/__set__/__delete__`, property) | **compatible** | `attributes.rs:4273,4377,4504`; intrinsic property/classmethod/staticmethod | SQLAlchemy, attrs, pydantic |
| D3 | `__slots__` | **compatible** | `types.rs` slot layout; dataclass `slots=True` rebuild | attrs, pydantic, perf-libs |
| D4 | Metaclasses (`__new__`/`__init__`/`__prepare__`, conflict resolution) | **compatible** | `types.rs:4757`–`4901` | Django models, SQLAlchemy decl base, abc |
| D5 | `__init_subclass__` | **compatible** | `ops.rs:9886` dispatch | pydantic v2, plugin registries |
| D6 | `__set_name__` | **compatible** | `ops.rs:9780`; `attributes.rs:3148` | descriptors, ORMs |
| D7 | ABCs + `abc.register` virtual subclasses | **compatible** (registry) | `abc.py:147`; `builtins/abc.rs` | collections.abc consumers |
| D8 | `__subclasshook__` structural checks | PARTIAL → **shim-able** | abc.rs hook path; full structural subtyping incomplete | typing Protocols, abc |
| D9 | `@runtime_checkable` Protocol isinstance | PARTIAL → **shim-able** | `typing.py:873`; `_ProtocolMeta` instancecheck partial | pydantic, beartype |
| D10 | Class introspection: `__annotations__`, `get_type_hints` (no forward-ref strings) | **compatible** | `typing.py:1260`; runtime attrs | attrs, pydantic, cattrs, dataclasses-json |
| D11 | Forward-ref *string* annotation eval (`get_type_hints` on `"Foo"`) | **incompatible-by-design** (subset: `eval`) — *unless* a typed resolver shim lands | excluded feature = `eval` of annotation strings; mitigated by `from __future__ import annotations` + resolver | pydantic v2 (heavy), SQLAlchemy 2.0 mapped[] |
| D12 | `inspect.signature` / Parameter binding | **compatible** | `inspect.py:48,85,97` (AOT-derived) | click, FastAPI-style DI, pytest |
| D13 | `inspect.getsource` / frame/source reflection | **incompatible-by-design** (unrestricted reflection) | excluded feature = source/frame reflection; no source object in AOT binary | IPython, some test libs |
| D14 | Controlled string-`exec` codegen (dataclasses/namedtuple/attrs-`__init__`) | **compatible-via-typed-shim** | shim = structural synthesis + `types.new_class` (`dataclasses.py:510,883`) | dataclasses, NamedTuple, attrs (slotted) |
| D15 | Arbitrary user `exec`/`eval`/`compile` | **incompatible-by-design** | excluded feature = `exec`/`eval`/`compile` (`builtins.py:300`) | jinja2 *compiled-template* path, simpleeval, marshmallow custom |
| D16 | Module-level `__getattr__` (PEP 562) — lazy top-level imports | **incompatible-by-design (TODAY)** → **shim-able (PRIORITY)** | **no module-level dispatch in runtime** (gap); intent = lazy submodule binding, expressible via static graph + a module-getattr hook | numpy, scipy, tensorflow, rich, sqlalchemy top-level |
| D17 | Import-time monkeypatching (`module.__dict__[x]=…` at import) | PARTIAL | static graph cannot reflect *runtime* mutation; import-time mutation that is *deterministic* may be capturable | six, future, eventlet (import-time patch) |
| D18 | Runtime monkeypatching / `mock.patch` / fixture injection | **incompatible-by-design** | excluded feature = runtime monkeypatching | pytest fixtures, unittest.mock-heavy, gevent monkey |
| D19 | Generators / `yield` / `yield from` / async generators | **incompatible-by-design (TODAY)** → keystone arc | excluded *today*; generator-fusion keystone (`docs/design/generator_fusion.md`) is the planned lift, not a permanent exclusion | trio, tenacity (some), SQLAlchemy, starlette, tornado |
| D20 | async/await core | **compatible** (runtime-light) / PARTIAL (runtime-heavy, wasm) | event loop/tasks/futures OK; 4/5 runtime-heavy wasm fail (doc 16) | httpx, anyio, asyncio apps |
| D21 | `threading` primitives | PARTIAL | core present; full primitive parity pending; wasm no threads | concurrent libs |
| D22 | `ctypes` / `cffi` FFI | PARTIAL | `ctypes/` real port; depth = probe item; wasm no dynamic link | cryptography(cffi), bcrypt, PIL-via-cffi |
| D23 | CPython C-API extension (binary `.so`) | **compatible-via-bridge** (gated, unproven) | Track 2 bridge `molt-cpython-abi/`; OR Track 1 libmolt recompile (`0215`) | numpy, scipy, pandas, lxml, pillow, orjson, pydantic-core |
| D24 | Entry-points / plugin discovery (`importlib.metadata.entry_points`) | **compatible** | intrinsic-backed (`importlib/metadata/__init__.py:43`) | pytest plugins, click plugins, flake8 |
| D25 | Exception `__traceback__` chain object model | PARTIAL | intrinsic format_exception/extract_tb (doc 16); full traceback object pending | logging, pytest, sentry-style |
| D26 | Regex advanced (lookahead, named groups, backrefs, flag scoping) | **incompatible-by-design (TODAY)** → engine arc | native engine raises NotImplementedError; host fallback disabled (doc 16) | pyyaml, pathspec, parsers |
| D27 | `sys.modules` / `__path__` rewriting at runtime | **incompatible-by-design** | excluded feature = runtime import-graph mutation | namespace plugins doing runtime path tricks |

**This table is the deliverable that makes library compatibility derivable.** Each
PyPI package maps to the set of features it requires; its verdict is the min. The
table also makes the *unlock structure* explicit: features marked
"→ shim-able / → arc" are where engineering converts incompatible-by-design
(today) into compatible (later).

---

## LANE C: TOP-25 PYPI PAPER TRIAGE (no compiling — known-requirements based)

Verdict = the min over required dynamism features **on the import + commonly-used
path**. "Hardest feature" = the one driving the verdict. "Shared?" = whether the
blocker, once fixed, unblocks many (cross-ref Lane D).

| Pkg | Verdict (today) | Hardest required feature | Blocker shared? | Notes / file-grounded basis |
|---|---|---|---|---|
| six | **compatible** | D17 import-time shims | high (D17) | pure-Python compat shims; import-tested (`scripts/test_six.py`) |
| typing-extensions | **compatible** | D10 typing runtime | high | re-exports typing machinery molt has |
| certifi | **compatible** | none (data + path) | — | import-green (`test_certifi.py`) |
| idna | **compatible** | D2/D10 | — | pure-Python; unicode tables |
| charset-normalizer | **compatible** | D2 | — | pure-Python detection |
| packaging | **compatible** | D2/D10 | — | import-green (`test_packaging.py`); version/marker parsing |
| python-dateutil | **compatible** | D2 | — | pure-Python; relativedelta |
| attrs | **compatible-via-shim** | D14 (slotted `__init__` codegen) | high (D14) | **functionally tested** (`test_attrs.py` builds + reads); same shim family as dataclasses |
| click | **compatible** | D12 signature + D1 decorators | high (D12) | import-green; decorator-driven CLI; `inspect.signature` present |
| pyyaml | PARTIAL | D26 regex (resolver) + D22 (C `_yaml` optional) | high (D26) | pure-Python `SafeLoader` likely OK; C-accel via Track-2/1; regex resolver patterns may hit D26 |
| markupsafe | **compatible-via-bridge** (C) or PARTIAL | D23 (C speedups) | high (D23) | pure-Python fallback exists in the package; C path needs extension track |
| jinja2 | PARTIAL→**incompatible** on compiled path | D15 (template compile = `compile`/`exec`) | high (D15) | ⚠️ jinja2 *compiles* templates to Python via `compile`; the compiled path hits D15. Sandboxed/precompiled-template strategy needed |
| requests | PARTIAL | D20 (urllib3) + D22 (cert/ssl) | high | import-green (`test_requests.py` checks get/post/Session); functional HTTP = ssl/socket depth |
| urllib3 | PARTIAL | D20 + ssl/socket flags (doc 16) | high | retries/pool work; ssl MSG_* gaps (doc 16) |
| httpx | PARTIAL | D20 async + D19 (streaming) | high (D19/D20) | sync path closer; async-heavy + generators |
| anyio | PARTIAL | D20 + D19 | high (D19) | async abstraction; generator-based scopes |
| pydantic v2 | **incompatible-by-design (today)** | D23 (pydantic-core Rust ext) + D11 (forward-refs) | very high (D23) | core is a compiled Rust extension → needs Track-1 recompile or Track-2 bridge; pure-Python validators also hit D11 |
| flask | PARTIAL | D24 + D2 + D20 | high | import-green (`test_flask.py`); werkzeug routing + signature; functional serve = socket/async |
| werkzeug | PARTIAL | D2 + D12 | medium | import-green; descriptor/datastructure heavy |
| sqlalchemy | **incompatible-by-design (today)** | D19 (lazy-load generators) + D11 (2.0 `Mapped[]`) + D4 | very high (D19) | decl-base metaclass works (D4); query/lazy patterns + Mapped typing are the wall |
| rich | PARTIAL | D16 (lazy submodule `__getattr__`) + D12 | high (D16) | rich uses module-level lazy attrs; D16 gap bites at import |
| tqdm | **compatible** | D1 + D21 (optional thread) | — | core is pure-Python; iterator wrapper |
| marshmallow | **compatible-via-shim** | D14/D10 | high (D14) | import-green (`test_marshmallow.py`); schema = declarative class synthesis |
| dataclasses-json | **compatible-via-shim** | D14 + D10 | high (D14) | builds on dataclasses shim + type hints |
| toml / tomli | **compatible** | none | — | import-green (`test_toml.py`,`test_tomli.py`); pure-Python parse |

**Triage summary (today, on the import + common-use path):**
fully **compatible**: ~10/25; **compatible-via-shim**: ~4/25 (attrs, marshmallow,
dataclasses-json + the dataclasses family); **PARTIAL** (works at import / pure
path, blocked on a named arc for full function): ~8/25; **incompatible-by-design
today**: ~3/25 (pydantic v2 [D23], sqlalchemy [D19/D11], jinja2 compiled path
[D15]).

---

## LANE D: FEATURE-UNLOCK LEVERAGE RANKING

Ranking dynamism features by **how many of the triaged-25 (and the broader
ecosystem) they unblock when fixed**. This is the five-year prioritization spine.

| Rank | Feature (taxonomy id) | Pkgs unblocked in the 25 | Broader ecosystem | Effort | Convertible? |
|---|---|---|---|---|---|
| 1 | **D19 Generators / `yield`** | httpx, anyio, sqlalchemy (+tornado/starlette/trio outside the 25) | 50+ | Very High | YES — generator-fusion keystone (`docs/design/generator_fusion.md`) |
| 2 | **D23 CPython C-extension support (Track-1 recompile and/or Track-2 ABI bridge)** | pydantic v2 (pydantic-core), markupsafe-C, pyyaml-C (+numpy/scipy/pandas/lxml/pillow/orjson/cryptography outside the 25) | the entire "scientific + perf" tier, 100s | High (Track-2 ~70% built) | YES — **half-built already** (`molt-cpython-abi/`) |
| 3 | **D16 Module-level `__getattr__` (PEP 562)** | rich, sqlalchemy/numpy/scipy top-level imports | 30+ (every lazy-top-level package) | **Low–Medium** | YES — single runtime hook + frontend module-attr fallback |
| 4 | **D11 Forward-ref string annotation resolution** | pydantic v2, sqlalchemy 2.0 `Mapped[]` | 40+ typing-heavy | Medium | YES — typed resolver shim (no general `eval`) |
| 5 | **D26 Regex advanced** | pyyaml, pathspec | 25+ parsers | Medium | YES — native engine feature arc |
| 6 | **D20 asyncio runtime-heavy + wasm** | httpx, anyio, requests/urllib3 functional | 40+ ASGI | High | partial (in-progress; `18_asyncio-wasm-event-loop-fix-plan.md`) |
| 7 | **D9/D8 Protocol `runtime_checkable` + `__subclasshook__`** | pydantic, beartype-style | 20+ | Medium | YES |
| 8 | **D25 Exception `__traceback__` object model** | logging, pytest, sentry-style | 30+ | Medium | YES |
| 9 | **D22 ctypes/cffi FFI completeness** | cryptography, bcrypt | 50+ if cffi-class works | High | partial |
| 10 | **D14 controlled-codegen shim breadth** (NamedTuple, attrs-slotted, enum functional API) | attrs, marshmallow, dataclasses-json | broad | Low (extend existing shim) | YES — extend `dataclasses.py` template |

**Leverage headline:** the **top two ranks (D19 generators, D23 C-extensions) are
the same two the stdlib/GPU audit (doc 16) and doc 17 surfaced** — convergent
evidence they are *the* five-year keystones. The **highest ROI surprise is rank 3
(D16 PEP-562 module `__getattr__`)**: a *low–medium* effort single feature that
unblocks the import of a large lazy-top-level cohort (numpy/scipy/rich/sqlalchemy)
and is currently a silent gap, not a tracked arc.

---

## LANE E: THE ECOSYSTEM RATCHET DESIGN (fail-closed, down-only, manifest-derivable)

The lane's #1 missing piece is a **measurement substrate that makes verdicts
derivable and protects them against regression.** The pattern already exists and
is battle-tested: `tools/check_satellite_parity.py` + `satellite_parity_baseline.json`
(28 pairs, `ratchet_ceiling=2662`, per-pair count+SHA, ceiling may only DECREASE,
fail-closed on new drift). Mirror it exactly.

### E.1 Structure (mirrors the satellite-parity machinery)

- **`tools/check_ecosystem_compat.py`** — a CONTRACT, not a sync/build script.
  For each package in the triage set it produces a *derivable verdict* and
  compares to a committed baseline. It never edits sources.
- **`tools/ecosystem_compat_baseline.json`** — per-package
  `{verdict, required_features, hardest_feature, sha256_of_evidence}` plus a
  one-way **`compatible_floor`** (count of `compatible`-class packages may only
  INCREASE; the guard refuses to lower it) and an **`incompatible_ceiling`**
  (count of `incompatible-by-design` may only DECREASE). Down-only on regression,
  up-only on the good metric — same asymmetry as the satellite ceiling.
- **Verdict is DERIVED, not hand-asserted.** The script computes each package's
  verdict as `min` over its `required_features` looked up in a **machine-readable
  copy of the Lane-B taxonomy** (`tools/dynamism_features.json` — feature id →
  verdict class + evidence path). This is the "manifest-derivable" requirement
  from policy amendment A: change the taxonomy (a feature graduates from
  incompatible→shim→compatible) and *every* package verdict recomputes
  automatically. No per-package verdict is trusted on its own.
- **Every `incompatible-by-design` entry MUST name the exact excluded feature**
  (the script fails closed if an incompatible verdict has an empty
  `excluded_feature`), satisfying the binding requirement.

### E.2 Failure conditions (fail-closed, copied from the satellite guard's spirit)

The guard exits non-zero when, for any package:
- its derived verdict **regressed** (compatible→shim, shim→incompatible) vs.
  baseline, OR
- the **evidence SHA changed** while the verdict was unchanged (the test/script
  that justified the verdict changed — re-verify), OR
- a package has **no baseline entry** (new package needs an explicit verdict), OR
- `compatible_floor` decreased or `incompatible_ceiling` increased, OR
- an `incompatible-by-design` verdict has no named excluded feature.

Graduating a feature in `dynamism_features.json` (e.g., D16 module `__getattr__`
lands) shrinks the incompatible set and *raises* the compatible floor; you then
`--update-baseline`, which the guard permits **only** in the improving direction.

### E.3 Integration with the #46 suite-honesty manifest family

The mission references a "#46 suite-honesty manifest" — no committed file by that
exact name was found in this audit (searched `docs/`, `tools/`, `tests/`); the
existing members of that **machinery family** are
`tools/check_satellite_parity.py`, `tools/check_stdlib_intrinsics.py`
(`tests/test_check_stdlib_intrinsics.py`), and the generated stdlib audit
surfaces under `docs/spec/areas/compat/surfaces/stdlib/`. The ecosystem ratchet
should join this family by:
- emitting a **generated** `docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md`
  (package × verdict × hardest-feature), regenerated from the baseline — same
  "generated from canonical evidence, not hand-maintained" rule STATUS.md already
  states for compat/bench rollups; and
- sharing the `--update-baseline` / down-only-ceiling idiom so a reviewer reads
  one familiar contract across stdlib, satellite, and ecosystem guards.

### E.4 Tie to the functional suite

The `sha256_of_evidence` for each package should hash the **molt-vs-CPython
stdout-equality result** from `tests/runtime_compat/` (extended to record a
machine-readable per-package PASS/FAIL + the dynamism features the script
exercised). This converts the existing mixed import/functional suite (A.7) into
the verdict's *evidence*, closing the "not derivable, not ratcheted, not in CI"
gap in one structural move.

---

## LANE F: RECOMMENDED ARCS (priority order; each a complete structural piece)

Per the no-stopgap doctrine, each arc is a *complete* structural change, not a
partial step. Y1 = foundation/measurement; Y1.5 = the ABI ladder; later =
keystones.

### Arc 1 (Y1, FIRST — the substrate everything else needs): Ecosystem ratchet + machine-readable taxonomy.
Build `tools/dynamism_features.json` (Lane B as data, evidence-pathed),
`tools/check_ecosystem_compat.py` + `ecosystem_compat_baseline.json`, the
generated matrix, and the CI lane — mirroring `check_satellite_parity.py`
exactly. Extend `tests/runtime_compat/` to emit per-package PASS/FAIL + exercised
features as the evidence the verdicts hash. **Complete piece** = verdicts are
derivable + ratcheted + in CI + a published matrix. Without this, all other arcs
are unmeasurable. (Directly closes doc 17's "missing build program" items 1–2,4.)

### Arc 2 (Y1, highest ROI single feature): D16 — module-level `__getattr__` (PEP 562).
Add a runtime module-attribute fallback hook (mirror the class `__getattr__` path
at `attributes.rs:429`) + frontend module-attr lowering fallback, so a module's
`__getattr__(name)` is consulted on missing top-level attributes, and the static
import graph captures the lazily-bound submodules. **Complete piece** = PEP-562
parity across native/WASM/LLVM/Luau + a differential regression + the triage's
rich/numpy-top-level cohort flips at import. Low–medium effort, large unlock
(Lane D rank 3).

### Arc 3 (Y1.5, THE ABI LADDER — the biggest latent asset): finish + prove + reconcile-policy the CPython-ABI bridge.
Three sub-pieces that form one arc:
(a) **Real-package proof**: pick one published compiled wheel that exercises
METH_VARARGS/O/KEYWORDS/FASTCALL (the brief names numpy/pydantic-core/orjson) and
get it importing + running a real call path under the `cext_loader` feature, with
committed wheel+manifest+result (this is the "zero ecosystem proof" gap from doc
17, and the missing 0215-spec evidence).
(b) **Policy reconciliation**: the 0215 spec line *"no CPython ABI
compatibility"* contradicts the shipped bridge. Decide and document the
**two-track contract** (Track 1 libmolt-recompile = primary/safe; Track 2
ABI-dlopen = explicit, allowlisted, capability-gated bridge for unmodifiable
wheels) so the codebase and spec stop disagreeing. Per CLAUDE.md, a code/spec
divergence is itself a defect to fix.
(c) **Capability gating**: route Track-2 loads through the existing capability
manifest (`src/molt/capability_manifest.py`) and `MOLT_EXTENSION_PATH` allowlist,
fail-closed.
**Complete piece** = one real wheel proven on each supported native target + the
two-track policy documented + capability-gated. This flips D23 packages (pydantic
v2 core, numpy-class) from incompatible-by-design to compatible-via-bridge with
evidence. **Do NOT** ship (a) without (b) — shipping a working CPython-ABI bridge
while the spec says it doesn't exist is exactly the kind of code/doc divergence
the doctrine forbids.

### Arc 4 (Y1.5): D11 forward-ref string annotation resolver (no general `eval`).
A typed resolver for `get_type_hints` over string annotations using the static
module symbol table (resolve names structurally; refuse only genuinely dynamic
expressions). **Complete piece** = pydantic v2 / SQLAlchemy 2.0 `Mapped[]` typing
resolves on the supported subset, with the residual genuinely-dynamic cases
raising a named `MOLT_COMPAT_ERROR` (excluded feature = `eval`). Unlocks the
typing-heavy tier (Lane D rank 4).

### Arc 5 (multi-year keystones — sequence behind the substrate): D19 generators, then D26 regex, then D25 traceback.
These are the large lifts already designed/in-flight elsewhere (generator-fusion
keystone; regex engine; traceback object model). They are listed here for
*sequencing* — Arc 1 must precede them so their ecosystem impact is measured by
the ratchet rather than asserted.

### Decisions to make (not arcs, but planning inputs): sdist policy; `.pth`/editable-install policy.
Both are currently ABSENT (A.1). Either implement or document-as-unsupported with
a fail-closed diagnostic — leaving them silent is a sharp edge.

---

## COMPILE-PROBE PENDING (for a later verification wave)

This audit was READ/RESEARCH/WRITE-only (no `cargo`, no `molt build` — shared
target dir was rebuilding). The following claims rest on source evidence and need
a compile-probe to confirm runtime behavior. Each probe is inline and minimal;
run each as `molt build --target native` + run, compared to CPython.

1. **D16 confirmation (module `__getattr__` truly unsupported).** Expect: CPython
   prints `lazy`; molt either errors or AttributeErrors at `mod.foo` — confirming
   the gap.
   ```python
   # probe_pep562.py  — module-level __getattr__
   import probe_pep562_mod as m
   print(m.foo)
   # probe_pep562_mod.py:
   #   def __getattr__(name): return "lazy:" + name
   ```
2. **D11 forward-ref string eval in get_type_hints.**
   ```python
   from __future__ import annotations
   from typing import get_type_hints
   class A:
       x: "int"
       y: "list[str]"
   print(get_type_hints(A))   # CPython resolves to {'x': int, 'y': list[str]}
   ```
3. **dataclasses shim end-to-end (slots + make_dataclass).**
   ```python
   from dataclasses import dataclass, make_dataclass, field
   @dataclass(slots=True, frozen=True)
   class P:
       x: int; y: int = 5
   D = make_dataclass("D", [("a", int), ("b", int, field(default=2))])
   print(P(1).x, P(1).y, D(7).a, D(7).b)
   ```
4. **runtime_checkable Protocol isinstance (D9).**
   ```python
   from typing import Protocol, runtime_checkable
   @runtime_checkable
   class HasName(Protocol):
       def name(self) -> str: ...
   class C:
       def name(self): return "c"
   print(isinstance(C(), HasName))   # CPython True
   ```
5. **abc.register virtual subclass (D7).**
   ```python
   from abc import ABC
   class MyABC(ABC): ...
   class Plain: ...
   MyABC.register(Plain)
   print(issubclass(Plain, MyABC), isinstance(Plain(), MyABC))  # True True
   ```
6. **importlib.metadata.entry_points present at runtime (D24).**
   ```python
   from importlib.metadata import entry_points
   eps = entry_points()
   print(type(eps).__name__)   # should not raise
   ```
7. **ctypes FFI depth (D22)** — does a real `CDLL`/`Structure`/`c_int` round-trip?
   ```python
   import ctypes
   libc = ctypes.CDLL(None)            # platform-dependent; probe behavior/error
   class Pt(ctypes.Structure):
       _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int)]
   p = Pt(3, 4); print(p.x, p.y)
   ```
8. **Track-2 ABI bridge real-wheel import (D23)** — build the crate with
   `--features cext_loader,extension-loader`, point `MOLT_EXTENSION_PATH` at a
   dir holding a CPython 3.12 `.so` (start with the in-repo `_testmolt`), and
   confirm `import _testmolt; _testmolt.add(2,3)==5` end-to-end through the
   runtime import boundary (not just the Rust integration test).
9. **inspect.signature on a decorated function (D12).**
   ```python
   import functools, inspect
   def deco(f):
       @functools.wraps(f)
       def w(*a, **k): return f(*a, **k)
       return w
   @deco
   def g(a: int, b: str = "x") -> bool: ...
   print(str(inspect.signature(g)))   # (a: int, b: str = 'x') -> bool
   ```
10. **The triage's "PARTIAL" verdicts at import** — for each of {requests,
    flask, werkzeug, pyyaml, rich, tqdm}, confirm bare `import X; X.__version__`
    succeeds (the verdict assumes import-green; several have committed
    `tests/runtime_compat/scripts/` evidence but should be re-run on current
    main).

---

## OPEN QUESTIONS

1. **Doc 17 disposition.** This doc (24) supersedes 17. Should 17 be marked
   "superseded by 24" / removed, or kept as the dated first-recon? (Read-only
   audit did not modify it.)
2. **Two-track extension policy.** Is the long-term intent (a) libmolt-recompile
   only, with the CPython-ABI bridge as a *temporary* migration aid; or (b) both
   as permanent first-class tracks? The 0215 spec and the `molt-cpython-abi`
   crate currently encode *different* answers. This is the highest-stakes policy
   ambiguity in the lane and gates Arc 3.
3. **"#46 suite-honesty manifest."** No file by that exact name was found. Is it
   planned-but-uncommitted, or is the intended referent the satellite-parity /
   stdlib-intrinsics guard family? Arc 1's ratchet should slot into whichever is
   canonical.
4. **Scope of D11.** How far should the forward-ref resolver go before it is
   "really `eval`"? A subset (dotted names, subscripts of known generics) is
   safe; arbitrary expressions are not. Where is the line?
5. **Functional-coverage bar per verdict.** What exactly must a package *do*
   (not just import) to earn `compatible`? Arc 1 needs this defined so the
   evidence SHA is meaningful (e.g., "exercises ≥1 documented primary API path").
6. **sdist / `.pth` decisions** (see Lane F) — implement or document-as-refused?

---

## ARC 1 — IMPLEMENTED (the ecosystem ratchet, this audit's Lane E/F-Arc-1 made real)

Lane E's measurement substrate is now built and CI-gated. The verdicts in Lanes
B/C are no longer prose: they are **derived from machine-readable manifests and
ratcheted down-only**, exactly as Lane E specified (mirroring
`tools/check_satellite_parity.py`).

**Artifacts**

- `tools/ecosystem/dynamism_features.json` — Lane B.1's 27-feature taxonomy as
  data; the single source of truth for each feature's `status`
  (`supported` / `typed-shim` / `bridge` / `partial` / `unsupported`), with the
  file:line `evidence` carried over from this document.
- `tools/ecosystem/package_triage.json` — Lane C's 25 packages, each with the
  features it **requires** on its import + commonly-used path (`required_features`)
  plus doc-named-but-optional features (`optional_features`, excluded from the
  min). `compile_probe_status` is `pending` for all (fail-closed; no package is
  molt-build-verified yet — the COMPILE-PROBE wave is still pending).
- `tools/check_ecosystem_compat.py` — re-derives every verdict as the min over
  required features and fails on a hand-edited verdict, a missing
  `evidence`/`excluded_feature`/`tracking`, an unknown feature reference, an
  evidence-SHA drift, or a distribution regression (`compatible_floor` down /
  `incompatible_ceiling` up / `partial_ceiling` up).
- `tools/ecosystem/ecosystem_compat_baseline.json` — the committed one-way
  ratchet.
- `docs/spec/areas/compat/surfaces/ecosystem/ecosystem_compat_matrix.generated.md`
  — generated from the manifests (Lane E.3's "generated, not hand-maintained"
  rule), regenerated via `--update-matrix`.
- CI: a `docs-gates` step in `.github/workflows/ci.yml`, a `lint` gate in
  `pyproject.toml`, and tests in `tests/test_ecosystem_compat.py`.

**Derived verdict distribution (today, 25 packages):** compatible 10,
compatible-via-typed-shim 3, compatible-via-bridge 1, partial 7,
incompatible-by-design 4.

**Interpretations taken to make Lanes B/C machine-checkable** (each flagged
in the manifests' `_interpretation_notes`):

1. **A fifth verdict class `partial` was introduced.** The mission lists four
   classes, but Lane C verdicts ~8 packages PARTIAL (import-green,
   function-blocked on a named arc). Forcing those into one of the four would
   either falsely assert support or falsely assert a permanent exclusion.
   `partial` is ordered between `compatible-via-bridge` and
   `incompatible-by-design`; the four mission classes remain the canonical set.
2. **`rich` is derived `incompatible-by-design`, tightening its Lane C cell.**
   Lane C's summary says `rich = PARTIAL`, but this document's own feature-level
   evidence overrides it: D16 (module `__getattr__`) is incompatible-by-design
   TODAY, rich's note says "D16 gap bites at import," and Lane D rank 3 says D16
   blocks rich's import. PARTIAL means "imports"; rich does not. Fail-closed →
   incompatible. (This is why the derived incompatible count is 4, not the
   Lane-C-summary "~3"; the partial count is 7, not "~8".)
3. **`required` vs `optional` was reconstructed from each row's Notes + Lane B's
   path-scope rule**, because Lane C's "Hardest required feature" column is not a
   complete required set and sometimes names optional features (e.g. six/D17,
   tqdm/D21, httpx-anyio/D19, pyyaml/D26+D23). werkzeug gained D20 from its
   "functional serve = socket/async" note so its derived verdict matches the
   stated PARTIAL. D20 (async, dual-state) and D8/D9/D17/D21/D22/D25 (PARTIAL)
   are encoded `partial`; the TODAY status governs all "→ shim-able/arc" rows
   (fail-closed), with the conversion arc recorded in each feature's `tracking`.

This closes Lane E and OQ3's machinery question: the ratchet joins the
`check_satellite_parity` / `check_stdlib_intrinsics` guard family. **Still
pending** (not part of Arc 1): the functional-suite tie (Lane E.4 / OQ5 — wiring
`tests/runtime_compat/` per-package PASS/FAIL into the evidence SHA so
`compile_probe_status` flips from `pending`), and Arcs 2–5.

---

## APPENDIX: corrections to doc 17 (for the record)

- ⚠️**CORRECTS-17 §4.3** ("metaclass generation likely unsupported, not
  documented"): metaclass `__prepare__` + winner resolution + conflict detection
  are implemented (`types.rs:4757`–`4901`); `__init_subclass__`
  (`ops.rs:9886`) and `__set_name__` (`ops.rs:9780`) are implemented.
- ⚠️**CORRECTS-17 §4.5** (`get_type_hints` "pending"): implemented
  (`typing.py:1260`); `@runtime_checkable` exists (`:873`); `inspect.signature`
  intrinsic-backed (`inspect.py:48`).
- ⚠️**CORRECTS-17 LANE 2** ("tests only `import X` + `hasattr` — no functional
  testing") and ⚠️**LANE 3 + capability table** ("zero ecosystem proof," "CPython
  C-API extensions ABSENT — explicitly blocked"): the runtime-compat suite is
  mixed import+functional (`test_tenacity.py`, `test_attrs.py` run real code);
  and a ~6,250-line CPython binary-ABI bridge exists and is runtime-wired
  (`runtime/molt-cpython-abi/` + `platform.rs:3652`), with a passing
  compile-dlopen-call integration test (`cext_integration.rs`). The accurate
  statement is "no committed *real published-wheel* proof," not "absent."
