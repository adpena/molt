# 0017 Stdlib Lowering Sweep (Intrinsic-First)

Generated: 2026-02-08
Scope: `src/molt/stdlib/**`, `runtime/molt-runtime/src/builtins/**`, `src/molt/frontend/__init__.py`, compatibility matrix coverage.

## Method
- Intrinsics audit: `python3 tools/check_stdlib_intrinsics.py --json-out /tmp/stdlib_intrinsics_audit.json`
- Strict lowering gate smoke: `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
- TODO/API gap scan: `rg -n "TODO\(" src/molt/stdlib`
- Fail-fast surface scan: `raise NotImplementedError`, `MOLT_COMPAT_ERROR`
- Matrix gap extraction: `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md`

## Current Lowering State
From `/tmp/stdlib_intrinsics_audit.json`:
- `intrinsic-backed`: 58 modules
- `intrinsic-partial`: 16 modules
- `probe-only`: 13 modules
- `python-only`: 25 modules

Critical strict-import gate currently fails for:
- `asyncio` (`intrinsic-partial`)
- `pathlib` (`intrinsic-partial`)
- `socket` (`intrinsic-partial`)

## CPython Fallback Sweep Findings
### A. Explicit CPython-style import fallback in production stdlib
- No `except ImportError` / `except ModuleNotFoundError` fallback chains found in non-test stdlib modules.
- Test-only fallback imports exist under `src/molt/stdlib/test/**`.

### B. Runtime/stdlib fallback-like fail-fast paths still present
- `runtime/molt-runtime/src/builtins/modules.rs:1496`
- `runtime/molt-runtime/src/builtins/modules.rs:1525`
- `runtime/molt-runtime/src/builtins/platform.rs:2034`

These are explicit TODO-marked restricted-source execution lanes for `runpy`/`importlib` module execution and extension/sourceless execution parity. They remain the highest-impact runtime-lowering blockers.

### C. Frontend bridge/fallback policy surfaces
- `src/molt/frontend/__init__.py` contains `bridge` policy plumbing and `_bridge_fallback` emission paths.
- `src/molt/compat.py` models bridge-tier compatibility outcomes.

This is compile-time tooling behavior, not compiled-binary runtime behavior. It is still a policy surface to reduce/retire as strict lowering expands.

## Missing PEP/API Coverage (TODO Taxonomy Sweep)
TODO count in stdlib: 29 (`P1`: 8, `P2`: 9, `P3`: 12)

Highest-priority (`P1`) coverage gaps:
- `src/molt/stdlib/asyncio.py` SSL transport orchestration intrinsic-lowering
- `src/molt/stdlib/socket.py` full socket parity (sendmsg/recvmsg, ancillary data, timeout/error subclass edges)
- `src/molt/stdlib/re.py` lookarounds/backrefs/named groups/Unicode semantics
- `src/molt/stdlib/json.py` remaining parity surfaces
- `src/molt/stdlib/pickle.py` remaining opcode/type coverage
- `src/molt/stdlib/email/message.py` full policy/headerregistry semantics
- `src/molt/stdlib/math.py` determinism-policy completion
- `src/molt/stdlib/multiprocessing/__init__.py` divergent runtime TODO

## Fail-fast/API-unsupported Hotspots (non-test stdlib)
Modules with explicit `raise NotImplementedError`:
- `src/molt/stdlib/re.py`
- `src/molt/stdlib/multiprocessing/__init__.py`
- `src/molt/stdlib/zipfile.py`
- `src/molt/stdlib/importlib/resources/__init__.py` (write-mode guardrails)
- `src/molt/stdlib/_bz2.py`
- `src/molt/stdlib/logging.py`
- `src/molt/stdlib/random.py`
- `src/molt/stdlib/dataclasses.py`

`MOLT_COMPAT_ERROR` usage in production stdlib:
- `src/molt/stdlib/doctest.py`

## Missing/Planned Module Inventory (Matrix vs Implemented Module Set)
Representative planned tokens not yet implemented as concrete stdlib modules include:
- Core/data: `array`, `fractions`, `statistics`, `datetime`, `zoneinfo`, `gzip`, `bz2`, `lzma`, `zlib`, `secrets`, `binascii`, `tomllib`, `xml`, `unicodedata`
- Tooling/language: `argparse`, `getopt`, `atexit`, `queue`, `calendar`, `difflib`, `dis`, `marshal`, `tokenize`, `tracemalloc`
- Capability-gated system/network/UI: `subprocess`, `ssl`, `sqlite3`, `ftplib`, `imaplib`, `smtplib`, `tkinter`, `webbrowser`, `venv`, `readline`, `resource`, `pty`, `pwd`, `winreg`

See `docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md` for full planned list and ownership.

## Recommended Next Tranches (strict lowering order)
1. Runtime execution parity blockers
- Replace restricted-source TODO lanes in:
  - `runtime/molt-runtime/src/builtins/modules.rs`
  - `runtime/molt-runtime/src/builtins/platform.rs`
- Goal: remove TODO-marked shim execution path and move to runtime-owned execution primitives.

2. Strict-import roots to intrinsic-backed
- Bring `socket`, `pathlib`, `asyncio` from `intrinsic-partial` to `intrinsic-backed` so `--critical-allowlist` passes.

3. P1 API/PEP cluster
- `re`, `socket`, `json`, `pickle`, `email.message`, `math` parity deltas with targeted differential additions.

4. Probe-only to intrinsic-backed cluster
- `json`, `warnings`, `unittest`, `tempfile`, `http.client`, `http.server`, `contextvars`, `random`, `base64`, `string`, `pprint`, `bisect`.

5. Python-only module landing cluster
- Start with high-impact planned modules used by ecosystems: `datetime`, `subprocess`, `ssl`, `sqlite3`, `argparse`, `queue`, `zlib`.

## Notes
- This sweep found no production stdlib ImportError-based CPython fallback import chains.
- Main remaining risk is not silent fallback; it is explicit unsupported/partial behavior that must be lowered to runtime intrinsics for parity.
