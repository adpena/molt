from __future__ import annotations

import json
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

import tools.symphony_readiness_audit as readiness_audit


def test_title_hygiene_flags_detects_common_manifest_noise() -> None:
    flags = readiness_audit._title_hygiene_flags(
        "[P1][RT2] example issue with noisy trailer.) |"
    )
    assert "trailing_period_before_close_paren" in flags
    assert "trailing_pipe_marker" in flags


def test_extract_metadata_block_parses_dash_and_star_markers() -> None:
    description = (
        "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
        "- area: runtime\n"
        "* owner: stdlib\n"
        "- milestone: SL1\n"
        "- priority: P1\n"
        "- source: ROADMAP.md:123\n"
        "- status: partial\n"
    )
    metadata = readiness_audit._extract_metadata_block(description)
    assert metadata["area"] == "runtime"
    assert metadata["owner"] == "stdlib"
    assert metadata["milestone"] == "SL1"
    assert metadata["priority"] == "P1"
    assert metadata["source"] == "ROADMAP.md:123"
    assert metadata["status"] == "partial"


def test_audit_manifest_entries_reports_title_and_metadata_gaps() -> None:
    entries = [
        {
            "title": "[P1][SL1] clean title",
            "metadata": {
                "area": "runtime",
                "owner": "stdlib",
                "milestone": "SL1",
                "priority": "P1",
                "status": "planned",
                "source": "ROADMAP.md:1",
            },
        },
        {
            "title": "[P1][SL1] malformed.) |",
            "metadata": {
                "area": "runtime",
                "owner": "stdlib",
            },
        },
    ]
    report = readiness_audit._audit_manifest_entries(
        manifest_path=Path("ops/linear/manifests/sample.json"),
        entries=entries,
    )
    assert len(report["malformed_titles"]) == 1
    assert len(report["metadata_gaps"]) == 1
    assert report["metadata_gaps"][0]["missing"] == [
        "milestone",
        "priority",
        "status",
        "source",
    ]


def test_overall_status_orders_fail_then_warn_then_pass() -> None:
    assert (
        readiness_audit._overall_status([{"severity": "warn"}, {"severity": "info"}])
        == "warn"
    )
    assert (
        readiness_audit._overall_status([{"severity": "fail"}, {"severity": "warn"}])
        == "fail"
    )
    assert readiness_audit._overall_status([{"severity": "info"}]) == "pass"


def test_strict_autonomy_promotes_selected_warnings_to_failures() -> None:
    findings = [
        {"severity": "warn", "code": "linear_no_active_flow"},
        {"severity": "warn", "code": "linear_labels_minimal"},
    ]
    strict = readiness_audit._apply_strict_autonomy(findings)
    promoted = [row for row in strict if row["code"] == "linear_no_active_flow"][0]
    untouched = [row for row in strict if row["code"] == "linear_labels_minimal"][0]
    assert promoted["severity"] == "fail"
    assert promoted["strict_autonomy_promoted"] is True
    assert untouched["severity"] == "warn"


def test_audit_lin_cli_compat_flags_schema_drift(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text("LINEAR_API_KEY=test-token\n", encoding="utf-8")

    monkeypatch.setattr(readiness_audit.shutil, "which", lambda _name: "/usr/bin/lin")
    monkeypatch.setattr(
        readiness_audit.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout="", stderr=""),
    )
    monkeypatch.setattr(
        readiness_audit.linear_workspace,
        "graphql",
        lambda _query, _variables=None: {"__type": {"fields": [{"name": "id"}]}},
    )

    result = readiness_audit._audit_lin_cli_compat(env_file)
    assert result["status"] == "warn"
    assert result["reason"] == "schema_missing_project_milestone"


def test_run_audit_uses_linear_api_key_from_env_file(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text("LINEAR_API_KEY=file-token\n", encoding="utf-8")
    monkeypatch.delenv("LINEAR_API_KEY", raising=False)

    monkeypatch.setattr(
        readiness_audit,
        "_audit_env_and_volume",
        lambda env_file, ext_root: {  # type: ignore[no-untyped-def]
            "status": "pass",
            "ext_root_mounted": True,
            "missing_env_keys": [],
            "has_linear_api_key": True,
            "ext_root": str(ext_root),
        },
    )
    monkeypatch.setattr(
        readiness_audit, "_audit_docs_and_tools", lambda _repo_root: {"status": "pass"}
    )
    monkeypatch.setattr(
        readiness_audit,
        "_audit_launchd",
        lambda: {"status": "pass", "main_loaded": True, "watchdog_loaded": True},
    )
    monkeypatch.setattr(
        readiness_audit,
        "_audit_durable_memory",
        lambda _durable_root: {
            "status": "pass",
            "checks": {
                "jsonl_readable": {"ok": True},
                "duckdb_readable": {"ok": True},
            },
        },
    )
    monkeypatch.setattr(
        readiness_audit, "_audit_manifest_index", lambda _index_path: {"status": "pass"}
    )
    monkeypatch.setattr(
        readiness_audit,
        "_audit_linear_workspace",
        lambda _team: {"status": "pass", "env_seen": os.environ.get("LINEAR_API_KEY")},
    )
    monkeypatch.setattr(
        readiness_audit,
        "_audit_lin_cli_compat",
        lambda _env_file: {"status": "pass", "lin_installed": True},
    )
    monkeypatch.setattr(
        readiness_audit,
        "_audit_formal_suite",
        lambda _repo_root, _mode: {"status": "pass", "mode": _mode},
    )

    report = readiness_audit.run_audit(
        repo_root=tmp_path,
        team="MOL",
        env_file=env_file,
        index_path=tmp_path / "index.json",
        ext_root=tmp_path,
        durable_root=tmp_path / "durable",
        strict_autonomy=False,
    )
    linear_section = report["sections"]["linear_workspace"]
    assert linear_section["env_seen"] == "file-token"


def test_collect_findings_marks_formal_toolchain_mismatch_warn() -> None:
    report = {
        "sections": {
            "environment": {
                "ext_root_mounted": True,
                "missing_env_keys": [],
                "has_linear_api_key": True,
            },
            "docs_and_tools": {
                "missing_docs": [],
                "missing_tools": [],
                "has_human_authority_gate": True,
            },
            "launchd": {"main_loaded": True, "watchdog_loaded": True},
            "durable_memory": {
                "checks": {
                    "jsonl_readable": {"ok": True},
                    "duckdb_readable": {"ok": True},
                }
            },
            "manifest_index": {
                "missing_manifest_files": [],
                "malformed_titles": [],
                "metadata_gaps": [],
            },
            "linear_workspace": {
                "status": "pass",
                "missing_project": [],
                "seeded_missing_metadata": [],
                "malformed_titles": [],
                "active_execution_flow": True,
                "label_count": 10,
            },
            "linear_cli_compat": {"status": "pass", "lin_installed": True},
            "formal_suite": {
                "status": "warn",
                "mode": "all",
                "reason": "toolchain_mismatch",
                "returncode": 1,
            },
        }
    }
    findings = readiness_audit._collect_findings(report)
    codes = {row["code"]: row for row in findings}
    assert "formal_suite_toolchain_mismatch" in codes
    assert codes["formal_suite_toolchain_mismatch"]["severity"] == "warn"


def test_collect_findings_warns_on_node25_even_when_formal_passes() -> None:
    report = {
        "sections": {
            "environment": {
                "ext_root_mounted": True,
                "missing_env_keys": [],
                "has_linear_api_key": True,
            },
            "docs_and_tools": {
                "missing_docs": [],
                "missing_tools": [],
                "has_human_authority_gate": True,
            },
            "launchd": {"main_loaded": True, "watchdog_loaded": True},
            "durable_memory": {
                "checks": {
                    "jsonl_readable": {"ok": True},
                    "duckdb_readable": {"ok": True},
                }
            },
            "manifest_index": {
                "missing_manifest_files": [],
                "malformed_titles": [],
                "metadata_gaps": [],
            },
            "linear_workspace": {
                "status": "pass",
                "missing_project": [],
                "seeded_missing_metadata": [],
                "malformed_titles": [],
                "active_execution_flow": True,
                "label_count": 10,
            },
            "linear_cli_compat": {"status": "pass", "lin_installed": True},
            "formal_suite": {
                "status": "pass",
                "mode": "all",
                "returncode": 0,
                "report": {
                    "ok": True,
                    "checks": {
                        "quint": {
                            "ok": True,
                            "diagnostics": {
                                "fallback_used": True,
                                "fallback_prefix": ["npx", "-y", "node@22"],
                                "node": {"major": 25, "version": "v25.8.0"},
                            },
                        }
                    },
                },
            },
        }
    }
    findings = readiness_audit._collect_findings(report)
    codes = {row["code"]: row for row in findings}
    assert codes["formal_suite_pass"]["severity"] == "info"
    assert codes["formal_suite_fallback_used"]["severity"] == "info"
    assert codes["formal_suite_node_major_mismatch"]["severity"] == "warn"


def test_audit_formal_suite_classifies_java_runtime_missing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    payload = {
        "ok": False,
        "checks": {
            "quint": {
                "ok": False,
                "errors": ["quint_java_runtime_missing: Java runtime missing"],
                "diagnostics": {"java_runtime_missing": True},
            }
        },
    }

    monkeypatch.setattr(
        readiness_audit.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(
            returncode=1, stdout=json.dumps(payload), stderr=""
        ),
    )

    result = readiness_audit._audit_formal_suite(tmp_path, "all")
    assert result["status"] == "fail"
    assert result["reason"] == "java_runtime_missing"


def test_collect_findings_reports_java_runtime_missing_failure() -> None:
    report = {
        "sections": {
            "environment": {
                "ext_root_mounted": True,
                "missing_env_keys": [],
                "has_linear_api_key": True,
            },
            "docs_and_tools": {
                "missing_docs": [],
                "missing_tools": [],
                "has_human_authority_gate": True,
            },
            "launchd": {"main_loaded": True, "watchdog_loaded": True},
            "durable_memory": {
                "checks": {
                    "jsonl_readable": {"ok": True},
                    "duckdb_readable": {"ok": True},
                }
            },
            "manifest_index": {
                "missing_manifest_files": [],
                "malformed_titles": [],
                "metadata_gaps": [],
            },
            "linear_workspace": {
                "status": "pass",
                "missing_project": [],
                "seeded_missing_metadata": [],
                "malformed_titles": [],
                "active_execution_flow": True,
                "label_count": 10,
            },
            "linear_cli_compat": {"status": "pass", "lin_installed": True},
            "formal_suite": {
                "status": "fail",
                "mode": "all",
                "reason": "java_runtime_missing",
                "returncode": 1,
            },
        }
    }
    findings = readiness_audit._collect_findings(report)
    codes = {row["code"]: row for row in findings}
    assert codes["formal_suite_java_runtime_missing"]["severity"] == "fail"


def test_audit_harness_engineering_scores_full_coverage(tmp_path: Path) -> None:
    harness_doc = tmp_path / "docs" / "HARNESS_ENGINEERING.md"
    harness_doc.parent.mkdir(parents=True, exist_ok=True)
    harness_doc.write_text(
        (
            "Agent-first repository legibility.\n"
            "Quality gate checks are deterministic.\n"
            "Execution plan artifacts live in docs/exec-plans.\n"
            "Observability and intervention loops are required.\n"
            "Doc gardening and entropy cleanup run continuously.\n"
            "Recursive and continual learning loops are enforced.\n"
        ),
        encoding="utf-8",
    )
    (tmp_path / "docs" / "QUALITY_SCORE.md").write_text("# score\n", encoding="utf-8")
    (tmp_path / "docs" / "exec-plans" / "TEMPLATE.md").parent.mkdir(
        parents=True, exist_ok=True
    )
    (tmp_path / "docs" / "exec-plans" / "TEMPLATE.md").write_text(
        "# template\n", encoding="utf-8"
    )
    (tmp_path / "docs" / "exec-plans" / "active" / "README.md").parent.mkdir(
        parents=True, exist_ok=True
    )
    (tmp_path / "docs" / "exec-plans" / "active" / "README.md").write_text(
        "# active\n", encoding="utf-8"
    )
    (tmp_path / "docs" / "exec-plans" / "completed" / "README.md").parent.mkdir(
        parents=True, exist_ok=True
    )
    (tmp_path / "docs" / "exec-plans" / "completed" / "README.md").write_text(
        "# completed\n", encoding="utf-8"
    )

    result = readiness_audit._audit_harness_engineering(tmp_path)
    assert result["status"] == "pass"
    assert result["score"] == 100
    assert result["missing_artifacts"] == []
    assert result["missing_principles"] == []


def test_audit_harness_engineering_fails_when_core_artifacts_missing(
    tmp_path: Path,
) -> None:
    result = readiness_audit._audit_harness_engineering(tmp_path)
    assert result["status"] == "fail"
    assert "docs/HARNESS_ENGINEERING.md" in result["critical_missing_artifacts"]
    assert "docs/QUALITY_SCORE.md" in result["critical_missing_artifacts"]


def test_collect_findings_reports_harness_score_gap() -> None:
    report = {
        "sections": {
            "environment": {
                "ext_root_mounted": True,
                "missing_env_keys": [],
                "has_linear_api_key": True,
            },
            "docs_and_tools": {
                "missing_docs": [],
                "missing_tools": [],
                "has_human_authority_gate": True,
            },
            "harness_engineering": {
                "score": 72,
                "target_score": 90,
                "missing_artifacts": ["docs/exec-plans/completed/README.md"],
                "critical_missing_artifacts": [],
                "missing_principles": ["entropy_cleanup_loop"],
            },
            "launchd": {"main_loaded": True, "watchdog_loaded": True},
            "durable_memory": {
                "checks": {
                    "jsonl_readable": {"ok": True},
                    "duckdb_readable": {"ok": True},
                }
            },
            "manifest_index": {
                "missing_manifest_files": [],
                "malformed_titles": [],
                "metadata_gaps": [],
            },
            "linear_workspace": {
                "status": "pass",
                "missing_project": [],
                "seeded_missing_metadata": [],
                "malformed_titles": [],
                "active_execution_flow": True,
                "label_count": 10,
            },
            "linear_cli_compat": {"status": "pass", "lin_installed": True},
            "formal_suite": {"status": "pass", "mode": "inventory", "returncode": 0},
        }
    }
    findings = readiness_audit._collect_findings(report)
    codes = {row["code"]: row for row in findings}
    assert codes["harness_artifacts_missing"]["severity"] == "warn"
    assert codes["harness_principles_missing"]["severity"] == "warn"
    assert codes["harness_score_below_target"]["severity"] == "warn"


def test_audit_dspy_routing_warns_when_enabled_and_module_missing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    env_file = tmp_path / "symphony.env"
    env_file.write_text(
        (
            "MOLT_SYMPHONY_DSPY_ENABLE=1\n"
            "MOLT_SYMPHONY_DSPY_MODEL=openai/gpt-4.1-mini\n"
            "MOLT_SYMPHONY_DSPY_API_KEY_ENV=OPENAI_API_KEY\n"
            "OPENAI_API_KEY=test-key\n"
        ),
        encoding="utf-8",
    )

    def _fake_find_spec(name: str) -> object | None:
        if name == "dspy":
            return None
        if name == "pydantic":
            return object()
        return object()

    monkeypatch.setattr(readiness_audit.importlib.util, "find_spec", _fake_find_spec)

    result = readiness_audit._audit_dspy_routing(env_file)
    assert result["status"] == "warn"
    assert result["enabled"] is True
    assert result["reason"] == "dspy_module_unavailable"


def test_collect_findings_reports_dspy_not_ready_warn() -> None:
    report = {
        "sections": {
            "environment": {
                "ext_root_mounted": True,
                "missing_env_keys": [],
                "has_linear_api_key": True,
            },
            "docs_and_tools": {
                "missing_docs": [],
                "missing_tools": [],
                "has_human_authority_gate": True,
            },
            "harness_engineering": {
                "score": 100,
                "target_score": 90,
                "missing_artifacts": [],
                "critical_missing_artifacts": [],
                "missing_principles": [],
            },
            "dspy_routing": {
                "status": "warn",
                "enabled": True,
                "reason": "model_missing",
                "model_configured": False,
                "api_key_present": True,
                "module_available": True,
                "pydantic_available": True,
                "api_key_env": "OPENAI_API_KEY",
            },
            "launchd": {"main_loaded": True, "watchdog_loaded": True},
            "durable_memory": {
                "checks": {
                    "jsonl_readable": {"ok": True},
                    "duckdb_readable": {"ok": True},
                }
            },
            "manifest_index": {
                "missing_manifest_files": [],
                "malformed_titles": [],
                "metadata_gaps": [],
            },
            "linear_workspace": {
                "status": "pass",
                "missing_project": [],
                "seeded_missing_metadata": [],
                "malformed_titles": [],
                "active_execution_flow": True,
                "label_count": 10,
            },
            "linear_cli_compat": {"status": "pass", "lin_installed": True},
            "formal_suite": {
                "status": "pass",
                "mode": "inventory",
                "returncode": 0,
            },
        }
    }
    findings = readiness_audit._collect_findings(report)
    codes = {row["code"]: row for row in findings}
    assert codes["dspy_routing_not_ready"]["severity"] == "warn"


def _sample_report(
    *,
    generated_at: str = "2026-03-05T13:17:18.842231Z",
    overall_status: str = "pass",
    linear_status: str = "pass",
    issue_count: int = 211,
    project_count: int = 8,
    label_count: int = 19,
    active_execution_flow: bool = True,
    formal_mode: str = "all",
    durable_jsonl: int = 73_545_691,
    durable_duckdb: int = 12_070_912,
    durable_parquet: int = 1_158_973,
) -> dict[str, object]:
    return {
        "generated_at": generated_at,
        "overall_status": overall_status,
        "sections": {
            "linear_workspace": {
                "status": linear_status,
                "issue_count": issue_count,
                "project_count": project_count,
                "label_count": label_count,
                "active_execution_flow": active_execution_flow,
            },
            "harness_engineering": {"score": 100, "target_score": 90},
            "formal_suite": {"status": "pass", "mode": formal_mode},
            "durable_memory": {
                "status": "pass",
                "files": {
                    "jsonl": {"size_bytes": durable_jsonl},
                    "duckdb": {"size_bytes": durable_duckdb},
                    "parquet": {"size_bytes": durable_parquet},
                },
            },
        },
    }


def test_apply_durable_growth_gate_warns_on_budget_breach() -> None:
    report = _sample_report(
        generated_at="2026-03-05T17:15:56.868769Z",
        durable_jsonl=83_608_942,
        durable_duckdb=14_430_208,
        durable_parquet=1_355_670,
    )
    previous = {
        "captured_at": "2026-03-05T13:17:18.842231Z",
        "durable_jsonl_size": 73_545_691,
        "durable_duckdb_size": 12_070_912,
        "durable_parquet_size": 1_158_973,
    }
    findings: list[dict[str, object]] = []
    readiness_audit._apply_durable_growth_gate(
        report=report, findings=findings, previous_baseline=previous
    )
    row = [f for f in findings if f.get("code") == "durable_growth_budget_exceeded"][0]
    assert row["severity"] == "warn"
    details = row["details"]
    assert isinstance(details, dict)
    assert details["threshold_ratio"] == 0.05


def test_apply_durable_growth_gate_emits_info_without_baseline() -> None:
    findings: list[dict[str, object]] = []
    readiness_audit._apply_durable_growth_gate(
        report=_sample_report(), findings=findings, previous_baseline=None
    )
    row = [f for f in findings if f.get("code") == "durable_growth_baseline_missing"][0]
    assert row["severity"] == "info"


def test_persist_harness_metrics_appends_and_dedupes(tmp_path: Path) -> None:
    ext_root = tmp_path / "ext"
    report = _sample_report(generated_at="2026-03-05T13:17:18.842231Z")
    readiness_audit._persist_harness_metrics(ext_root, report, retention_days=90)
    readiness_audit._persist_harness_metrics(ext_root, report, retention_days=90)

    csv_path = ext_root / "logs" / "symphony" / "metrics" / "harness_timeseries.csv"
    lines = csv_path.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 2  # header + one unique row
    assert "2026-03-05T13:17:18.842231Z" in lines[1]

    second = _sample_report(
        generated_at="2026-03-05T17:15:56.868769Z",
        overall_status="warn",
        formal_mode="inventory",
    )
    readiness_audit._persist_harness_metrics(ext_root, second, retention_days=90)
    lines = csv_path.read_text(encoding="utf-8").strip().splitlines()
    assert len(lines) == 3
    assert "2026-03-05T17:15:56.868769Z" in lines[2]

    history = ext_root / "logs" / "symphony" / "readiness" / "history"
    baselines = sorted(history.glob("baseline_*.json"))
    assert len(baselines) == 2


def test_persist_harness_metrics_skips_when_linear_fails(tmp_path: Path) -> None:
    ext_root = tmp_path / "ext"
    failed = _sample_report(
        generated_at="2026-03-05T17:14:09.702724Z",
        overall_status="fail",
        linear_status="fail",
    )
    result = readiness_audit._persist_harness_metrics(
        ext_root, failed, retention_days=90
    )
    assert result["status"] == "skipped"
    csv_path = ext_root / "logs" / "symphony" / "metrics" / "harness_timeseries.csv"
    assert not csv_path.exists()


def test_prune_baseline_history_removes_old_files(tmp_path: Path) -> None:
    history = tmp_path / "history"
    history.mkdir(parents=True, exist_ok=True)
    old_payload = {"captured_at": "2024-01-01T00:00:00Z"}
    new_payload = {"captured_at": "2026-03-05T00:00:00Z"}
    (history / "baseline_old.json").write_text(json.dumps(old_payload), encoding="utf-8")
    (history / "baseline_new.json").write_text(json.dumps(new_payload), encoding="utf-8")
    removed = readiness_audit._prune_baseline_history(history, retention_days=30)
    assert removed >= 1
    assert not (history / "baseline_old.json").exists()
    assert (history / "baseline_new.json").exists()


def test_post_growth_alert_comment_skips_without_breach(tmp_path: Path) -> None:
    report = _sample_report()
    report["findings"] = [{"code": "other", "severity": "info"}]
    env_file = tmp_path / "symphony.env"
    env_file.write_text("", encoding="utf-8")
    out = readiness_audit._post_growth_alert_comment(
        report=report,
        team="Moltlang",
        issue_ref="MOL-211",
        env_file=env_file,
    )
    assert out["status"] == "skipped"
