from __future__ import annotations

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
