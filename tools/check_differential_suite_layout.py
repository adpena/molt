#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
DIFF_ROOT = ROOT / "tests" / "differential"
COVERAGE_INDEX = DIFF_ROOT / "COVERAGE_INDEX.yaml"
# The canonical differential lanes. Each is a first-class differential corpus
# discovered and run by tests/molt_diff.py; the committed native suite-honesty
# snapshot (tools/suite_honesty/native_calibration.jsonl) calibrates
# {basic, stdlib, loop_overflow_peel, memory, pyperformance}. The P0 hard-pass
# regression lanes (overflow_scalar, int_loop_modulo, memory_safety) are run by
# explicit path and are intentionally NOT in the expected-fail honesty ledger —
# they assert silent-wrong-answer / memory-corruption fixes that must always
# match CPython, so they may never be marked xfail.
ALLOWED_LANES = {
    "basic",  # core language + builtin (non-module) semantics. runner: tests/molt_diff.py
    "stdlib",  # stdlib module/submodule semantics. runner: tests/molt_diff.py
    "moltlib",  # molt-specific runtime/library features. runner: tests/molt_diff.py
    "pyperformance",  # targeted pyperformance smoke inputs. runner: tests/molt_diff.py
    # The loop integer-overflow peel matrix (the cited "peel matrix 9/9" gated
    # lane): seeded/boundary sum() programs proving 47-bit/63-bit overflow
    # promotion stays BigInt-correct. runner: tests/molt_diff.py.
    "loop_overflow_peel",
    # The RC/RSS-bounded corpus (DropInsertion arc): drop-site / alias /
    # generator-consumer programs run under the molt_diff memory guard (RSS
    # measurement default-on). runner: tests/molt_diff.py.
    "memory",
    # Scalar (non-loop) integer-overflow soundness (INT-lane unification,
    # RawI64FullDeopt tier): the CheckedAdd/CheckedMul slow-path deopt plus
    # from_int inline-window / as_int truncation boundaries. Seeded/boundary
    # mul + accumulate programs proving an i64 overflow re-executes on a BigInt
    # carrier and never silently wraps. Distinct from loop_overflow_peel: scalar
    # boundary values, not the sum() loop peel matrix. runner: tests/molt_diff.py.
    "overflow_scalar",
    # Loop-induction-variable division-family carrier correctness:
    # `i % const` / floordiv / var-divisor programs proving the boxed-fallback
    # store and its raw-i64 consumers agree on the carrier (the modulo-carrier
    # P0 — a NaN-boxed result read back through a raw-i64 store = silent wrong
    # answer). runner: tests/molt_diff.py.
    "int_loop_modulo",
    # P0 memory-SAFETY corruption corpus (Spine-4 ownership-lattice trust root):
    # resurrection / weakref-callback ordering / deopt-index bounds programs that
    # a wrong-place or wrong-order Free would turn into a use-after-free. Hard
    # pass against CPython (never xfail); the RC/RSS-bounded sibling cases live in
    # the `memory` lane. runner: tests/molt_diff.py.
    "memory_safety",
}

# Core Python 3.12+ PEP coverage tracked in COVERAGE_INDEX.yaml.
REQUIRED_CORE_PEPS = {
    "448",
    "526",
    "563",
    "570",
    "572",
    "584",
    "585-604",
    "618",
    "634",
    "649",
    "657",
    "695",
    "701",
}

# Tests that intentionally live in basic while still being listed under stdlib
# coverage entries because they assert builtin/core behavior boundaries.
STDLIB_SECTION_BASIC_ALLOWLIST = {
    "tests/differential/basic/builtins_basic.py",
    "tests/differential/basic/builtins_api_surface_312_plus.py",
    "tests/differential/basic/bytes_translate_maketrans.py",
    "tests/differential/basic/context_return_unwind_scope.py",
    "tests/differential/basic/oserror_errno.py",
    "tests/differential/basic/unicode_surrogate_handling.py",
}

STDLIB_SECTION_BASIC_ALLOW_PREFIXES = ("tests/differential/basic/builtins_",)

# Basic-lane files whose leading filename token collides with a stdlib module
# name in STDLIB_PREFIXES, but whose CONTENT tests compiler internals or a
# builtin (non-module) surface — NOT the stdlib module the prefix names. Each
# entry is an explicit, justified exception to the "no stdlib-prefixed file in
# basic/" rule. Adding a NEW stdlib-prefixed basic file still fails the check
# until it is triaged into a lane or justified here (fail-closed).
BASIC_LANE_PREFIX_ALLOWLIST = {
    # `copy` token vs the `copy` module: exercises the LLVM TIR
    # `Copy[_original_kind=...]` value-producing lowering arms (int_from_obj,
    # slice, dict_keys, enumerate, object_new, ...), a backend codegen test.
    "tests/differential/basic/copy_arm_conversions.py",
    # `copy` token vs the `copy` module: the native-backend `copy` op must route
    # to its family handler via `fc::native_op_family` (the arm<->HANDLED_KINDS
    # authority); a dispatch-routing regression test, not the `copy` module. No
    # `import copy`.
    "tests/differential/basic/copy_op_routing.py",
    # `struct` token vs the `struct` module: MemGVN store-to-load forwarding
    # (S5-2b) on instance fields — a compiler memory-optimization test.
    "tests/differential/basic/struct_field_forwarding.py",
    # `struct` token vs the `struct` module: SROA / scalar-replacement-of-
    # aggregates (S5-2d) on non-escaping objects — a compiler codegen test.
    "tests/differential/basic/struct_sroa.py",
    # `operator` token vs the `operator` module: core `or`/`and` short-circuit
    # operator-protocol semantics (operand return + type preservation), no
    # `import operator`.
    "tests/differential/basic/operator_semantics.py",
    # `string` token vs the `string` module: the `str.split()` builtin method
    # under scalar/control-flow forwarding — a str-builtin compiler test, no
    # `import string`.
    "tests/differential/basic/string_split_scalar_control.py",
    # `string` token vs the `string` module: split-field int parsing and field
    # deforestation over the `str.split()` builtin method — a compiler
    # optimization test, no `import string`.
    "tests/differential/basic/string_split_field_int_parse.py",
    # `stdlib` token: the module-attribute-access lowering mechanism exercised
    # across multiple modules (sys.platform / version_info) — a compiler
    # attribute-access test, not single-module stdlib semantics.
    "tests/differential/basic/stdlib_attr_access.py",
    # `stdlib` token: broad module-attribute access (sys + os + math) — same
    # compiler attribute-access mechanism, multi-module vehicle.
    "tests/differential/basic/stdlib_attr_broad.py",
    # `stdlib` token: chained module-attribute access (sys.version_info.major,
    # os.path.sep, math.floor(...)) — the chained-attribute lowering path.
    "tests/differential/basic/stdlib_attr_chained.py",
}

# Leading filename tokens (split on the first "_") that name a stdlib module.
# A basic/ file whose first token is in this set is presumed misfiled (it should
# live in the stdlib lane) unless explicitly justified in
# BASIC_LANE_PREFIX_ALLOWLIST. NOTE: "test" is included deliberately — it is not
# only the stdlib `test` package but also DOUBLES AS enforcement of the
# differential no-`test_`-prefix naming convention: differential test files are
# named for what they cover (error_messages_*.py), never `test_*.py`. After the
# 2026-06 layout cleanup there are zero `test_`-prefixed files in basic/.
STDLIB_PREFIXES = {
    "abc",
    "argparse",
    "ast",
    "asyncio",
    "bisect",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "collections",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "csv",
    "ctypes",
    "datetime",
    "decimal",
    "dis",
    "doctest",
    "email",
    "encodings",
    "enum",
    "errno",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "http",
    "httpclient",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "linecache",
    "locale",
    "logging",
    "mailbox",
    "marshal",
    "math",
    "multiprocessing",
    "netrc",
    "opcode",
    "operator",
    "os",
    "pathlib",
    "pdb",
    "pickle",
    "pkgutil",
    "plistlib",
    "poplib",
    "posix",
    "pprint",
    "py",
    "queue",
    "random",
    "re",
    "runpy",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "smtplib",
    "socket",
    "sre",
    "ssl",
    "stat",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "symtable",
    "sys",
    "tabnanny",
    "tempfile",
    "test",
    "textwrap",
    "threading",
    "time",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "urllib",
    "uuid",
    "warnings",
    "weakref",
    "wsgi",
    "wsgiref",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zoneinfo",
    "stdlib",
    "windows",
    "zlib",
}


def _collect_lane_files() -> tuple[list[str], list[str]]:
    wrong_lane: list[str] = []
    basic_stdlib_prefix: list[str] = []
    for path in sorted(DIFF_ROOT.rglob("*.py")):
        rel = path.relative_to(ROOT).as_posix()
        lane = path.relative_to(DIFF_ROOT).parts[0]
        if lane not in ALLOWED_LANES:
            wrong_lane.append(rel)
        if lane == "basic" and path.parent == DIFF_ROOT / "basic":
            prefix = path.stem.split("_", 1)[0]
            if prefix in STDLIB_PREFIXES and rel not in BASIC_LANE_PREFIX_ALLOWLIST:
                basic_stdlib_prefix.append(rel)
    return wrong_lane, basic_stdlib_prefix


def _parse_coverage_index() -> tuple[set[str], set[str], set[str], set[str]]:
    text = COVERAGE_INDEX.read_text(encoding="utf-8").splitlines()
    all_paths: set[str] = set()
    stdlib_paths: set[str] = set()
    pyperformance_paths: set[str] = set()
    peps: set[str] = set()
    section: str | None = None
    for raw in text:
        if raw.startswith("core:"):
            section = "core"
            continue
        if raw.startswith("pyperformance:"):
            section = "pyperformance"
            continue
        if raw.startswith("stdlib:"):
            section = "stdlib"
            continue
        pep_match = re.match(r'\s+"([^"]+)":\s*$', raw)
        if section == "core" and pep_match:
            candidate = pep_match.group(1)
            if candidate and candidate[0].isdigit():
                peps.add(candidate)
        stripped = raw.lstrip()
        if not stripped.startswith("- "):
            continue
        maybe_path = stripped[2:].strip()
        if not maybe_path.endswith(".py"):
            continue
        all_paths.add(maybe_path)
        if section == "stdlib":
            stdlib_paths.add(maybe_path)
        elif section == "pyperformance":
            pyperformance_paths.add(maybe_path)
    return all_paths, stdlib_paths, pyperformance_paths, peps


def main() -> int:
    errors: list[str] = []

    wrong_lane, basic_stdlib_prefix = _collect_lane_files()
    if wrong_lane:
        errors.append(
            "unexpected differential lane directories:\n- " + "\n- ".join(wrong_lane)
        )
    if basic_stdlib_prefix:
        errors.append(
            "basic lane has stdlib-prefixed tests (move to stdlib lane):\n- "
            + "\n- ".join(basic_stdlib_prefix)
        )

    # The justification allowlists must not rot: an entry pointing at a file
    # that no longer exists (deleted or moved) is a stale exception that would
    # silently mask a future same-named file. Fail closed on dead entries so the
    # allowlist stays a live, audited ledger.
    stale_allowlist = sorted(
        rel
        for rel in (BASIC_LANE_PREFIX_ALLOWLIST | STDLIB_SECTION_BASIC_ALLOWLIST)
        if not (ROOT / rel).exists()
    )
    if stale_allowlist:
        errors.append(
            "stale allowlist entries (file no longer exists — remove the "
            "justification):\n- " + "\n- ".join(stale_allowlist)
        )

    all_paths, stdlib_paths, pyperformance_paths, peps = _parse_coverage_index()
    missing_paths = sorted(path for path in all_paths if not (ROOT / path).exists())
    if missing_paths:
        errors.append(
            "COVERAGE_INDEX has missing test paths:\n- " + "\n- ".join(missing_paths)
        )

    retired_refs = sorted(
        path
        for path in all_paths
        if "/planned/" in path or "/core/" in path or "/scoping/" in path
    )
    if retired_refs:
        errors.append(
            "COVERAGE_INDEX still references retired lanes:\n- "
            + "\n- ".join(retired_refs)
        )

    stdlib_lane_violations = sorted(
        path
        for path in stdlib_paths
        if not path.startswith("tests/differential/stdlib/")
        and path not in STDLIB_SECTION_BASIC_ALLOWLIST
        and not any(
            path.startswith(prefix) for prefix in STDLIB_SECTION_BASIC_ALLOW_PREFIXES
        )
    )
    if stdlib_lane_violations:
        errors.append(
            "stdlib coverage section points outside stdlib lane:\n- "
            + "\n- ".join(stdlib_lane_violations)
        )

    pyperformance_lane_violations = sorted(
        path
        for path in pyperformance_paths
        if not path.startswith("tests/differential/pyperformance/")
    )
    if pyperformance_lane_violations:
        errors.append(
            "pyperformance coverage section points outside pyperformance lane:\n- "
            + "\n- ".join(pyperformance_lane_violations)
        )

    missing_peps = sorted(REQUIRED_CORE_PEPS - peps)
    if missing_peps:
        errors.append(
            "core PEP coverage entries missing from COVERAGE_INDEX:\n- "
            + "\n- ".join(missing_peps)
        )

    if errors:
        print("[FAIL] differential suite layout check failed")
        for err in errors:
            print(err)
        return 1

    print("[OK] differential suite layout check passed")
    print(f"[OK] lanes: {', '.join(sorted(ALLOWED_LANES))}")
    print(f"[OK] indexed tests: {len(all_paths)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
