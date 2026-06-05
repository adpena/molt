<!-- Ecosystem-compatibility recon (background agent, 2026-06-04). Read-only audit; codebase = sole source of truth. -->
<!-- User directive: "The five year plan should include ecosystem compatibility as well which we have already made significant progress on" — this is the evidence-grounded map of that progress and its gaps (the sibling of doc 16). -->

# Ecosystem / Third-Party Compatibility Gap Audit

## EXECUTIVE SUMMARY

The molt codebase reveals a **foundational but incomplete ecosystem compatibility layer**. While the compiler successfully handles package *resolution* and basic *third-party imports*, actual ecosystem support is thin. There are **34 known-green packages** tested at import level, **zero committed gap audit** for ecosystem (vs. the stdlib/GPU audit at `docs/design/foundation/16_cpython-surface-stdlib-gpu-gap-audit.md`), and **no documented policy** on pure-Python vs. native-extension boundaries enforced by tests. The "significant progress" is infrastructure-grade but lacks validation evidence.

---

## LANE 1: PACKAGE RESOLUTION & IMPORT GRAPH

### 1.1 How `molt build app.py` Discovers & Compiles Packages

**Entry point:** `src/molt/cli.py:4626-4682` (`_resolve_module_roots`)

**Module roots discovery (in order):**
1. **MOLT_MODULE_ROOTS env var** (line 4642): explicit override (`:`-separated)
2. **Project roots** (4650-4655): project_root, cwd_root, and `{root}/src/` if they exist
3. **Vendor directories** (4656): `{root}/vendor/packages` and `{root}/vendor/local` via `_vendor_roots()` (line 1280)
4. **PYTHONPATH** (4657-4665): if `--respect-pythonpath` flag or config `respect_pythonpath=true`
5. **Explicit lib paths** (4667-4670): `--lib-path=...` CLI args or `[tool.molt] lib-paths` in pyproject.toml
6. **Active venv site-packages** (4671-4677): auto-detect `.venv/lib/python*/site-packages` (unless `MOLT_HERMETIC_MODULE_ROOTS=1`)
7. **.molt-venv site-packages** (4679-4681): UV-managed venv at `.molt-venv/lib/python*/site-packages`

`cli.py:1298-1312` (`_molt_venv_site_packages`) discovers both Unix (`lib/python*/site-packages`) and Windows (`Lib/site-packages`) layouts.

**Static import graph construction:** `cli.py:7886-8087` (`_discover_module_graph_from_paths`)
- AST-parses entry paths, resolves imports via `resolution_cache.resolve_module()`
- Persisted caching (7917-7941) avoids re-parsing on rebuilds
- Module-chain expansion (`foo.bar.baz` → `foo`, `foo.bar`, `foo.bar.baz`)
- Package detection (7959): `is_package = path.name == "__init__.py"`
- **Namespace package support** (4761-4795): PEP 420 namespace dirs detected; auto-generated stubs `namespace_{safe_name}.py` at build time (4926-4928)

### 1.2 Wheel & sdist Support

**Wheels supported; sdist support unclear (likely absent).**
- `cli.py:2730-2766` — wheel path resolution/validation; `_wheel_filename_tags()` extracts (python_tag, abi_tag, platform_tag)
- Wheels via `--wheel=...` or `[tool.molt] wheel = "path"`
- ABI tag validation (2752-2757): `molt_abi*` matching; platform tag validation (2758-2766); `wheel_sha256` checksum (2768-2776)
- Extension wheels (2778-2810): embedded native payloads with `extension_sha256`; manifest requires `molt_c_api_version`, `capabilities`, `abi_tag`, `target_triple`, `platform_tag`, `module`
- **No explicit sdist build/extraction found** — packages must be pre-built wheels or pure-Python source trees

### 1.3 Vendoring & Editable Installs

- Vendoring: `cli.py:1280-1287` — `{project}/vendor/packages/` and `{project}/vendor/local/`
- Editable installs: **`.pth` files NOT processed** (no handling found); UV-managed `.molt-venv` auto-discovery works; editable installs must resolve to real site-packages paths

---

## LANE 2: WHAT ALREADY WORKS (KNOWN-GREEN PACKAGES)

**Test location:** `tests/runtime_compat/scripts/` — **34 packages tested at import level:**

attrs, cachetools, certifi, chardet, click, colorama, decorator, django (partial), faker, filelock, flask, invoke, itsdangerous, jinja2, json-stdlib, marshmallow, packaging, pathspec, platformdirs, pydantic (v1 API check), pygments, pyjwt, pyyaml, requests, schedule, simplejson, six, tenacity, toml, tomli, trio, watchdog, wcwidth, werkzeug.

**Test harness:** `tests/runtime_compat/test_runtime_compat.py` — runs CPython → Molt build → Molt run per script; compares stdout (tolerates version-string/whitespace diffs). Timeouts: CPython 30s / build 120s / run 30s.

**Limitations of the current suite:**
- Tests only `import X` + basic attribute checks (`hasattr(X, "get")`) — **no functional testing**
- **No CI lane**; no published compat matrix; failures not tracked/aggregated
- No transitive-dependency manifests; no per-package capability mapping

---

## LANE 3: C-EXTENSION BOUNDARY & NATIVE EXTENSION POLICY

**Policy statement:** `docs/spec/areas/tooling/0215_MOLT_EXTENSION_BUILD_PIPELINE.md` line 14: **"Extensions must be recompiled; no CPython ABI compatibility."** Supported model = `libmolt` C-extension API (Molt-native, NOT the CPython C-API).

- CLI: `molt extension build` (`--project`, `--out-dir`, `--molt-abi`, `--target`, `--capabilities`, `--deterministic`, `--json`) → `.whl` tagged `py3-molt_abi<major>-<platform_tag>` + `extension_manifest.json`
- Required `[tool.molt.extension]` metadata: `molt_c_api_version`, `capabilities`, `determinism`, `effects`, `module`, `sources`, `include_dirs`, `extra_compile_args`, `extra_link_args` (spec lines 61-77)
- Validation: `cli.py:2663-2820` (`_validate_extension_manifest`) — version format, ABI tag (`molt_abi{MAJOR}`), wheel + payload checksums; failures cached per path+manifest fingerprint
- `docs/spec/areas/tooling/0602_WHEN_TO_WRITE_EXTENSIONS_OR_BINARIES.md` §4.2: "C extensions (avoid as a product strategy)"
- ctypes exists (`src/molt/stdlib/ctypes/`) as a native bridge; dynamic linking unsupported on WASM
- **Known gap:** zero evidence of a real package (numpy/msgpack/bcrypt-class) recompiled against libmolt with committed results — the pipeline is infrastructure without ecosystem proof

---

## LANE 4: BLOCKERS INVENTORY (LANGUAGE/RUNTIME GAPS)

### 4.1 Dynamic execution — INTENTIONALLY UNSUPPORTED
`src/molt/stdlib/builtins.py:300-315`: `eval`/`exec` raise `MOLT_COMPAT_ERROR` ("dynamic code execution is outside the verified subset"); `compile` validation-only. Affected: jinja2 custom dynamic filters, pydantic eval_type configs, doctest (blocked), eval-using ORMs.

### 4.2 Reflection & introspection
getattr/setattr/hasattr/dir: supported (intrinsic-backed). `inspect`: partial. Exception `__traceback__`: pending. `sys.breakpointhook`: unsupported.

### 4.3 Metaclass & `__init_subclass__`
Basic descriptor protocol works (intrinsic-backed classmethod/staticmethod/property boot); dynamic metaclass generation likely unsupported — NOT explicitly documented (needs a spec).

### 4.4 Decorators
Static decorators supported; stateful/parametrized fine unless they reach eval/exec.

### 4.5 Typing runtime behaviors
`typing.get_type_hints()` pending; `Protocol`/ABC bootstrap partial (TODO SL1 scaffolding); `@runtime_checkable` likely unsupported. **Blocks pydantic v2 / fastapi class workloads.**

### 4.6 Generator & iterator protocol — INCOMPLETE (highest-impact)
`yield`/`yield from`/async generators pending (the generator-fusion keystone). Affects trio, tenacity, fastapi/starlette, sqlalchemy query patterns — 50+ packages.

### 4.7 Monkeypatching & runtime mutation — NOT SUPPORTED BY DESIGN
Static import graph: runtime `sys.modules`/`module.__dict__` patching cannot be reflected. mock.patch/pytest-fixture-heavy packages fail. (Consistent with the project's stated parity exceptions.)

---

## COMPAT ERROR MACHINERY

`src/molt/compat.py:15-61` (`CompatibilityIssue`, `CompatibilityError`); raise sites: builtins.py:302 (eval/exec), doctest.py:~260, sys.py + _sys_impl.py (breakpointhook). Structured error format with feature/location/tier/impact/replace fields; `MOLT_COMPAT_WARNINGS=0` suppression.

**Capability registry:** `src/molt/capability_manifest.py:43-58` — `net, websocket.connect, websocket.listen, fs.read, fs.write, env.read, env.write, db.read, db.write, time.wall, time, random`. **No package→capabilities mapping exists.**

---

## TOP-10 HIGHEST-LEVERAGE ECOSYSTEM ARCS (packages unblocked × foundation criticality)

| Rank | Arc | Packages Unblocked | Criticality | Design Coverage |
|------|-----|-------------------|-------------|-----------------|
| 1 | **Generator protocol (yield/yield from)** | 50+ (tenacity, trio, fastapi, starlette, tornado, sqlalchemy) | CRITICAL | generator_fusion.md (compiler side); stdlib wiring none |
| 2 | **Typing runtime (get_type_hints, Protocol, runtime_checkable)** | 40+ (pydantic v2, fastapi, attrs configs) | HIGH | Partial (TODO SL1) |
| 3 | **Exception __traceback__ chain** | 30+ (logging, pytest, formatters) | MEDIUM | None |
| 4 | **Regex advanced (lookahead, named groups, backrefs)** | 25+ (pyyaml, pathspec, parsers) | MEDIUM | None |
| 5 | **asyncio runtime-heavy (event-loop policy, zipimport)** | 40+ (ASGI ecosystem) | CRITICAL for async | In progress (design agent, this session) |
| 6 | **ssl/socket MSG_* flags** | 15+ | LOW-MEDIUM | None |
| 7 | **os.walk generator semantics** | 20+ (indexers, linters) | MEDIUM | Via generator fusion |
| 8 | **dir_fd parameter family** | 10+ (POSIX tools) | MEDIUM | In progress (design agent, this session) |
| 9 | **Threading primitives completeness** | 25+ | MEDIUM | Partial |
| 10 | **ctypes/cffi FFI completeness + libmolt extension proof** | 50+ if recompilation works (numpy-class) | HIGH | Partial (0215 spec; zero ecosystem proof) |

---

## PRESENT/PARTIAL/ABSENT CAPABILITY TABLE

| Capability | Status | Notes |
|-----------|--------|-------|
| Package resolution | PRESENT | wheels, source trees, vendor dirs, namespace pkgs |
| Pure-Python packages | PRESENT | 34 import-green |
| Native extensions via libmolt | PARTIAL | pipeline exists; ecosystem proof absent |
| CPython C-API extensions | ABSENT | explicitly blocked (policy) |
| cffi/ctypes | PARTIAL | wasm dynamic linking unsupported |
| eval/exec/compile | ABSENT | policy |
| Generator protocol | ABSENT | keystone arc |
| Async/await core | PRESENT | runtime-heavy tranche fails |
| Threading | PARTIAL | primitives pending |
| Typing runtime | PARTIAL | get_type_hints pending |
| Reflection (getattr/dir) | PRESENT | intrinsic-backed |
| Exception chains | PARTIAL | __traceback__ pending |
| Regex advanced | ABSENT | native engine gap |
| os.walk laziness | ABSENT | eager-list OOM |
| Monkeypatching | ABSENT | by design (static graph) |
| .pth files | ABSENT | not processed |
| sdist | ABSENT (unverified) | wheels/source-trees only |

---

## WHAT EXISTS vs WHAT'S MISSING

**Exists:** stdlib/GPU gap audit (doc 16); extension pipeline spec (0215); runtime-compat harness + 34 import tests; capability registry; structured compat errors.

**Missing (the build program):**
1. **Functional (not import-only) package tests** + results tracking + a CI lane gating on the known-green set
2. **Published compat matrix** (package×version → status) + transitive-dependency manifests
3. **libmolt extension proof-of-concept** — recompile a real package (msgpack/bcrypt-class) and commit wheel+manifest+results
4. **Package→capabilities mapping**
5. **Metaclass/`__init_subclass__` support spec** (undocumented today)
6. **.pth / editable-install handling decision** (support or document-as-unsupported)
7. **sdist policy decision**

## RECOMMENDED ARC ORDER (ecosystem track)
1. Functional-test the 34 known-green packages (turns "imports" into "works") + CI lane + results JSON — the measurement substrate everything else needs
2. Generator protocol + typing runtime (ranks 1-2 — they unblock the modern-framework tier)
3. libmolt extension proof (rank 10 — converts the extension pipeline from spec to evidence)
4. Compat matrix publication + package-capability manifests
