from __future__ import annotations

import json
from pathlib import Path

from molt.debug.bisect import (
    ProbeSupervisorAttemptConfig,
    bisect_backend_profile_ic,
    bisect_first_bad_pass,
)
from molt.debug.reduce import (
    build_reduction_payload,
    load_reduction_input,
    normalize_failure_oracle,
    reduce_source_text,
)


def test_normalize_failure_oracle_canonicalizes_required_categories() -> None:
    exit_oracle = normalize_failure_oracle("exit:nonzero")
    assert exit_oracle == {
        "kind": "process_exit",
        "match": {"classification": "nonzero_exit"},
        "reason_code": "process_exit",
        "schema_version": 1,
    }

    verifier_oracle = normalize_failure_oracle(
        {
            "kind": "verify",
            "verifier": "ir-contract",
            "message_contains": "guard_dict_shape",
        }
    )
    assert verifier_oracle == {
        "kind": "verifier_failure",
        "match": {
            "message_contains": "guard_dict_shape",
            "severity": "error",
            "verifier": "ir-contract",
        },
        "reason_code": "verifier_failure",
        "schema_version": 1,
    }

    diff_oracle = normalize_failure_oracle(
        {
            "kind": "diff",
            "mismatch_class": "stdout_mismatch",
            "field": "stdout",
        }
    )
    assert diff_oracle == {
        "kind": "structured_diff_mismatch",
        "match": {
            "field": "stdout",
            "mismatch_class": "stdout_mismatch",
        },
        "reason_code": "structured_diff_mismatch",
        "schema_version": 1,
    }

    trace_oracle = normalize_failure_oracle(
        {
            "kind": "trace",
            "event": "guard_fail",
            "signature": "invoke_ffi_deopt",
            "match_mode": "exact",
        }
    )
    assert trace_oracle == {
        "kind": "trace_signature",
        "match": {
            "event": "guard_fail",
            "match_mode": "exact",
            "signature": "invoke_ffi_deopt",
        },
        "reason_code": "trace_signature",
        "schema_version": 1,
    }

    manifest_oracle = normalize_failure_oracle(
        {
            "kind": "manifest",
            "predicates": [
                {"path": "data.status", "op": "equals", "value": "bad"},
                {"path": "artifacts.reduced_source", "op": "exists"},
            ],
        }
    )
    assert manifest_oracle == {
        "kind": "manifest_predicate",
        "match": {
            "predicates": [
                {"op": "exists", "path": "artifacts.reduced_source"},
                {"op": "equals", "path": "data.status", "value": "bad"},
            ]
        },
        "reason_code": "manifest_predicate",
        "schema_version": 1,
    }


def test_load_reduction_input_accepts_source_or_manifest(tmp_path: Path) -> None:
    source_path = tmp_path / "sample.py"
    source_path.write_text("print('source')\n", encoding="utf-8")

    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "subcommand": "reduce",
                "data": {
                    "source_path": str(source_path),
                    "source_text": "print('manifest')\n",
                    "oracle": {"kind": "exit:nonzero"},
                },
            }
        ),
        encoding="utf-8",
    )

    source_input = load_reduction_input(source_path)
    assert source_input.input_kind == "source"
    assert source_input.source_path == source_path
    assert source_input.source_text == "print('source')\n"

    manifest_input = load_reduction_input(manifest_path)
    assert manifest_input.input_kind == "manifest"
    assert manifest_input.source_path == source_path
    assert manifest_input.source_text == "print('manifest')\n"
    assert manifest_input.manifest_path == manifest_path
    assert manifest_input.original_manifest is not None


def test_reduce_source_and_payload_are_promotion_ready(tmp_path: Path) -> None:
    source_path = tmp_path / "failing_case.py"
    source_path.write_text(
        "\n".join(
            [
                "print('noise-1')",
                "print('KEEP_MARK')",
                "print('noise-2')",
                "",
            ]
        ),
        encoding="utf-8",
    )
    oracle = normalize_failure_oracle(
        {"kind": "trace", "event": "stdout", "signature": "KEEP_MARK"}
    )

    def evaluator(candidate: str) -> dict[str, object]:
        return {
            "matched": "KEEP_MARK" in candidate,
            "trace_events": [
                {
                    "event": "stdout",
                    "signature": "KEEP_MARK" if "KEEP_MARK" in candidate else "NO_MATCH",
                }
            ],
            "manifest": {
                "data": {
                    "status": "bad" if "KEEP_MARK" in candidate else "good",
                    "candidate_length": len(candidate),
                }
            },
        }

    result = reduce_source_text(
        load_reduction_input(source_path),
        oracle=oracle,
        evaluator=evaluator,
    )
    payload = build_reduction_payload(result)

    assert result.reduced_source.strip() == "print('KEEP_MARK')"
    assert payload["input"]["kind"] == "source"
    assert payload["oracle"] == oracle
    assert payload["reduction"] == {
        "original_bytes": len(source_path.read_text(encoding="utf-8")),
        "original_lines": 4,
        "preserved_failure": True,
        "reduced_bytes": len(result.reduced_source),
        "reduced_lines": 1,
        "strategy": "ddmin-lines",
    }
    assert payload["artifacts"]["reduced_source"].endswith("failing_case_reduced.py")
    assert payload["promotion"] == {
        "category": "differential_regression",
        "recommended_path": "tests/differential/debug/failing_case_reduced.py",
    }
    assert isinstance(payload["failure_signature"], str)
    assert len(payload["failure_signature"]) == 64


def test_bisect_first_bad_pass_returns_canonical_result_shape() -> None:
    passes = ["parse", "typecheck", "lower_inline_cache", "codegen"]
    oracle = normalize_failure_oracle({"kind": "trace", "signature": "lower_inline_cache"})

    def evaluator(prefix: tuple[str, ...]) -> dict[str, object]:
        matched = "lower_inline_cache" in prefix
        return {
            "matched": matched,
            "trace_events": [
                {
                    "event": "pass",
                    "signature": "lower_inline_cache" if matched else "clean",
                }
            ],
        }

    result = bisect_first_bad_pass(passes, oracle=oracle, evaluator=evaluator)

    assert result["mode"] == "first_bad_pass"
    assert result["status"] == "ok"
    assert result["first_bad_index"] == 2
    assert result["first_bad_pass"] == "lower_inline_cache"
    assert result["pass_window"] == {
        "start": 2,
        "end": 2,
        "passes": ["lower_inline_cache"],
    }
    assert result["decisions"]
    assert isinstance(result["failure_signature"], str)
    assert len(result["failure_signature"]) == 64


def test_bisect_backend_profile_ic_returns_minimal_bad_toggle_shape() -> None:
    oracle = normalize_failure_oracle({"kind": "diff", "mismatch_class": "stderr_mismatch"})

    def evaluator(config: dict[str, object]) -> dict[str, object]:
        matched = config["ic"] is False
        return {
            "matched": matched,
            "diff": {
                "mismatch_class": "stderr_mismatch" if matched else "match",
                "field": "stderr",
            },
        }

    result = bisect_backend_profile_ic(
        baseline={"backend": "native", "profile": "dev", "ic": True},
        failing={"backend": "wasm", "profile": "release", "ic": False},
        oracle=oracle,
        evaluator=evaluator,
    )

    assert result["mode"] == "config_toggle_bisect"
    assert result["status"] == "ok"
    assert result["baseline"] == {"backend": "native", "profile": "dev", "ic": True}
    assert result["failing"] == {"backend": "wasm", "profile": "release", "ic": False}
    assert result["minimal_bad_dimensions"] == ["ic"]
    assert result["minimal_bad_config"] == {
        "backend": "native",
        "profile": "dev",
        "ic": False,
    }
    assert result["decisions"]
    assert isinstance(result["failure_signature"], str)
    assert len(result["failure_signature"]) == 64


def test_probe_supervisor_attempt_configs_are_canonical() -> None:
    attempts = (
        ProbeSupervisorAttemptConfig(attempt=1, jobs=2, timeout_sec=600),
        ProbeSupervisorAttemptConfig(attempt=2, jobs=1, timeout_sec=600),
    )
    assert attempts[0].attempt == 1
    assert attempts[1].jobs == 1
