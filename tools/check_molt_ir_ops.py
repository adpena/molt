#!/usr/bin/env python3
from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC_PATH = ROOT / "docs/spec/areas/compiler/0100_MOLT_IR.md"
FRONTEND_PATH = ROOT / "src/molt/frontend/__init__.py"
NATIVE_BACKEND_PATH = ROOT / "runtime/molt-backend/src/lib.rs"
WASM_BACKEND_PATH = ROOT / "runtime/molt-backend/src/wasm.rs"

# Canonical aliases where spec naming and frontend op naming differ intentionally.
ALIASES: dict[str, list[str]] = {
    "ConstInt": ["CONST", "CONST_BIGINT"],
    "Branch": ["IF", "ELSE", "END_IF"],
    "Return": ["ret"],
    "Throw": ["RAISE"],
    "LoadAttr": ["GETATTR"],
    "StoreAttr": ["SETATTR"],
    "LoadIndex": ["INDEX"],
    "Iter": ["ITER_NEW"],
    "ClosureLoad": ["LOAD_CLOSURE"],
    "ClosureStore": ["STORE_CLOSURE"],
    "GetAttrGenericPtr": ["GETATTR_GENERIC_PTR"],
    "SetAttrGenericPtr": ["SETATTR_GENERIC_PTR"],
    "GetAttrGenericObj": ["GETATTR_GENERIC_OBJ"],
    "SetAttrGenericObj": ["SETATTR_GENERIC_OBJ"],
    "IntArrayFromSeq": ["INTARRAY_FROM_SEQ"],
    "MemoryViewNew": ["MEMORYVIEW_NEW"],
    "MemoryViewToBytes": ["MEMORYVIEW_TOBYTES"],
    "Buffer2DNew": ["BUFFER2D_NEW"],
    "Buffer2DGet": ["BUFFER2D_GET"],
    "Buffer2DSet": ["BUFFER2D_SET"],
    "Buffer2DMatmul": ["BUFFER2D_MATMUL"],
    "AIter": ["AITER"],
    "ANext": ["ANEXT"],
    "AllocGenerator": ["ASYNCGEN_NEW"],
    "AllocFuture": ["PROMISE_NEW"],
}

DEFAULT_ALLOWED_MISSING: set[str] = set()

P0_REQUIRED = {"CallIndirect", "InvokeFFI", "GuardTag", "GuardDictShape"}

# Lowering lanes that must exist in both backends for current IR mappings.
REQUIRED_BACKEND_KINDS = {
    "call_indirect",
    "invoke_ffi",
    "guard_tag",
    "guard_dict_shape",
    "inc_ref",
    "dec_ref",
    "borrow",
    "release",
    "box",
    "unbox",
    "cast",
    "widen",
    "call_bind",
    "call_func",
    "guard_type",
    "guard_layout",
    "identity_alias",
}

REQUIRED_DIFF_PROBES = (
    "tests/differential/basic/call_indirect_dynamic_callable.py",
    "tests/differential/basic/call_indirect_noncallable_deopt.py",
    "tests/differential/basic/invoke_ffi_os_getcwd.py",
    "tests/differential/basic/invoke_ffi_bridge_capability_enabled.py",
    "tests/differential/basic/invoke_ffi_bridge_capability_denied.py",
    "tests/differential/basic/guard_tag_type_hint_fail.py",
    "tests/differential/basic/guard_dict_shape_mutation.py",
)


@dataclass(frozen=True)
class SemanticAssertion:
    scope: str
    description: str
    pattern: str


FRONTEND_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(
        scope="frontend",
        description="CALL_INDIRECT lowers to dedicated lane",
        pattern=(
            r'elif op\.kind == "CALL_INDIRECT":[\s\S]*?' r'"kind": "call_indirect"'
        ),
    ),
    SemanticAssertion(
        scope="frontend",
        description="INVOKE_FFI lowers to dedicated lane",
        pattern=(r'elif op\.kind == "INVOKE_FFI":[\s\S]*?' r'"kind": "invoke_ffi"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="GUARD_TAG lowers to dedicated lane",
        pattern=(r'elif op\.kind == "GUARD_TAG":[\s\S]*?"kind": "guard_tag"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="GUARD_DICT_SHAPE lowers to dedicated lane",
        pattern=(
            r'elif op\.kind == "GUARD_DICT_SHAPE":[\s\S]*?'
            r'"kind": "guard_dict_shape"'
        ),
    ),
    SemanticAssertion(
        scope="frontend",
        description="INC_REF lowers to dedicated lane",
        pattern=(r'elif op\.kind == "INC_REF":[\s\S]*?"kind": "inc_ref"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="DEC_REF lowers to dedicated lane",
        pattern=(r'elif op\.kind == "DEC_REF":[\s\S]*?"kind": "dec_ref"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="BORROW lowers to dedicated lane",
        pattern=(r'elif op\.kind == "BORROW":[\s\S]*?"kind": "borrow"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="RELEASE lowers to dedicated lane",
        pattern=(r'elif op\.kind == "RELEASE":[\s\S]*?"kind": "release"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="conversion ops preserve dedicated lowering map",
        pattern=(
            r'"BOX": "box",[\s\S]*?"UNBOX": "unbox",[\s\S]*?'
            r'"CAST": "cast",[\s\S]*?"WIDEN": "widen"'
        ),
    ),
)

NATIVE_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(
        scope="native",
        description="call_func keeps dedicated native lane",
        pattern=(r'"call_func"\s*=>[\s\S]*?' r'let call_site_prefix = "call_func";'),
    ),
    SemanticAssertion(
        scope="native",
        description="invoke_ffi uses dedicated invoke_ffi_ic bridge/deopt lane",
        pattern=(
            r'"invoke_ffi"\s*=>[\s\S]*?'
            r'"invoke_ffi_bridge"[\s\S]*?"invoke_ffi_deopt"[\s\S]*?'
            r"box_bool\(if bridge_lane \{ 1 \} else \{ 0 \}\)[\s\S]*?"
            r'declare_function\("molt_invoke_ffi_ic"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="call_bind/call_indirect keep distinct call-site labels + dedicated imports",
        pattern=(
            r'"call_bind"\s*\|\s*"call_indirect"\s*=>[\s\S]*?'
            r'"molt_call_indirect_ic"[\s\S]*?"molt_call_bind_ic"[\s\S]*?'
            r'if op\.kind == "call_indirect"[\s\S]*?'
            r'"call_indirect"[\s\S]*?"call_bind"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="guard_tag uses molt_guard_type runtime guard",
        pattern=(
            r'"guard_type"\s*\|\s*"guard_tag"\s*=>[\s\S]*?'
            r'declare_function\("molt_guard_type"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="guard_dict_shape uses molt_guard_layout_ptr runtime guard",
        pattern=(
            r'"guard_layout"\s*\|\s*"guard_dict_shape"\s*=>[\s\S]*?'
            r'declare_function\("molt_guard_layout_ptr"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="inc_ref/borrow call local_inc_ref_obj",
        pattern=(
            r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?'
            r"call\(local_inc_ref_obj,\s*&\[src\]\)"
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="dec_ref/release call local_dec_ref_obj and write None on out",
        pattern=(
            r'"dec_ref"\s*\|\s*"release"\s*=>[\s\S]*?'
            r"call\(local_dec_ref_obj,\s*&\[src\]\)[\s\S]*?box_none\(\)"
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="box/unbox/cast/widen stay explicit conversion lanes",
        pattern=(r'"box"\s*\|\s*"unbox"\s*\|\s*"cast"\s*\|\s*"widen"\s*=>'),
    ),
)

WASM_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(
        scope="wasm",
        description="dec_ref_obj import is registered",
        pattern=r'add_import\("dec_ref_obj",\s*1,\s*&mut self\.import_ids\);',
    ),
    SemanticAssertion(
        scope="wasm",
        description="call_func/invoke_ffi keep dedicated labels and invoke_ffi import",
        pattern=(
            r'"call_func"\s*\|\s*"invoke_ffi"\s*=>[\s\S]*?'
            r'"invoke_ffi_bridge"[\s\S]*?"invoke_ffi_deopt"[\s\S]*?"call_func"[\s\S]*?'
            r'import_ids\["invoke_ffi_ic"\]'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="call_bind/call_indirect keep distinct labels and dedicated import lanes",
        pattern=(
            r'"call_bind"\s*\|\s*"call_indirect"\s*=>[\s\S]*?'
            r'if op\.kind == "call_indirect"[\s\S]*?"call_indirect"[\s\S]*?"call_bind"'
            r'[\s\S]*?import_ids\["call_indirect_ic"\][\s\S]*?import_ids\["call_bind_ic"\]'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="guard_tag lowering remains explicit",
        pattern=r'"guard_type"\s*\|\s*"guard_tag"\s*=>',
    ),
    SemanticAssertion(
        scope="wasm",
        description="guard_dict_shape lowering remains explicit",
        pattern=r'"guard_layout"\s*\|\s*"guard_dict_shape"\s*=>',
    ),
    SemanticAssertion(
        scope="wasm",
        description="inc_ref/borrow call inc_ref_obj import",
        pattern=(
            r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?' r'import_ids\["inc_ref_obj"\]'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="dec_ref/release call dec_ref_obj import and write None on out",
        pattern=(
            r'"dec_ref"\s*\|\s*"release"\s*=>[\s\S]*?'
            r'import_ids\["dec_ref_obj"\][\s\S]*?box_none\(\)'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="box/unbox/cast/widen stay explicit conversion lanes",
        pattern=(r'"box"\s*\|\s*"unbox"\s*\|\s*"cast"\s*\|\s*"widen"\s*=>'),
    ),
)


def _camel_to_upper_snake(name: str) -> str:
    out: list[str] = []
    for i, ch in enumerate(name):
        if i and ch.isupper():
            prev = name[i - 1]
            nxt = name[i + 1] if i + 1 < len(name) else ""
            if prev.islower() or (nxt and nxt.islower()):
                out.append("_")
        out.append(ch.upper())
    return "".join(out)


def _ordered_unique(items: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        out.append(item)
    return out


def _parse_spec_ops(spec_text: str) -> list[str]:
    marker = "## Instruction categories (minimum set)"
    end = "## Invariants"
    if marker not in spec_text or end not in spec_text:
        raise RuntimeError(
            "Could not locate instruction categories section in IR spec."
        )
    section = spec_text.split(marker, 1)[1].split(end, 1)[0]
    ops: list[str] = []
    for line in section.splitlines():
        if line.strip().startswith("- **"):
            ops.extend(re.findall(r"`([A-Za-z][A-Za-z0-9]*)`", line))
    return _ordered_unique(ops)


def _scan_frontend_emit_kinds(frontend_text: str) -> set[str]:
    return set(re.findall(r'MoltOp\(kind="([A-Za-z0-9_]+)"', frontend_text))


def _scan_frontend_lower_kinds(frontend_text: str) -> set[str]:
    lower_kinds = set(re.findall(r'op\.kind == "([A-Za-z0-9_]+)"', frontend_text))
    for body in re.findall(r"op\.kind in \{([^}]*)\}", frontend_text, flags=re.S):
        lower_kinds.update(re.findall(r"['\"]([A-Za-z0-9_]+)['\"]", body))
    return lower_kinds


def _scan_backend_kinds(backend_text: str) -> set[str]:
    kinds: set[str] = set()
    for pattern in re.finditer(r'((?:"[a-z0-9_]+"(?:\s*\|\s*)?)+)\s*=>', backend_text):
        kinds.update(re.findall(r'"([a-z0-9_]+)"', pattern.group(1)))
    return kinds


def _candidate_kinds(spec_op: str) -> list[str]:
    candidates = [_camel_to_upper_snake(spec_op)]
    candidates.extend(ALIASES.get(spec_op, ()))
    return _ordered_unique(candidates)


def _parse_allow_missing(raw: str | None) -> set[str]:
    if raw is None:
        return set(DEFAULT_ALLOWED_MISSING)
    parts = [part.strip() for part in raw.split(",")]
    return {part for part in parts if part}


def check_semantic_assertions(
    frontend_text: str, native_backend_text: str, wasm_backend_text: str
) -> list[str]:
    failures: list[str] = []
    checks: list[tuple[str, SemanticAssertion]] = []
    checks.extend(("frontend", assertion) for assertion in FRONTEND_SEMANTIC_ASSERTIONS)
    checks.extend(("native", assertion) for assertion in NATIVE_SEMANTIC_ASSERTIONS)
    checks.extend(("wasm", assertion) for assertion in WASM_SEMANTIC_ASSERTIONS)
    text_by_scope = {
        "frontend": frontend_text,
        "native": native_backend_text,
        "wasm": wasm_backend_text,
    }
    for scope, assertion in checks:
        text = text_by_scope[scope]
        if re.search(assertion.pattern, text, flags=re.S) is None:
            failures.append(f"[{assertion.scope}] {assertion.description}")
    return failures


def check_required_diff_probes(
    root: Path = ROOT, required_probes: tuple[str, ...] = REQUIRED_DIFF_PROBES
) -> list[str]:
    missing: list[str] = []
    for rel_path in required_probes:
        if not (root / rel_path).exists():
            missing.append(rel_path)
    return missing


def _normalize_probe_path(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _default_diff_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    return ROOT


def _load_rss_metrics(path: Path) -> list[dict]:
    entries: list[dict] = []
    if not path.exists():
        return entries
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            entries.append(payload)
    return entries


def _resolve_probe_run_id(
    entries: list[dict], required_probes: tuple[str, ...]
) -> str | None:
    required = {_normalize_probe_path(path) for path in required_probes}
    latest_ts = float("-inf")
    latest_run_id: str | None = None
    for payload in entries:
        run_id = payload.get("run_id")
        file_path = payload.get("file")
        if not isinstance(run_id, str) or not run_id:
            continue
        if not isinstance(file_path, str):
            continue
        if _normalize_probe_path(file_path) not in required:
            continue
        timestamp = payload.get("timestamp")
        ts = float(timestamp) if isinstance(timestamp, (int, float)) else float("-inf")
        if ts >= latest_ts:
            latest_ts = ts
            latest_run_id = run_id
    return latest_run_id


def check_required_probe_execution(
    required_probes: tuple[str, ...],
    *,
    rss_metrics_path: Path,
    run_id: str | None = None,
) -> tuple[list[str], str | None]:
    entries = _load_rss_metrics(rss_metrics_path)
    if not entries:
        return [f"missing or empty RSS metrics file: {rss_metrics_path}"], None
    resolved_run_id = run_id or _resolve_probe_run_id(entries, required_probes)
    if not resolved_run_id:
        return ["no run_id found for required differential probes"], None

    required = {_normalize_probe_path(path) for path in required_probes}
    latest_by_probe: dict[str, dict] = {}
    for payload in entries:
        if payload.get("run_id") != resolved_run_id:
            continue
        file_path = payload.get("file")
        if not isinstance(file_path, str):
            continue
        normalized = _normalize_probe_path(file_path)
        if normalized not in required:
            continue
        current = latest_by_probe.get(normalized)
        current_ts = (
            float(current.get("timestamp"))
            if isinstance(current, dict)
            and isinstance(current.get("timestamp"), (int, float))
            else float("-inf")
        )
        new_ts = (
            float(payload.get("timestamp"))
            if isinstance(payload.get("timestamp"), (int, float))
            else float("-inf")
        )
        if new_ts >= current_ts:
            latest_by_probe[normalized] = payload

    failures: list[str] = []
    for probe in sorted(required):
        payload = latest_by_probe.get(probe)
        if payload is None:
            failures.append(f"{probe}: not executed in run_id={resolved_run_id}")
            continue
        status = payload.get("status")
        if status != "ok":
            failures.append(f"{probe}: status={status!r} in run_id={resolved_run_id}")
    return failures, resolved_run_id


def check_failure_queue_linkage(
    required_probes: tuple[str, ...], *, failure_queue_path: Path
) -> list[str]:
    if not failure_queue_path.exists():
        return [f"missing failure queue file: {failure_queue_path}"]
    required = {_normalize_probe_path(path) for path in required_probes}
    raw_lines = failure_queue_path.read_text(encoding="utf-8").splitlines()
    queue_entries: set[str] = set()
    for line in raw_lines:
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        queue_entries.add(_normalize_probe_path(text.split()[0]))
    linked_failures = sorted(required & queue_entries)
    return linked_failures


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify 0100_MOLT_IR inventory coverage against frontend emit/lowering op kinds."
    )
    parser.add_argument(
        "--allow-missing",
        help=("Comma-separated spec op names allowed to be missing. Defaults to none."),
    )
    parser.add_argument(
        "--require-probe-execution",
        action="store_true",
        help=(
            "Require required differential probes to have status=ok in RSS metrics and "
            "to be absent from the failure queue."
        ),
    )
    parser.add_argument(
        "--probe-rss-metrics",
        help=(
            "Path to rss_metrics.jsonl from differential runs. "
            "Defaults to <MOLT_DIFF_ROOT>/rss_metrics.jsonl."
        ),
    )
    parser.add_argument(
        "--probe-run-id",
        help=(
            "Optional differential run_id to validate for probe execution status. "
            "Defaults to latest run touching required probes."
        ),
    )
    parser.add_argument(
        "--failure-queue",
        help=(
            "Path to differential failure queue file. "
            "Defaults to $MOLT_DIFF_FAILURES or <MOLT_DIFF_ROOT>/failures.txt."
        ),
    )
    args = parser.parse_args()

    spec_text = SPEC_PATH.read_text(encoding="utf-8")
    frontend_text = FRONTEND_PATH.read_text(encoding="utf-8")
    native_backend_text = NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_backend_text = WASM_BACKEND_PATH.read_text(encoding="utf-8")

    spec_ops = _parse_spec_ops(spec_text)
    emit_kinds = _scan_frontend_emit_kinds(frontend_text)
    lower_kinds = _scan_frontend_lower_kinds(frontend_text)
    frontend_kinds = emit_kinds | lower_kinds
    native_backend_kinds = _scan_backend_kinds(native_backend_text)
    wasm_backend_kinds = _scan_backend_kinds(wasm_backend_text)
    semantic_failures = check_semantic_assertions(
        frontend_text=frontend_text,
        native_backend_text=native_backend_text,
        wasm_backend_text=wasm_backend_text,
    )
    semantic_assertions_total = (
        len(FRONTEND_SEMANTIC_ASSERTIONS)
        + len(NATIVE_SEMANTIC_ASSERTIONS)
        + len(WASM_SEMANTIC_ASSERTIONS)
    )
    missing_diff_probes = check_required_diff_probes()
    probe_exec_failures: list[str] = []
    failure_queue_hits: list[str] = []
    probe_run_id: str | None = None
    probe_rss_metrics_path: Path | None = None
    failure_queue_path: Path | None = None
    if args.require_probe_execution:
        diff_root = _default_diff_root()
        probe_rss_metrics_path = (
            Path(args.probe_rss_metrics).expanduser()
            if args.probe_rss_metrics
            else diff_root / "rss_metrics.jsonl"
        )
        failure_queue_default = os.environ.get("MOLT_DIFF_FAILURES", "").strip()
        failure_queue_path = (
            Path(args.failure_queue).expanduser()
            if args.failure_queue
            else (
                Path(failure_queue_default).expanduser()
                if failure_queue_default
                else diff_root / "failures.txt"
            )
        )
        probe_exec_failures, probe_run_id = check_required_probe_execution(
            REQUIRED_DIFF_PROBES,
            rss_metrics_path=probe_rss_metrics_path,
            run_id=args.probe_run_id,
        )
        failure_queue_hits = check_failure_queue_linkage(
            REQUIRED_DIFF_PROBES,
            failure_queue_path=failure_queue_path,
        )
    allow_missing = _parse_allow_missing(args.allow_missing)

    unknown_allow = sorted(allow_missing - set(spec_ops))
    if unknown_allow:
        print("molt_ir_ops gate failed: unknown --allow-missing entries:")
        for name in unknown_allow:
            print(f"  - {name}")
        return 1

    missing: list[str] = []
    missing_lowering: list[str] = []
    for spec_op in spec_ops:
        candidates = _candidate_kinds(spec_op)
        if not any(candidate in frontend_kinds for candidate in candidates):
            missing.append(spec_op)
        if not any(candidate in lower_kinds for candidate in candidates):
            missing_lowering.append(spec_op)

    missing_set = set(missing)
    unexpected_missing = sorted(missing_set - allow_missing)
    p0_missing = sorted(P0_REQUIRED & missing_set)
    recovered = sorted(allow_missing - missing_set)
    missing_lowering_set = set(missing_lowering)
    unexpected_missing_lowering = sorted(missing_lowering_set - allow_missing)
    native_missing_backend_kinds = sorted(REQUIRED_BACKEND_KINDS - native_backend_kinds)
    wasm_missing_backend_kinds = sorted(REQUIRED_BACKEND_KINDS - wasm_backend_kinds)

    print(
        "molt_ir_ops gate summary: "
        f"spec_ops={len(spec_ops)} emit_kinds={len(emit_kinds)} "
        f"lower_kinds={len(lower_kinds)} "
        f"native_backend_kinds={len(native_backend_kinds)} "
        f"wasm_backend_kinds={len(wasm_backend_kinds)} "
        f"semantic_assertions={semantic_assertions_total} "
        f"diff_probes={len(REQUIRED_DIFF_PROBES) - len(missing_diff_probes)}/{len(REQUIRED_DIFF_PROBES)} "
        f"probe_exec={'on' if args.require_probe_execution else 'off'} "
        f"missing={len(missing_set)}"
    )
    if args.require_probe_execution:
        print(
            "probe execution context: "
            f"run_id={probe_run_id!r} rss_metrics={probe_rss_metrics_path} "
            f"failure_queue={failure_queue_path}"
        )
    if missing:
        print("missing ops:")
        for name in sorted(missing):
            print(f"  - {name}")
    if missing_lowering:
        print("ops missing lowering coverage:")
        for name in sorted(missing_lowering):
            print(f"  - {name}")

    if recovered:
        print("note: allowed-missing ops now covered:")
        for name in recovered:
            print(f"  - {name}")

    if p0_missing:
        print("molt_ir_ops gate failed: P0-required IR ops are still missing:")
        for name in p0_missing:
            print(f"  - {name}")
        return 1

    if unexpected_missing:
        print("molt_ir_ops gate failed: unexpected missing IR ops:")
        for name in unexpected_missing:
            print(f"  - {name}")
        print(
            "update frontend lowering/emit coverage or explicitly extend --allow-missing "
            "with linked ROADMAP/STATUS TODOs."
        )
        return 1

    if unexpected_missing_lowering:
        print("molt_ir_ops gate failed: unexpected IR ops missing lowering coverage:")
        for name in unexpected_missing_lowering:
            print(f"  - {name}")
        print(
            "add explicit lowering handling in map_ops_to_json (or documented alias lane) "
            "or extend --allow-missing with linked ROADMAP/STATUS TODOs."
        )
        return 1

    if native_missing_backend_kinds:
        print("molt_ir_ops gate failed: native backend missing required lowered lanes:")
        for kind in native_missing_backend_kinds:
            print(f"  - {kind}")
        return 1

    if wasm_missing_backend_kinds:
        print("molt_ir_ops gate failed: wasm backend missing required lowered lanes:")
        for kind in wasm_missing_backend_kinds:
            print(f"  - {kind}")
        return 1

    if semantic_failures:
        print("molt_ir_ops gate failed: semantic lane assertions did not match:")
        for failure in semantic_failures:
            print(f"  - {failure}")
        return 1

    if missing_diff_probes:
        print("molt_ir_ops gate failed: required differential probes are missing:")
        for rel_path in missing_diff_probes:
            print(f"  - {rel_path}")
        return 1

    if probe_exec_failures:
        print("molt_ir_ops gate failed: required differential probes are not green:")
        for failure in probe_exec_failures:
            print(f"  - {failure}")
        return 1

    if failure_queue_hits:
        print("molt_ir_ops gate failed: required probes still listed in failure queue:")
        for failure in failure_queue_hits:
            print(f"  - {failure}")
        return 1

    print("molt_ir_ops gate: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
