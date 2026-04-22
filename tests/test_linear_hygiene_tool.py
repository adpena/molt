from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

import tools.linear_hygiene as linear_hygiene


def test_sanitize_issue_title_removes_trailing_noise() -> None:
    assert (
        linear_hygiene.sanitize_issue_title(
            "[P1][RT2] async I/O cancellation propagation.) |"
        )
        == "[P1][RT2] async I/O cancellation propagation)"
    )
    assert (
        linear_hygiene.sanitize_issue_title(
            "[P1][DB2] wasm connector parity with cancellation bytes))"
        )
        == "[P1][DB2] wasm connector parity with cancellation bytes)"
    )


def test_canonicalize_title_normalizes_variants() -> None:
    left = linear_hygiene.canonicalize_title(
        "[P1][RT2] async I/O cancellation propagation.)"
    )
    right = linear_hygiene.canonicalize_title(
        "[P1][RT2] async I/O cancellation propagation)"
    )
    assert left == right


def test_infer_seed_metadata_fills_missing_source_from_manifest() -> None:
    issue = {
        "title": "[P1][RT2] async I/O cancellation propagation.)",
        "description": (
            "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
            "Original TODO: async I/O cancellation propagation\n"
            "Area: async-runtime\n"
            "Owner lane: runtime\n"
            "Milestone: RT2\n"
            "Status tag: partial\n"
        ),
    }
    lookup = {
        linear_hygiene.canonicalize_title(
            "[P1][RT2] async I/O cancellation propagation)"
        ): {
            "source": "ROADMAP.md:123",
            "priority": "P1",
            "area": "async-runtime",
            "owner": "runtime",
            "milestone": "RT2",
            "status": "partial",
        }
    }
    metadata = linear_hygiene._infer_seed_metadata(issue=issue, manifest_lookup=lookup)
    assert metadata is not None
    assert metadata["source"] == "ROADMAP.md:123"
    assert metadata["priority"] == "P1"
    assert metadata["owner"] == "runtime"


def test_infer_seed_metadata_accepts_legacy_seed_header() -> None:
    issue = {
        "title": "[P1][RT2] async I/O cancellation propagation.)",
        "description": (
            "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
            "Original TODO: async I/O cancellation propagation\n"
            "Area: async-runtime\n"
            "Owner lane: runtime\n"
            "Milestone: RT2\n"
            "Status tag: partial\n"
        ),
    }
    lookup = {
        linear_hygiene.canonicalize_title(
            "[P1][RT2] async I/O cancellation propagation)"
        ): {
            "source": "ROADMAP.md:123",
            "priority": "P1",
            "area": "async-runtime",
            "owner": "runtime",
            "milestone": "RT2",
            "status": "partial",
        }
    }

    metadata = linear_hygiene._infer_seed_metadata(issue=issue, manifest_lookup=lookup)

    assert metadata is not None
    assert metadata["source"] == "ROADMAP.md:123"


def test_run_formal_suite_warns_on_runtime_mismatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    payload = {
        "ok": False,
        "checks": {
            "quint": {"diagnostics": {"runtime_mismatch_detected": True}, "errors": []}
        },
    }
    monkeypatch.setattr(
        linear_hygiene.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(
            returncode=1,
            stdout=json.dumps(payload),
            stderr="",
        ),
    )
    result = linear_hygiene._run_formal_suite("quint")
    assert result["status"] == "warn"
    assert "--quint" in result["command"]


def test_run_formal_suite_rejects_invalid_mode() -> None:
    with pytest.raises(RuntimeError):
        linear_hygiene._run_formal_suite("invalid")


def test_project_name_for_issue_uses_title_prefix_overrides() -> None:
    assert (
        linear_hygiene._project_name_for_issue(
            {"title": "orchestration: role routing cleanup"}, {}
        )
        == "Tooling & DevEx"
    )
    assert (
        linear_hygiene._project_name_for_issue(
            {"title": "formal: prove deterministic scheduler"}, {}
        )
        == "Testing & Differential"
    )


def test_project_name_for_issue_routes_metadata_areas() -> None:
    assert (
        linear_hygiene._project_name_for_issue(
            {"title": "[P1][DB1] SQLite demo path before Postgres)"},
            {"area": "db", "owner": "runtime"},
        )
        == "Offload & Data Ecosystem"
    )
    assert (
        linear_hygiene._project_name_for_issue(
            {"title": "[P1][SL3] native HTTP package)"},
            {"area": "http-runtime", "owner": "runtime"},
        )
        == "Offload & Data Ecosystem"
    )
    assert (
        linear_hygiene._project_name_for_issue(
            {"title": "[P2][TL2] benchmarking regression gates)"},
            {"area": "observability", "owner": "tooling"},
        )
        == "Tooling & DevEx"
    )


def test_build_expected_local_artifacts_partitions_seed_items_and_updates_counts() -> (
    None
):
    seed_items = [
        {
            "title": "[P0][LF2] add per-pass wall-time telemetry",
            "description": "compiler",
            "priority": 1,
            "metadata": {
                "area": "compiler",
                "owner": "compiler",
                "milestone": "LF2",
                "priority": "P0",
                "status": "partial",
                "source": "ROADMAP.md:1",
            },
        },
        {
            "title": "[P1][DB1] SQLite demo path before Postgres)",
            "description": "db",
            "priority": 2,
            "metadata": {
                "area": "db",
                "owner": "runtime",
                "milestone": "DB1",
                "priority": "P1",
                "status": "planned",
                "source": "ROADMAP.md:2",
            },
        },
        {
            "title": "[P1][SL3] replace `_hashlib` top-level stub with full intrinsic-backed lowering",
            "description": "runtime stdlib a",
            "priority": 2,
            "metadata": {
                "area": "stdlib-compat",
                "owner": "stdlib",
                "milestone": "SL3",
                "priority": "P1",
                "status": "partial",
                "source": "ROADMAP.md:11",
            },
        },
        {
            "title": "[P1][SL3] replace `_csv` top-level stub with full intrinsic-backed lowering",
            "description": "runtime stdlib b",
            "priority": 2,
            "metadata": {
                "area": "stdlib-compat",
                "owner": "stdlib",
                "milestone": "SL3",
                "priority": "P1",
                "status": "planned",
                "source": "ROADMAP.md:12",
            },
        },
        {
            "title": "[P1][SL3] implement `molt extension build`",
            "description": "tooling",
            "priority": 2,
            "metadata": {
                "area": "tooling",
                "owner": "tooling",
                "milestone": "SL3",
                "priority": "P1",
                "status": "partial",
                "source": "ROADMAP.md:13",
            },
        },
    ]
    index_rows = [
        {
            "project": "Compiler & Frontend",
            "path": "ops/linear/manifests/compiler_and_frontend.json",
            "count": 0,
        },
        {
            "project": "Offload & Data Ecosystem",
            "path": "ops/linear/manifests/offload_and_data_ecosystem.json",
            "count": 0,
        },
        {
            "project": "Runtime & Intrinsics",
            "path": "ops/linear/manifests/runtime_and_intrinsics.json",
            "count": 0,
        },
        {
            "project": "Tooling & DevEx",
            "path": "ops/linear/manifests/tooling_and_devex.json",
            "count": 0,
        },
    ]

    manifests, updated_index = linear_hygiene._build_expected_local_artifacts(
        seed_items=seed_items, index_rows=index_rows
    )

    assert [row["count"] for row in updated_index] == [1, 1, 1, 1]
    assert [
        item["metadata"]["group_key"]
        for item in manifests["ops/linear/manifests/compiler_and_frontend.json"]
    ] == ["compiler-and-frontend:compiler-optimization-and-lowering"]
    assert [
        item["metadata"]["group_key"]
        for item in manifests["ops/linear/manifests/offload_and_data_ecosystem.json"]
    ] == ["offload-and-data-ecosystem:offload-database-and-dataframe"]
    assert len(manifests["ops/linear/manifests/runtime_and_intrinsics.json"]) == 1
    assert (
        manifests["ops/linear/manifests/runtime_and_intrinsics.json"][0]["metadata"][
            "leaf_count"
        ]
        == 2
    )
    assert [
        item["metadata"]["group_key"]
        for item in manifests["ops/linear/manifests/tooling_and_devex.json"]
    ] == ["tooling-and-devex:tooling-extension-build-and-abi"]


def test_dspy_runtime_status_uses_custom_api_key_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_LINEAR_HYGIENE_DSPY_ENABLE", "1")
    monkeypatch.setenv("MOLT_LINEAR_HYGIENE_DSPY_MODEL", "openai/gpt-4.1-mini")
    monkeypatch.setenv("MOLT_LINEAR_HYGIENE_DSPY_API_KEY_ENV", "ALT_KEY")
    monkeypatch.setenv("ALT_KEY", "abc123")
    monkeypatch.setattr(linear_hygiene, "dspy", object())
    monkeypatch.setattr(linear_hygiene, "BaseModel", object())

    status = linear_hygiene._dspy_runtime_status()
    assert status["enabled"] is True
    assert status["api_key_env"] == "ALT_KEY"
    assert status["api_key_present"] is True
    assert status["reason"] == "ready"
    assert status["ready"] is True


def test_dspy_route_decision_falls_back_when_not_ready(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_LINEAR_HYGIENE_DSPY_ENABLE", "1")
    monkeypatch.delenv("MOLT_LINEAR_HYGIENE_DSPY_MODEL", raising=False)
    fallback = linear_hygiene.RouteDecision(
        role="executor",
        formal_required=False,
        rationale="heuristic",
        extra_labels=[],
    )
    decision = linear_hygiene._dspy_route_decision(
        issue={"title": "x"}, fallback=fallback
    )
    assert decision == fallback
