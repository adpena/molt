from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import json
import os
import re
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
SPEC_PATH = ROOT / "docs/spec/areas/compiler/0100_MOLT_IR.md"
FRONTEND_PATH = ROOT / "src/molt/frontend/__init__.py"
NATIVE_BACKEND_PATH = ROOT / "runtime/molt-backend/src/native_backend/function_compiler.rs"
WASM_BACKEND_PATH = ROOT / "runtime/molt-backend/src/wasm.rs"
WASM_IMPORTS_PATH = ROOT / "runtime/molt-backend/src/wasm_imports.rs"

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


@dataclass(frozen=True)
class VerificationFinding:
    verifier: str
    message: str
    function: str | None = None
    pass_name: str | None = None
    artifact: str | None = None
    severity: str = "error"


FRONTEND_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(
        scope="frontend",
        description="CALL_INDIRECT lowers to dedicated lane",
        pattern=(r'elif op\.kind == "CALL_INDIRECT":[\s\S]*?"kind": "call_indirect"'),
    ),
    SemanticAssertion(
        scope="frontend",
        description="INVOKE_FFI lowers to dedicated lane",
        pattern=(r'elif op\.kind == "INVOKE_FFI":[\s\S]*?"kind": "invoke_ffi"'),
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
            r'elif op\.kind == "GUARD_DICT_SHAPE":[\s\S]*?"kind": "guard_dict_shape"'
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
            r'"BOX": "box",[\s\S]*?"UNBOX": "unbox",[\s\S]*?"CAST": "cast",[\s\S]*?"WIDEN": "widen"'
        ),
    ),
)

NATIVE_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(
        scope="native",
        description="call_func keeps dedicated native lane",
        pattern=(r'"call_func"\s*=>'),
    ),
    SemanticAssertion(
        scope="native",
        description="invoke_ffi uses dedicated invoke_ffi_ic bridge/deopt lane",
        pattern=(
            r'"invoke_ffi"\s*=>[\s\S]*?"invoke_ffi_bridge"[\s\S]*?"invoke_ffi_deopt"[\s\S]*?box_bool\(if bridge_lane \{ 1 \} else \{ 0 \}\)[\s\S]*?"molt_invoke_ffi_ic"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="call_bind/call_indirect keep distinct call-site labels + dedicated imports",
        pattern=(
            r'"call_bind"\s*\|\s*"call_indirect"\s*=>[\s\S]*?"molt_call_indirect_ic"[\s\S]*?"molt_call_bind_ic"[\s\S]*?if op\.kind == "call_indirect"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="guard_tag uses molt_guard_type runtime guard",
        pattern=(r'"guard_type"\s*\|\s*"guard_tag"\s*=>[\s\S]*?"molt_guard_type"'),
    ),
    SemanticAssertion(
        scope="native",
        description="guard_dict_shape uses molt_guard_layout_ptr runtime guard",
        pattern=(
            r'"guard_layout"\s*\|\s*"guard_dict_shape"\s*=>[\s\S]*?"molt_guard_layout_ptr"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="inc_ref/borrow call local_inc_ref_obj",
        pattern=(r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?emit_inc_ref_obj\(.*local_inc_ref_obj'),
    ),
    SemanticAssertion(
        scope="native",
        description="dec_ref/release call local_dec_ref_obj and write None on out",
        pattern=(
            r'"dec_ref"\s*\|\s*"release"\s*=>[\s\S]*?local_dec_ref_obj[\s\S]*?box_none\(\)'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="box/unbox/cast/widen stay explicit conversion lanes",
        pattern=(r'"box"\s*\|\s*"unbox"\s*\|\s*"cast"\s*\|\s*"widen"\s*=>'),
    ),
)

WASM_SEMANTIC_ASSERTIONS: tuple[SemanticAssertion, ...] = (
    SemanticAssertion(scope="wasm", description="dec_ref_obj import is registered", pattern=r'"dec_ref_obj"'),
    SemanticAssertion(
        scope="wasm",
        description="call_func/invoke_ffi keep dedicated labels and invoke_ffi import",
        pattern=(
            r'"invoke_ffi"\s*=>[\s\S]*?"invoke_ffi_bridge"[\s\S]*?"invoke_ffi_deopt"[\s\S]*?import_ids\["invoke_ffi_ic"\]'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="call_bind/call_indirect keep distinct labels and dedicated import lanes",
        pattern=(
            r'"call_bind"\s*\|\s*"call_indirect"\s*=>[\s\S]*?if op\.kind == "call_indirect"[\s\S]*?"call_indirect"[\s\S]*?"call_bind"[\s\S]*?import_ids\["call_indirect_ic"\][\s\S]*?import_ids\["call_bind_ic"\]'
        ),
    ),
    SemanticAssertion(scope="wasm", description="guard_tag lowering remains explicit", pattern=r'"guard_type"\s*\|\s*"guard_tag"\s*=>'),
    SemanticAssertion(scope="wasm", description="guard_dict_shape lowering remains explicit", pattern=r'"guard_layout"\s*\|\s*"guard_dict_shape"\s*=>'),
    SemanticAssertion(
        scope="wasm",
        description="inc_ref/borrow call inc_ref_obj import",
        pattern=(r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?import_ids\["inc_ref_obj"\]'),
    ),
    SemanticAssertion(
        scope="wasm",
        description="dec_ref/release call dec_ref_obj import and write None on out",
        pattern=(r'"dec_ref"\s*\|\s*"release"\s*=>[\s\S]*?import_ids\["dec_ref_obj"\]'),
    ),
    SemanticAssertion(
        scope="wasm",
        description="box/unbox/cast/widen stay explicit conversion lanes",
        pattern=(r'"box"\s*\|\s*"unbox"\s*\|\s*"cast"\s*\|\s*"widen"\s*=>'),
    ),
)


def _finding_dict(finding: VerificationFinding) -> dict[str, Any]:
    payload = asdict(finding)
    payload["pass"] = payload.pop("pass_name")
    return payload


def build_verify_result_payload(checks: list[dict[str, Any]]) -> dict[str, Any]:
    normalized: list[dict[str, Any]] = []
    for check in checks:
        findings = check.get("findings", [])
        normalized.append(
            {
                "name": check["name"],
                "status": check["status"],
                "findings": [
                    _finding_dict(finding) if isinstance(finding, VerificationFinding) else dict(finding)
                    for finding in findings
                ],
            }
        )
    return {"checks": normalized}


def _camel_to_upper_snake(name: str) -> str:
    out: list[str] = []
    for index, ch in enumerate(name):
        if index and ch.isupper():
            prev = name[index - 1]
            nxt = name[index + 1] if index + 1 < len(name) else ""
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
        raise RuntimeError("Could not locate instruction categories section in IR spec.")
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
    return {part for part in (item.strip() for item in raw.split(",")) if part}


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
        if re.search(assertion.pattern, text_by_scope[scope], flags=re.S) is None:
            failures.append(f"[{assertion.scope}] {assertion.description}")
    return failures


def check_required_diff_probes(
    root: Path = ROOT, required_probes: tuple[str, ...] = REQUIRED_DIFF_PROBES
) -> list[str]:
    return [rel_path for rel_path in required_probes if not (root / rel_path).exists()]


def _normalize_probe_path(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _default_diff_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    return ROOT


def _load_rss_metrics(path: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    if not path.exists():
        return entries
    for line in path.read_text(encoding="utf-8").splitlines():
        text = line.strip()
        if not text:
            continue
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            entries.append(payload)
    return entries


def _resolve_probe_run_id(
    entries: list[dict[str, Any]], required_probes: tuple[str, ...]
) -> str | None:
    required = {_normalize_probe_path(path) for path in required_probes}
    latest_ts = float("-inf")
    latest_run_id: str | None = None
    for payload in entries:
        run_id = payload.get("run_id")
        file_path = payload.get("file")
        if not isinstance(run_id, str) or not run_id:
            continue
        if not isinstance(file_path, str) or _normalize_probe_path(file_path) not in required:
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
    latest_by_probe: dict[str, dict[str, Any]] = {}
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
            if isinstance(current, dict) and isinstance(current.get("timestamp"), (int, float))
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
    queue_entries: set[str] = set()
    for line in failure_queue_path.read_text(encoding="utf-8").splitlines():
        text = line.strip()
        if not text or text.startswith("#"):
            continue
        queue_entries.add(_normalize_probe_path(text.split()[0]))
    return sorted(required & queue_entries)


def _read_backend_texts() -> tuple[str, str, str, str]:
    spec_text = SPEC_PATH.read_text(encoding="utf-8")
    frontend_text = FRONTEND_PATH.read_text(encoding="utf-8")
    native_backend_text = NATIVE_BACKEND_PATH.read_text(encoding="utf-8")
    wasm_backend_text = WASM_BACKEND_PATH.read_text(encoding="utf-8")
    if WASM_IMPORTS_PATH.exists():
        wasm_backend_text += "\n" + WASM_IMPORTS_PATH.read_text(encoding="utf-8")
    return spec_text, frontend_text, native_backend_text, wasm_backend_text


def _build_findings(verifier: str, messages: list[str], *, artifact: str | None = None) -> list[VerificationFinding]:
    return [
        VerificationFinding(verifier=verifier, severity="error", message=message, artifact=artifact)
        for message in messages
    ]


def run_default_verify_checks(
    *,
    allow_missing: str | None = None,
    require_probe_execution: bool = False,
    probe_rss_metrics: Path | None = None,
    probe_run_id: str | None = None,
    failure_queue: Path | None = None,
) -> tuple[list[dict[str, Any]], list[str]]:
    spec_text, frontend_text, native_backend_text, wasm_backend_text = _read_backend_texts()

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

    allow_missing_set = _parse_allow_missing(allow_missing)
    unknown_allow = sorted(allow_missing_set - set(spec_ops))
    missing: list[str] = []
    missing_lowering: list[str] = []
    for spec_op in spec_ops:
        candidates = _candidate_kinds(spec_op)
        if not any(candidate in frontend_kinds for candidate in candidates):
            missing.append(spec_op)
        if not any(candidate in lower_kinds for candidate in candidates):
            missing_lowering.append(spec_op)

    missing_set = set(missing)
    unexpected_missing = sorted(missing_set - allow_missing_set)
    p0_missing = sorted(P0_REQUIRED & missing_set)
    unexpected_missing_lowering = sorted(set(missing_lowering) - allow_missing_set)
    native_missing_backend_kinds = sorted(REQUIRED_BACKEND_KINDS - native_backend_kinds)
    wasm_missing_backend_kinds = sorted(REQUIRED_BACKEND_KINDS - wasm_backend_kinds)
    missing_diff_probes = check_required_diff_probes()

    checks: list[dict[str, Any]] = []
    errors: list[str] = []

    inventory_messages: list[str] = []
    if unknown_allow:
        inventory_messages.extend(f"unknown --allow-missing entry: {name}" for name in unknown_allow)
    if p0_missing:
        inventory_messages.extend(f"P0-required IR op missing: {name}" for name in p0_missing)
    inventory_messages.extend(f"unexpected missing IR op: {name}" for name in unexpected_missing)
    inventory_messages.extend(
        f"unexpected IR op missing lowering coverage: {name}" for name in unexpected_missing_lowering
    )
    inventory_messages.extend(
        f"native backend missing required lowered lane: {kind}" for kind in native_missing_backend_kinds
    )
    inventory_messages.extend(
        f"wasm backend missing required lowered lane: {kind}" for kind in wasm_missing_backend_kinds
    )
    checks.append(
        {
            "name": "ir-inventory",
            "status": "error" if inventory_messages else "ok",
            "findings": _build_findings("ir-inventory", inventory_messages, artifact=str(SPEC_PATH)),
        }
    )
    errors.extend(inventory_messages)

    checks.append(
        {
            "name": "semantic-assertions",
            "status": "error" if semantic_failures else "ok",
            "findings": _build_findings(
                "semantic-assertions",
                semantic_failures,
                artifact=str(FRONTEND_PATH),
            ),
        }
    )
    errors.extend(semantic_failures)

    checks.append(
        {
            "name": "required-diff-probes",
            "status": "error" if missing_diff_probes else "ok",
            "findings": _build_findings(
                "required-diff-probes",
                [f"missing required probe: {probe}" for probe in missing_diff_probes],
            ),
        }
    )
    errors.extend(f"missing required probe: {probe}" for probe in missing_diff_probes)

    if require_probe_execution:
        diff_root = _default_diff_root()
        probe_rss_metrics_path = probe_rss_metrics or (diff_root / "rss_metrics.jsonl")
        failure_queue_path = failure_queue or (
            Path(os.environ.get("MOLT_DIFF_FAILURES", "")).expanduser()
            if os.environ.get("MOLT_DIFF_FAILURES", "").strip()
            else diff_root / "failures.txt"
        )
        probe_exec_failures, resolved_run_id = check_required_probe_execution(
            REQUIRED_DIFF_PROBES,
            rss_metrics_path=probe_rss_metrics_path,
            run_id=probe_run_id,
        )
        failure_queue_hits = check_failure_queue_linkage(
            REQUIRED_DIFF_PROBES,
            failure_queue_path=failure_queue_path,
        )
        checks.append(
            {
                "name": "required-probe-execution",
                "status": "error" if probe_exec_failures or failure_queue_hits else "ok",
                "findings": _build_findings(
                    "required-probe-execution",
                    probe_exec_failures
                    + [f"required probe still listed in failure queue: {hit}" for hit in failure_queue_hits],
                    artifact=str(probe_rss_metrics_path),
                )
                + (
                    [
                        VerificationFinding(
                            verifier="required-probe-execution",
                            severity="info",
                            message=f"validated run_id={resolved_run_id}",
                            artifact=str(probe_rss_metrics_path),
                        )
                    ]
                    if resolved_run_id is not None
                    else []
                ),
            }
        )
        errors.extend(probe_exec_failures)
        errors.extend(f"required probe still listed in failure queue: {hit}" for hit in failure_queue_hits)

    return checks, errors


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Verify 0100_MOLT_IR inventory coverage against frontend emit/lowering op kinds."
    )
    parser.add_argument("--allow-missing", help="Comma-separated spec op names allowed to be missing.")
    parser.add_argument(
        "--require-probe-execution",
        action="store_true",
        help="Require required differential probes to have status=ok in RSS metrics and to be absent from the failure queue.",
    )
    parser.add_argument("--probe-rss-metrics", help="Path to rss_metrics.jsonl from differential runs.")
    parser.add_argument("--probe-run-id", help="Optional differential run_id to validate for probe execution status.")
    parser.add_argument("--failure-queue", help="Path to differential failure queue file.")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    checks, errors = run_default_verify_checks(
        allow_missing=args.allow_missing,
        require_probe_execution=args.require_probe_execution,
        probe_rss_metrics=Path(args.probe_rss_metrics).expanduser() if args.probe_rss_metrics else None,
        probe_run_id=args.probe_run_id,
        failure_queue=Path(args.failure_queue).expanduser() if args.failure_queue else None,
    )
    spec_text, frontend_text, native_backend_text, wasm_backend_text = _read_backend_texts()
    spec_ops = _parse_spec_ops(spec_text)
    emit_kinds = _scan_frontend_emit_kinds(frontend_text)
    lower_kinds = _scan_frontend_lower_kinds(frontend_text)
    native_backend_kinds = _scan_backend_kinds(native_backend_text)
    wasm_backend_kinds = _scan_backend_kinds(wasm_backend_text)
    semantic_assertions_total = (
        len(FRONTEND_SEMANTIC_ASSERTIONS)
        + len(NATIVE_SEMANTIC_ASSERTIONS)
        + len(WASM_SEMANTIC_ASSERTIONS)
    )
    missing_diff_probes = check_required_diff_probes()
    print(
        "molt_ir_ops gate summary: "
        f"spec_ops={len(spec_ops)} emit_kinds={len(emit_kinds)} "
        f"lower_kinds={len(lower_kinds)} "
        f"native_backend_kinds={len(native_backend_kinds)} "
        f"wasm_backend_kinds={len(wasm_backend_kinds)} "
        f"semantic_assertions={semantic_assertions_total} "
        f"diff_probes={len(REQUIRED_DIFF_PROBES) - len(missing_diff_probes)}/{len(REQUIRED_DIFF_PROBES)} "
        f"probe_exec={'on' if args.require_probe_execution else 'off'} "
        f"missing={sum(len(check['findings']) for check in checks if check['status'] == 'error')}"
    )
    if errors:
        print("molt_ir_ops gate failed:")
        for error in errors:
            print(f"  - {error}")
        return 1
    print("molt_ir_ops gate: ok")
    return 0
