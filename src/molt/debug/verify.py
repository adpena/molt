from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import asdict, dataclass
import json
import os
import re
from pathlib import Path
from types import MappingProxyType
from typing import Any

from molt.source_root import compiler_source_root

_IR_SPEC = "docs/spec/areas/compiler/0100_MOLT_IR.md"

# Writable differential evidence retains its existing output authority.
_DEFAULT_DIFF_ROOT = Path(__file__).resolve().parents[3]

# Semantic spec names that differ from their registered wire operations.
# Frontend spelling changes are projected from the op-kind registry instead.
# The inventory check rejects absent spec ops, unregistered kinds and aliases
# that repeat the registered default.
SPEC_OP_KIND_ALIASES: Mapping[str, tuple[str, ...]] = MappingProxyType(
    {
        "Branch": ("IF", "ELSE", "END_IF"),
        "Return": ("ret",),
        "Throw": ("RAISE",),
        "LoadIndex": ("INDEX",),
        "AIter": ("AITER",),
        "ANext": ("ANEXT",),
        "AllocGenerator": ("ASYNCGEN_NEW",),
    }
)

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
    "binding_alias",
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


# Each frontend op kind and the backend lane its serialization must emit.
# Checked by serializing the op, so a refactor that keeps the lowering passes.
FRONTEND_LOWERING_LANES: Mapping[str, str] = MappingProxyType(
    {
        "CALL_INDIRECT": "call_indirect",
        "INVOKE_FFI": "invoke_ffi",
        "GUARD_TAG": "guard_tag",
        "GUARD_DICT_SHAPE": "guard_dict_shape",
        "INC_REF": "inc_ref",
        "DEC_REF": "dec_ref",
        # Ownership transfers lower through the canonical refcount lanes.
        "BORROW": "inc_ref",
        "RELEASE": "dec_ref",
        # Conversions keep their dedicated lanes.
        "BOX": "box",
        "UNBOX": "unbox",
        "CAST": "cast",
        "WIDEN": "widen",
    }
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
        description="guard_dict_shape uses molt_guard_layout runtime guard",
        pattern=(
            r'"guard_layout"\s*\|\s*"guard_dict_shape"\s*=>[\s\S]*?"molt_guard_layout"'
        ),
    ),
    SemanticAssertion(
        scope="native",
        description="inc_ref/borrow call local_inc_ref_obj",
        pattern=(
            r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?emit_inc_ref_obj\(.*local_inc_ref_obj'
        ),
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
    SemanticAssertion(
        scope="wasm",
        description="dec_ref_obj import is registered",
        pattern=r'"dec_ref_obj"',
    ),
    SemanticAssertion(
        scope="wasm",
        description="call_func/invoke_ffi keep dedicated labels and invoke_ffi import",
        pattern=(
            r'"invoke_ffi"\s*=>[\s\S]*?"invoke_ffi_bridge"[\s\S]*?"invoke_ffi_deopt"[\s\S]*?WasmRuntimeImport::InvokeFfiIc'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="call_bind/call_indirect keep distinct labels and dedicated import lanes",
        pattern=(
            r'"call_bind"\s*\|\s*"call_indirect"\s*=>[\s\S]*?if op\.kind == "call_indirect"[\s\S]*?"call_indirect"[\s\S]*?"call_bind"[\s\S]*?WasmRuntimeImport::CallIndirectIc[\s\S]*?WasmRuntimeImport::CallBindIc'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="guard_tag lowering remains explicit",
        pattern=r'"guard_tag"\s*=>\s*Some\([\s\S]*?WasmRuntimeImport::GuardType',
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
            r'"inc_ref"\s*\|\s*"borrow"\s*=>[\s\S]*?emit_inc_ref_like[\s\S]*?WasmRuntimeImport::IncRefObj'
        ),
    ),
    SemanticAssertion(
        scope="wasm",
        description="dec_ref/release/del_boundary call dec_ref_obj import and write None on out",
        pattern=(
            r'"dec_ref"\s*\|\s*"release"\s*\|\s*"del_boundary"\s*=>[\s\S]*?emit_dec_ref_like[\s\S]*?WasmRuntimeImport::DecRefObj[\s\S]*?emit_none'
        ),
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
                    _finding_dict(finding)
                    if isinstance(finding, VerificationFinding)
                    else dict(finding)
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


_SPEC_SECTION = "## Instruction categories (minimum set)"
_SPEC_SECTION_END = "## Invariants"
_SPEC_OP_NAME = re.compile(r"`([A-Za-z][A-Za-z0-9]*)`")
_SENTENCE_END = re.compile(r"\.(?:\s|$)")


def _without_parentheticals(text: str) -> str:
    kept: list[str] = []
    depth = 0
    for ch in text:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif depth == 0:
            kept.append(ch)
    return "".join(kept)


def _parse_spec_ops(spec_text: str) -> list[str]:
    """Return the op names each category bullet lists, in spec order.

    A category is a ``- **Name**:`` bullet and its indented continuation
    lines. Its op list is the first sentence after the colon, read without
    parenthetical remarks; nested bullets and later sentences are prose.
    """

    if _SPEC_SECTION not in spec_text or _SPEC_SECTION_END not in spec_text:
        raise RuntimeError(
            "Could not locate instruction categories section in IR spec."
        )
    section = spec_text.split(_SPEC_SECTION, 1)[1].split(_SPEC_SECTION_END, 1)[0]
    categories: list[list[str]] = []
    current: list[str] | None = None
    for line in section.splitlines():
        if line.startswith("- **"):
            current = [line]
            categories.append(current)
        elif (
            current is not None
            and line.startswith("  ")
            and not line.lstrip().startswith("- ")
        ):
            current.append(line.strip())
        else:
            current = None
    ops: list[str] = []
    for category in categories:
        listing = " ".join(category).split("**:", 1)[1]
        first_sentence = _SENTENCE_END.split(_without_parentheticals(listing), 1)[0]
        ops.extend(_SPEC_OP_NAME.findall(first_sentence))
    return _ordered_unique(ops)


def _scan_backend_kinds(backend_text: str) -> set[str]:
    kinds: set[str] = set()
    for pattern in re.finditer(r'((?:"[a-z0-9_]+"(?:\s*\|\s*)?)+)\s*=>', backend_text):
        kinds.update(re.findall(r'"([a-z0-9_]+)"', pattern.group(1)))
    return kinds


def _candidate_kinds(spec_op: str) -> tuple[str, ...]:
    from molt.frontend.lowering.op_kinds_generated import (
        FRONTEND_LOWERING_KINDS_BY_WIRE,
    )

    spelling = _camel_to_upper_snake(spec_op)
    return FRONTEND_LOWERING_KINDS_BY_WIRE.get(spelling.lower(), (spelling,))


def check_ir_inventory(
    spec_ops: list[str], registered_kinds: frozenset[str]
) -> list[str]:
    """Each spec op names a registered frontend kind; aliases stay minimal."""

    failures: list[str] = []
    for spec_op, kinds in SPEC_OP_KIND_ALIASES.items():
        if spec_op not in spec_ops:
            failures.append(f"alias names no spec op: {spec_op}")
        if all(kind in registered_kinds for kind in _candidate_kinds(spec_op)):
            failures.append(
                f"alias for {spec_op} repeats its registered default spelling"
            )
        failures.extend(
            f"alias for {spec_op} names an unregistered kind: {kind}"
            for kind in kinds
            if kind not in registered_kinds
        )
    for spec_op in spec_ops:
        if spec_op in SPEC_OP_KIND_ALIASES:
            continue
        failures.extend(
            f"IR op has no registered frontend kind: {spec_op} ({kind})"
            for kind in _candidate_kinds(spec_op)
            if kind not in registered_kinds
        )
    return failures


def _serialized_frontend_kinds(kind: str) -> list[str]:
    from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator

    op = MoltOp(
        kind=kind, args=[MoltValue("v0"), MoltValue("v1")], result=MoltValue("v2")
    )
    emitted = SimpleTIRGenerator().map_ops_to_json(
        [op], function_name="molt_debug_verify", run_midend=False
    )
    return [str(item["kind"]) for item in emitted if item["kind"] != "ret_void"]


def check_frontend_lowering_lanes(
    serialize: Callable[[str], list[str]] = _serialized_frontend_kinds,
) -> list[str]:
    """Serialize each op kind and compare the emitted lane with its contract."""

    failures: list[str] = []
    for kind, lane in FRONTEND_LOWERING_LANES.items():
        try:
            emitted = serialize(kind)
        except Exception as exc:  # the report names any serializer failure
            failures.append(
                f"[frontend] {kind} must lower to {lane}; serialization failed: "
                f"{type(exc).__name__}: {exc}"
            )
            continue
        if emitted != [lane]:
            failures.append(f"[frontend] {kind} must lower to {lane}, got {emitted}")
    return failures


def check_semantic_assertions(
    native_backend_text: str, wasm_backend_text: str
) -> list[str]:
    failures: list[str] = []
    checks: list[tuple[str, SemanticAssertion]] = []
    checks.extend(("native", assertion) for assertion in NATIVE_SEMANTIC_ASSERTIONS)
    checks.extend(("wasm", assertion) for assertion in WASM_SEMANTIC_ASSERTIONS)
    text_by_scope = {
        "native": native_backend_text,
        "wasm": wasm_backend_text,
    }
    for scope, assertion in checks:
        if re.search(assertion.pattern, text_by_scope[scope], flags=re.S) is None:
            failures.append(f"[{assertion.scope}] {assertion.description}")
    return failures


def check_required_diff_probes(
    root: Path | None = None, required_probes: tuple[str, ...] = REQUIRED_DIFF_PROBES
) -> list[str]:
    root = root if root is not None else compiler_source_root()
    return [rel_path for rel_path in required_probes if not (root / rel_path).exists()]


def _normalize_probe_path(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def _default_diff_root() -> Path:
    raw = os.environ.get("MOLT_DIFF_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    return _DEFAULT_DIFF_ROOT


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
        if (
            not isinstance(file_path, str)
            or _normalize_probe_path(file_path) not in required
        ):
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
        current_timestamp = (
            current.get("timestamp") if isinstance(current, dict) else None
        )
        payload_timestamp = payload.get("timestamp")
        current_ts = (
            float(current_timestamp)
            if isinstance(current_timestamp, (int, float))
            else float("-inf")
        )
        new_ts = (
            float(payload_timestamp)
            if isinstance(payload_timestamp, (int, float))
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


def _read_production_source_tree(root: Path, suffix: str) -> str:
    sources = sorted(
        path
        for path in root.rglob(f"*{suffix}")
        if "tests" not in path.relative_to(root).parts
    )
    if not sources:
        raise FileNotFoundError(f"no {suffix} production sources under {root}")
    return "\n".join(path.read_text(encoding="utf-8") for path in sources)


def _read_backend_texts() -> tuple[str, str, str]:
    root = compiler_source_root()
    spec_text = (root / _IR_SPEC).read_text(encoding="utf-8")
    native_backend_text = _read_production_source_tree(
        root / "runtime/molt-backend-native/src/native_backend", ".rs"
    )
    wasm_backend_text = _read_production_source_tree(
        root / "runtime/molt-backend-wasm/src", ".rs"
    )
    return spec_text, native_backend_text, wasm_backend_text


def _build_findings(
    verifier: str, messages: list[str], *, artifact: str | None = None
) -> list[VerificationFinding]:
    return [
        VerificationFinding(
            verifier=verifier, severity="error", message=message, artifact=artifact
        )
        for message in messages
    ]


def run_default_verify_checks(
    *,
    require_probe_execution: bool = False,
    probe_rss_metrics: Path | None = None,
    probe_run_id: str | None = None,
    failure_queue: Path | None = None,
) -> tuple[list[dict[str, Any]], list[str]]:
    from molt.frontend.lowering.op_kinds_generated import FRONTEND_REGISTERED_KINDS

    spec_text, native_backend_text, wasm_backend_text = _read_backend_texts()
    native_backend_kinds = _scan_backend_kinds(native_backend_text)
    wasm_backend_kinds = _scan_backend_kinds(wasm_backend_text)
    semantic_failures = check_frontend_lowering_lanes() + check_semantic_assertions(
        native_backend_text=native_backend_text,
        wasm_backend_text=wasm_backend_text,
    )
    missing_diff_probes = check_required_diff_probes()

    checks: list[dict[str, Any]] = []
    errors: list[str] = []

    inventory_messages = check_ir_inventory(
        _parse_spec_ops(spec_text), FRONTEND_REGISTERED_KINDS
    )
    inventory_messages.extend(
        f"native backend missing required lowered lane: {kind}"
        for kind in sorted(REQUIRED_BACKEND_KINDS - native_backend_kinds)
    )
    inventory_messages.extend(
        f"wasm backend missing required lowered lane: {kind}"
        for kind in sorted(REQUIRED_BACKEND_KINDS - wasm_backend_kinds)
    )
    checks.append(
        {
            "name": "ir-inventory",
            "status": "error" if inventory_messages else "ok",
            "findings": _build_findings(
                "ir-inventory",
                inventory_messages,
                artifact=str(compiler_source_root() / _IR_SPEC),
            ),
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
                artifact=str(compiler_source_root() / "src/molt/frontend"),
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
                "status": "error"
                if probe_exec_failures or failure_queue_hits
                else "ok",
                "findings": _build_findings(
                    "required-probe-execution",
                    probe_exec_failures
                    + [
                        f"required probe still listed in failure queue: {hit}"
                        for hit in failure_queue_hits
                    ],
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
        errors.extend(
            f"required probe still listed in failure queue: {hit}"
            for hit in failure_queue_hits
        )

    return checks, errors
