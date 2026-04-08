from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
from typing import Any, Callable


@dataclass(frozen=True)
class ReductionInput:
    input_kind: str
    source_path: Path
    source_text: str
    manifest_path: Path | None = None
    original_manifest: dict[str, Any] | None = None


@dataclass(frozen=True)
class ReductionResult:
    reduction_input: ReductionInput
    oracle: dict[str, Any]
    original_source: str
    reduced_source: str
    preserved_failure: bool
    failure_signature: str
    evaluation: dict[str, Any]


def _sorted_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _sha256_dict(value: Any) -> str:
    return hashlib.sha256(_sorted_json(value).encode("utf-8")).hexdigest()


def _normalize_predicates(predicates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        [dict(predicate) for predicate in predicates],
        key=lambda predicate: (
            str(predicate.get("path", "")),
            str(predicate.get("op", "")),
            _sorted_json(predicate.get("value")),
        ),
    )


def normalize_failure_oracle(oracle: str | dict[str, Any]) -> dict[str, Any]:
    if isinstance(oracle, str):
        if oracle == "exit:nonzero":
            return {
                "kind": "process_exit",
                "match": {"classification": "nonzero_exit"},
                "reason_code": "process_exit",
                "schema_version": 1,
            }
        raise ValueError(f"unsupported oracle string: {oracle}")

    kind = oracle.get("kind")
    if kind == "verify":
        return {
            "kind": "verifier_failure",
            "match": {
                "message_contains": oracle["message_contains"],
                "severity": oracle.get("severity", "error"),
                "verifier": oracle["verifier"],
            },
            "reason_code": "verifier_failure",
            "schema_version": 1,
        }
    if kind == "diff":
        return {
            "kind": "structured_diff_mismatch",
            "match": {
                "field": oracle.get("field", "stderr"),
                "mismatch_class": oracle["mismatch_class"],
            },
            "reason_code": "structured_diff_mismatch",
            "schema_version": 1,
        }
    if kind == "trace":
        return {
            "kind": "trace_signature",
            "match": {
                "event": oracle.get("event", "pass"),
                "match_mode": oracle.get("match_mode", "contains"),
                "signature": oracle["signature"],
            },
            "reason_code": "trace_signature",
            "schema_version": 1,
        }
    if kind == "manifest":
        return {
            "kind": "manifest_predicate",
            "match": {"predicates": _normalize_predicates(list(oracle["predicates"]))},
            "reason_code": "manifest_predicate",
            "schema_version": 1,
        }
    if kind in {
        "process_exit",
        "verifier_failure",
        "structured_diff_mismatch",
        "trace_signature",
        "manifest_predicate",
    }:
        normalized = dict(oracle)
        normalized.setdefault("schema_version", 1)
        normalized.setdefault("reason_code", str(kind))
        return normalized
    raise ValueError(f"unsupported oracle kind: {kind}")


def load_reduction_input(path: Path) -> ReductionInput:
    if path.suffix == ".json":
        payload = json.loads(path.read_text(encoding="utf-8"))
        data = payload.get("data", {})
        source_path = Path(data["source_path"])
        return ReductionInput(
            input_kind="manifest",
            source_path=source_path,
            source_text=str(data["source_text"]),
            manifest_path=path,
            original_manifest=payload,
        )
    return ReductionInput(
        input_kind="source",
        source_path=path,
        source_text=path.read_text(encoding="utf-8"),
    )


def _manifest_value_at_path(manifest: dict[str, Any], path: str) -> Any:
    current: Any = manifest
    for part in path.split("."):
        if not isinstance(current, dict) or part not in current:
            return None
        current = current[part]
    return current


def _oracle_matches(oracle: dict[str, Any], evaluation: dict[str, Any]) -> bool:
    if "matched" in evaluation:
        return bool(evaluation["matched"])

    kind = oracle["kind"]
    match = oracle["match"]
    if kind == "process_exit":
        return evaluation.get("classification") == match["classification"]
    if kind == "verifier_failure":
        findings = evaluation.get("findings", [])
        for finding in findings:
            if (
                finding.get("verifier") == match["verifier"]
                and match["message_contains"] in finding.get("message", "")
                and finding.get("severity", "error") == match["severity"]
            ):
                return True
        return False
    if kind == "structured_diff_mismatch":
        diff = evaluation.get("diff", {})
        return (
            diff.get("mismatch_class") == match["mismatch_class"]
            and diff.get("field") == match["field"]
        )
    if kind == "trace_signature":
        events = evaluation.get("trace_events", [])
        for event in events:
            if event.get("event") != match["event"]:
                continue
            signature = str(event.get("signature", ""))
            if match["match_mode"] == "exact" and signature == match["signature"]:
                return True
            if match["match_mode"] != "exact" and match["signature"] in signature:
                return True
        return False
    if kind == "manifest_predicate":
        manifest = evaluation.get("manifest", {})
        for predicate in match["predicates"]:
            value = _manifest_value_at_path(manifest, predicate["path"])
            op = predicate["op"]
            if op == "exists":
                if value is None:
                    return False
            elif op == "equals":
                if value != predicate["value"]:
                    return False
            else:
                raise ValueError(f"unsupported manifest predicate op: {op}")
        return True
    raise ValueError(f"unsupported oracle kind: {kind}")


def _reduce_lines(
    lines: list[str],
    *,
    oracle: dict[str, Any],
    evaluator: Callable[[str], dict[str, Any]],
) -> tuple[list[str], dict[str, Any]]:
    current_lines = list(lines)
    current_eval = evaluator("".join(current_lines))
    if not _oracle_matches(oracle, current_eval):
        raise ValueError("initial source does not satisfy the oracle")

    changed = True
    while changed and len(current_lines) > 1:
        changed = False
        for index in range(len(current_lines)):
            candidate_lines = current_lines[:index] + current_lines[index + 1 :]
            candidate_text = "".join(candidate_lines)
            candidate_eval = evaluator(candidate_text)
            if _oracle_matches(oracle, candidate_eval):
                current_lines = candidate_lines
                current_eval = candidate_eval
                changed = True
                break
    return current_lines, current_eval


def reduce_source_text(
    reduction_input: ReductionInput,
    *,
    oracle: dict[str, Any],
    evaluator: Callable[[str], dict[str, Any]],
) -> ReductionResult:
    normalized_oracle = normalize_failure_oracle(oracle)
    original_source = reduction_input.source_text
    reduced_lines, final_eval = _reduce_lines(
        original_source.splitlines(keepends=True),
        oracle=normalized_oracle,
        evaluator=evaluator,
    )
    reduced_source = "".join(reduced_lines).rstrip("\n")
    signature = _sha256_dict(
        {
            "oracle": normalized_oracle,
            "evaluation": final_eval,
            "reduced_source": reduced_source,
        }
    )
    return ReductionResult(
        reduction_input=reduction_input,
        oracle=normalized_oracle,
        original_source=original_source,
        reduced_source=reduced_source,
        preserved_failure=True,
        failure_signature=signature,
        evaluation=final_eval,
    )


def build_reduction_payload(result: ReductionResult) -> dict[str, Any]:
    source_path = result.reduction_input.source_path
    reduced_name = f"{source_path.stem}_reduced{source_path.suffix or '.py'}"
    reduced_path = source_path.with_name(reduced_name)
    original_lines = len(result.original_source.split("\n")) if result.original_source else 0
    reduced_lines = len(result.reduced_source.split("\n")) if result.reduced_source else 0
    return {
        "input": {
            "kind": result.reduction_input.input_kind,
            "source_path": str(result.reduction_input.source_path),
            "manifest_path": (
                str(result.reduction_input.manifest_path)
                if result.reduction_input.manifest_path is not None
                else None
            ),
        },
        "oracle": result.oracle,
        "reduction": {
            "strategy": "ddmin-lines",
            "original_bytes": len(result.original_source),
            "original_lines": original_lines,
            "reduced_bytes": len(result.reduced_source),
            "reduced_lines": reduced_lines,
            "preserved_failure": result.preserved_failure,
        },
        "artifacts": {
            "reduced_source": str(reduced_path),
        },
        "promotion": {
            "category": "differential_regression",
            "recommended_path": f"tests/differential/debug/{reduced_path.name}",
        },
        "failure_signature": result.failure_signature,
    }
