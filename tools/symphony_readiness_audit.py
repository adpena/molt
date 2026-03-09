from __future__ import annotations

import argparse
import contextlib
import csv
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

import tools.linear_workspace as linear_workspace  # noqa: E402
import tools.symphony_launchd as symphony_launchd  # noqa: E402
from molt.symphony.dlq import DeadLetterQueue  # noqa: E402
from molt.symphony.paths import (  # noqa: E402
    is_within,
    resolve_molt_ext_root,
    resolve_symphony_parent_root,
    resolve_symphony_store_root,
    symphony_api_token_file,
    symphony_dlq_events_file,
    symphony_durable_root,
    symphony_log_root,
    symphony_metrics_dir,
    symphony_readiness_dir,
    symphony_security_events_file,
    symphony_state_root,
    symphony_taste_memory_distillations_dir,
    symphony_taste_memory_events_file,
    symphony_tool_promotion_distillations_dir,
    symphony_tool_promotion_events_file,
    symphony_workspace_root,
)
from molt.symphony.tool_promotion import ToolPromotionStore  # noqa: E402

try:  # pragma: no cover - optional dependency
    import duckdb as _duckdb_module
except Exception:  # pragma: no cover - optional dependency
    _duckdb: Any = None
else:
    _duckdb = _duckdb_module


REQUIRED_ENV_KEYS = (
    "LINEAR_API_KEY",
    "MOLT_LINEAR_PROJECT_SLUG",
    "MOLT_SYMPHONY_SYNC_REMOTE",
    "MOLT_SYMPHONY_SYNC_BRANCH",
    "MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS",
    "MOLT_EXT_ROOT",
    "CARGO_TARGET_DIR",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "MOLT_CACHE",
    "MOLT_DIFF_ROOT",
    "MOLT_DIFF_TMPDIR",
    "UV_CACHE_DIR",
    "TMPDIR",
    "MOLT_SYMPHONY_DURABLE_MEMORY",
    "MOLT_SYMPHONY_DURABLE_ROOT",
    "MOLT_SYMPHONY_PARENT_ROOT",
    "MOLT_SYMPHONY_PROJECT_KEY",
    "MOLT_SYMPHONY_STORE_ROOT",
    "MOLT_SYMPHONY_DLQ_EVENTS_FILE",
    "MOLT_SYMPHONY_TASTE_MEMORY_EVENTS_FILE",
    "MOLT_SYMPHONY_TASTE_MEMORY_DISTILLATIONS_DIR",
    "MOLT_SYMPHONY_TOOL_PROMOTION_EVENTS_FILE",
    "MOLT_SYMPHONY_TOOL_PROMOTION_DISTILLATIONS_DIR",
)

REQUIRED_DOCS = (
    "docs/SYMPHONY.md",
    "docs/SYMPHONY_HUMAN_ROLE.md",
    "docs/SYMPHONY_OPERATOR_PLAYBOOK.md",
    "docs/LINEAR_WORKSPACE_BOOTSTRAP.md",
    "docs/SYMPHONY_CANONICAL_ALIGNMENT.md",
    "docs/HARNESS_ENGINEERING.md",
    "docs/QUALITY_SCORE.md",
    "docs/spec/STATUS.md",
    "ROADMAP.md",
)

REQUIRED_TOOLS = (
    "tools/symphony_bootstrap.py",
    "tools/symphony_run.py",
    "tools/symphony_launchd.py",
    "tools/symphony_watchdog.py",
    "tools/symphony_durable_admin.py",
    "tools/symphony_dlq.py",
    "tools/linear_workspace.py",
    "tools/symphony_perf.py",
    "tools/symphony_taste_memory.py",
    "tools/symphony_tool_promotion.py",
)

REQUIRED_HARNESS_ARTIFACTS = (
    "docs/HARNESS_ENGINEERING.md",
    "docs/QUALITY_SCORE.md",
    "docs/exec-plans/TEMPLATE.md",
    "docs/exec-plans/active/README.md",
    "docs/exec-plans/completed/README.md",
)
CRITICAL_HARNESS_ARTIFACTS = {
    "docs/HARNESS_ENGINEERING.md",
    "docs/QUALITY_SCORE.md",
}
HARNESS_PRINCIPLE_MARKERS: dict[str, tuple[str, ...]] = {
    "agent_repo_legibility": ("agent-first", "repository legibility"),
    "executable_quality_gates": ("quality gate", "deterministic"),
    "execution_plan_discipline": ("execution plan", "docs/exec-plans"),
    "observability_and_intervention": ("observability", "intervention"),
    "entropy_cleanup_loop": ("doc gardening", "entropy cleanup"),
    "recursive_learning_loop": ("recursive", "continual learning"),
}

REQUIRED_METADATA_KEYS = ("area", "owner", "milestone", "priority", "status", "source")
SEED_HEADER = "Auto-seeded from Molt roadmap/status TODO contracts."
_META_LINE_RE = re.compile(r"^\s*[-*]\s*([a-z_]+)\s*:\s*(.+?)\s*$")

STRICT_AUTONOMY_FAIL_CODES = {
    "manifest_titles_malformed",
    "linear_seeded_metadata_gaps",
    "linear_titles_malformed",
    "linear_no_active_flow",
    "harness_score_below_target",
    "harness_score_regressed",
    "active_flow_ratio_low",
    "formal_pass_ratio_low",
    "durable_growth_recurring",
    "dspy_routing_not_ready",
}
FORMAL_SUITE_MODES = ("off", "inventory", "lean", "quint", "all")
DSPY_ENABLE_ENV = "MOLT_SYMPHONY_DSPY_ENABLE"
DSPY_MODEL_ENV = "MOLT_SYMPHONY_DSPY_MODEL"
DSPY_API_KEY_ENV_ENV = "MOLT_SYMPHONY_DSPY_API_KEY_ENV"
DSPY_API_KEY_INLINE_ENV = "MOLT_SYMPHONY_DSPY_API_KEY"
DSPY_DEFAULT_API_KEY_ENV = "OPENAI_API_KEY"

QUERY_LABELS = """
query LabelsByTeam($teamId: ID!) {
  issueLabels(filter: { team: { id: { eq: $teamId } } }, first: 250) {
    nodes { id name color }
  }
}
""".strip()

QUERY_LABELS_GLOBAL = """
query LabelsGlobal {
  issueLabels(first: 250) {
    nodes { id name color }
  }
}
""".strip()

QUERY_PROJECT_TYPE_FIELDS = """
query ProjectTypeFields {
  __type(name: "Project") {
    fields { name }
  }
}
""".strip()

MUTATION_COMMENT_CREATE = """
mutation CommentCreate($input: CommentCreateInput!) {
  commentCreate(input: $input) {
    success
    comment { id body createdAt }
  }
}
""".strip()

HARNESS_TIMESERIES_FIELDS = (
    "captured_at",
    "readiness_overall_status",
    "harness_score",
    "harness_target",
    "linear_issue_count",
    "linear_project_count",
    "linear_label_count",
    "linear_active_execution_flow",
    "formal_suite_status",
    "formal_suite_mode",
    "durable_status",
    "durable_jsonl_size",
    "durable_duckdb_size",
    "durable_parquet_size",
)

DEFAULT_DURABLE_GROWTH_WARN_RATIO = 0.05
DEFAULT_TREND_WINDOW = 12
DEFAULT_MAX_HARNESS_SCORE_DROP = 5
DEFAULT_MIN_ACTIVE_FLOW_RATIO = 0.7
DEFAULT_MIN_FORMAL_PASS_RATIO = 0.8
DEFAULT_MAX_DLQ_OPEN_FAILURES = 3
DEFAULT_MAX_DLQ_RECURRING_FINGERPRINTS = 1

_PRIORITY_ORDER = {"P0": 0, "P1": 1, "P2": 2, "P3": 3}


def _utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _load_env_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip()
    return values


def _coerce_bool(value: str, *, default: bool = False) -> bool:
    raw = value.strip().lower()
    if not raw:
        return default
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return default


def _resolve_linear_api_key(env_file: Path) -> str:
    current = os.environ.get("LINEAR_API_KEY", "").strip()
    if current:
        return current
    return _load_env_file(env_file).get("LINEAR_API_KEY", "").strip()


@contextlib.contextmanager
def _temporary_linear_api_key(api_key: str) -> Any:
    old = os.environ.get("LINEAR_API_KEY")
    had_old = "LINEAR_API_KEY" in os.environ
    if api_key:
        os.environ["LINEAR_API_KEY"] = api_key
    try:
        yield
    finally:
        if had_old:
            os.environ["LINEAR_API_KEY"] = old or ""
        else:
            os.environ.pop("LINEAR_API_KEY", None)


def _title_hygiene_flags(title: str) -> list[str]:
    flags: list[str] = []
    if not title:
        return ["missing_title"]
    if "\\n" in title or "\n" in title:
        flags.append("contains_newline_escape")
    if '\\"' in title or "\\'" in title:
        flags.append("contains_escaped_quote")
    if title.strip() != title:
        flags.append("leading_or_trailing_whitespace")
    if re.search(r"\.\)\s*(?:\||$)", title):
        flags.append("trailing_period_before_close_paren")
    if re.search(r"\)\)\s*(?:\||$)", title):
        flags.append("duplicate_closing_paren")
    if re.search(r"\s+\|\s*$", title):
        flags.append("trailing_pipe_marker")
    if title.endswith(","):
        flags.append("trailing_comma")
    return flags


def _extract_metadata_block(description: str) -> dict[str, str]:
    metadata: dict[str, str] = {}
    for raw in description.splitlines():
        match = _META_LINE_RE.match(raw)
        if not match:
            continue
        key = match.group(1).strip().lower()
        value = match.group(2).strip()
        if key and value:
            metadata[key] = value
    return metadata


def _missing_metadata_keys(metadata: dict[str, str]) -> list[str]:
    return [key for key in REQUIRED_METADATA_KEYS if not metadata.get(key)]


def _file_snapshot(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"exists": False, "size_bytes": 0, "modified_at": None}
    stat = path.stat()
    modified = (
        datetime.fromtimestamp(stat.st_mtime, tz=UTC).isoformat().replace("+00:00", "Z")
    )
    return {"exists": True, "size_bytes": int(stat.st_size), "modified_at": modified}


def _check_jsonl(path: Path, *, max_lines: int = 500) -> dict[str, Any]:
    if not path.exists():
        return {"ok": True, "reason": "missing", "lines_checked": 0}
    lines = 0
    try:
        with path.open("r", encoding="utf-8") as handle:
            for raw in handle:
                if lines >= max_lines:
                    break
                text = raw.strip()
                if not text:
                    continue
                parsed = json.loads(text)
                if not isinstance(parsed, dict):
                    return {"ok": False, "reason": "non_object_json", "line": lines + 1}
                lines += 1
    except Exception as exc:
        return {"ok": False, "reason": "jsonl_parse_failed", "error": str(exc)}
    return {"ok": True, "lines_checked": lines}


def _is_duckdb_lock_error(error: str) -> bool:
    return (
        "Conflicting lock is held" in error
        or "Could not set lock on file" in error
        or "different configuration than existing connections" in error
    )


def _check_duckdb_table_count(path: Path, *, table: str) -> dict[str, Any]:
    if not path.exists():
        return {"ok": True, "reason": "missing"}
    if _duckdb is None:
        return {"ok": True, "reason": "duckdb_unavailable", "warning": True}
    query = f"SELECT COUNT(*) AS c FROM {table}"
    try:
        conn = _duckdb.connect(str(path), read_only=True)
        try:
            rows = conn.execute(query).fetchall()
        finally:
            conn.close()
        count = int(rows[0][0]) if rows else 0
        return {"ok": True, "rows": count}
    except Exception as exc:  # pragma: no cover - duckdb dependent
        error = str(exc)
        if _is_duckdb_lock_error(error):
            return {
                "ok": True,
                "reason": "duckdb_locked_by_writer",
                "warning": True,
                "error": error,
            }
        return {"ok": False, "reason": "duckdb_query_failed", "error": error}


def _check_parquet(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"ok": True, "reason": "missing"}
    if _duckdb is None:
        return {"ok": True, "reason": "duckdb_unavailable", "warning": True}
    path_sql = str(path).replace("'", "''")
    query = f"SELECT COUNT(*) AS c FROM read_parquet('{path_sql}')"
    try:
        conn = _duckdb.connect(":memory:")
        try:
            rows = conn.execute(query).fetchall()
        finally:
            conn.close()
        count = int(rows[0][0]) if rows else 0
        return {"ok": True, "rows": count}
    except Exception as exc:  # pragma: no cover - duckdb dependent
        return {"ok": False, "reason": "parquet_query_failed", "error": str(exc)}


def _audit_durable_memory(root: Path) -> dict[str, Any]:
    jsonl_path = root / "events.jsonl"
    duckdb_path = root / "events.duckdb"
    parquet_path = root / "events.parquet"
    checks = {
        "jsonl_readable": _check_jsonl(jsonl_path),
        "duckdb_readable": _check_duckdb_table_count(duckdb_path, table="events"),
        "parquet_readable": _check_parquet(parquet_path),
    }
    ok = all(bool(check.get("ok")) for check in checks.values())
    warnings = [name for name, check in checks.items() if bool(check.get("warning"))]
    status = "pass" if ok and not warnings else "warn" if ok else "fail"
    return {
        "status": status,
        "root": str(root),
        "files": {
            "jsonl": _file_snapshot(jsonl_path),
            "duckdb": _file_snapshot(duckdb_path),
            "parquet": _file_snapshot(parquet_path),
        },
        "checks": checks,
        "warnings": warnings,
    }


def _audit_launchd() -> dict[str, Any]:
    main_plist = symphony_launchd.plist_path()
    watchdog_plist = symphony_launchd.watchdog_plist_path()
    try:
        proc = subprocess.run(
            ["launchctl", "list"],
            check=False,
            capture_output=True,
            text=True,
        )
    except Exception as exc:  # pragma: no cover - platform dependent
        return {
            "status": "warn",
            "error": str(exc),
            "main_plist_exists": main_plist.exists(),
            "watchdog_plist_exists": watchdog_plist.exists(),
        }
    output = proc.stdout or ""
    main_loaded = symphony_launchd._is_loaded_label(output, symphony_launchd.LABEL)
    watchdog_loaded = symphony_launchd._is_loaded_label(
        output, symphony_launchd.WATCHDOG_LABEL
    )
    status = "pass" if main_loaded and watchdog_loaded else "warn"
    return {
        "status": status,
        "main_plist": str(main_plist),
        "main_plist_exists": main_plist.exists(),
        "main_loaded": main_loaded,
        "watchdog_plist": str(watchdog_plist),
        "watchdog_plist_exists": watchdog_plist.exists(),
        "watchdog_loaded": watchdog_loaded,
        "launchctl_returncode": int(proc.returncode),
    }


def _audit_docs_and_tools(repo_root: Path) -> dict[str, Any]:
    missing_docs = [rel for rel in REQUIRED_DOCS if not (repo_root / rel).exists()]
    missing_tools = [rel for rel in REQUIRED_TOOLS if not (repo_root / rel).exists()]
    human_role_path = repo_root / "docs" / "SYMPHONY_HUMAN_ROLE.md"
    human_role_text = (
        human_role_path.read_text(encoding="utf-8") if human_role_path.exists() else ""
    )
    has_human_authority_gate = (
        "The human remains accountable" in human_role_text
        and "Non-Delegable Human Responsibilities" in human_role_text
    )
    status = (
        "pass"
        if not missing_docs and not missing_tools and has_human_authority_gate
        else "warn"
    )
    return {
        "status": status,
        "missing_docs": missing_docs,
        "missing_tools": missing_tools,
        "has_human_authority_gate": has_human_authority_gate,
    }


def _audit_harness_engineering(repo_root: Path) -> dict[str, Any]:
    artifact_states: list[dict[str, Any]] = []
    missing_artifacts: list[str] = []
    critical_missing_artifacts: list[str] = []

    for rel in REQUIRED_HARNESS_ARTIFACTS:
        exists = (repo_root / rel).exists()
        artifact_states.append({"path": rel, "exists": exists})
        if exists:
            continue
        missing_artifacts.append(rel)
        if rel in CRITICAL_HARNESS_ARTIFACTS:
            critical_missing_artifacts.append(rel)

    harness_doc = repo_root / "docs" / "HARNESS_ENGINEERING.md"
    harness_text = (
        harness_doc.read_text(encoding="utf-8").lower() if harness_doc.exists() else ""
    )
    principle_coverage: dict[str, bool] = {}
    missing_principles: list[str] = []
    for key, markers in HARNESS_PRINCIPLE_MARKERS.items():
        covered = all(marker in harness_text for marker in markers)
        principle_coverage[key] = covered
        if not covered:
            missing_principles.append(key)

    artifact_total = len(REQUIRED_HARNESS_ARTIFACTS)
    principle_total = len(HARNESS_PRINCIPLE_MARKERS)
    artifact_present = artifact_total - len(missing_artifacts)
    principle_present = principle_total - len(missing_principles)
    artifact_score = (
        int(round((60 * artifact_present) / artifact_total)) if artifact_total else 60
    )
    principle_score = (
        int(round((40 * principle_present) / principle_total))
        if principle_total
        else 40
    )
    score = artifact_score + principle_score

    if critical_missing_artifacts:
        status = "fail"
    elif score >= 90:
        status = "pass"
    elif score >= 70:
        status = "warn"
    else:
        status = "fail"

    return {
        "status": status,
        "score": score,
        "target_score": 90,
        "artifact_score": artifact_score,
        "principle_score": principle_score,
        "artifact_states": artifact_states,
        "missing_artifacts": missing_artifacts,
        "critical_missing_artifacts": critical_missing_artifacts,
        "principle_coverage": principle_coverage,
        "missing_principles": missing_principles,
    }


def _audit_dspy_routing(env_file: Path) -> dict[str, Any]:
    env_values = _load_env_file(env_file)
    enabled_raw = env_values.get(DSPY_ENABLE_ENV, os.environ.get(DSPY_ENABLE_ENV, ""))
    enabled = _coerce_bool(enabled_raw, default=False)
    model = str(
        env_values.get(DSPY_MODEL_ENV, os.environ.get(DSPY_MODEL_ENV, ""))
    ).strip()
    api_key_env_name = (
        str(
            env_values.get(
                DSPY_API_KEY_ENV_ENV, os.environ.get(DSPY_API_KEY_ENV_ENV, "")
            )
        ).strip()
        or DSPY_DEFAULT_API_KEY_ENV
    )
    inline_api_key = str(
        env_values.get(
            DSPY_API_KEY_INLINE_ENV, os.environ.get(DSPY_API_KEY_INLINE_ENV, "")
        )
    ).strip()
    scoped_api_key = str(
        env_values.get(api_key_env_name, os.environ.get(api_key_env_name, ""))
    ).strip()
    api_key_present = bool(inline_api_key or scoped_api_key)
    module_available = importlib.util.find_spec("dspy") is not None
    pydantic_available = importlib.util.find_spec("pydantic") is not None

    if not enabled:
        status = "info"
        reason = "disabled"
    elif not module_available:
        status = "warn"
        reason = "dspy_module_unavailable"
    elif not pydantic_available:
        status = "warn"
        reason = "pydantic_unavailable"
    elif not model:
        status = "warn"
        reason = "model_missing"
    elif not api_key_present:
        status = "warn"
        reason = "api_key_missing"
    else:
        status = "pass"
        reason = "ready"

    return {
        "status": status,
        "enabled": enabled,
        "reason": reason,
        "model": model,
        "model_configured": bool(model),
        "api_key_env": api_key_env_name,
        "api_key_present": api_key_present,
        "module_available": module_available,
        "pydantic_available": pydantic_available,
    }


def _audit_manifest_entries(
    *,
    manifest_path: Path,
    entries: list[dict[str, Any]],
) -> dict[str, Any]:
    malformed_titles: list[dict[str, Any]] = []
    metadata_gaps: list[dict[str, Any]] = []
    for idx, item in enumerate(entries, start=1):
        title = str(item.get("title") or "").strip()
        flags = _title_hygiene_flags(title)
        if flags:
            malformed_titles.append(
                {
                    "path": str(manifest_path),
                    "index": idx,
                    "title": title,
                    "flags": flags,
                }
            )
        metadata = item.get("metadata")
        normalized: dict[str, str] = {}
        if isinstance(metadata, dict):
            for key, value in metadata.items():
                if value is None:
                    continue
                normalized[str(key).lower()] = str(value).strip()
        missing = _missing_metadata_keys(normalized)
        if missing:
            metadata_gaps.append(
                {
                    "path": str(manifest_path),
                    "index": idx,
                    "title": title,
                    "missing": missing,
                }
            )
    return {
        "malformed_titles": malformed_titles,
        "metadata_gaps": metadata_gaps,
    }


def _audit_manifest_index(index_path: Path) -> dict[str, Any]:
    if not index_path.exists():
        return {
            "status": "fail",
            "error": f"missing_index:{index_path}",
            "manifest_count": 0,
            "entry_count": 0,
            "malformed_titles": [],
            "metadata_gaps": [],
            "missing_manifest_files": [],
        }

    try:
        index = json.loads(index_path.read_text(encoding="utf-8"))
    except Exception as exc:
        return {
            "status": "fail",
            "error": f"invalid_index_json:{exc}",
            "manifest_count": 0,
            "entry_count": 0,
            "malformed_titles": [],
            "metadata_gaps": [],
            "missing_manifest_files": [],
        }

    if not isinstance(index, list):
        return {
            "status": "fail",
            "error": "index_must_be_list",
            "manifest_count": 0,
            "entry_count": 0,
            "malformed_titles": [],
            "metadata_gaps": [],
            "missing_manifest_files": [],
        }

    malformed_titles: list[dict[str, Any]] = []
    metadata_gaps: list[dict[str, Any]] = []
    missing_manifest_files: list[str] = []
    entry_count = 0

    for row in index:
        if not isinstance(row, dict):
            continue
        rel = str(row.get("path") or "").strip()
        if not rel:
            continue
        manifest_path = Path(rel)
        if not manifest_path.exists():
            missing_manifest_files.append(str(manifest_path))
            continue
        try:
            entries_raw = json.loads(manifest_path.read_text(encoding="utf-8"))
        except Exception:
            missing_manifest_files.append(str(manifest_path))
            continue
        if not isinstance(entries_raw, list):
            missing_manifest_files.append(str(manifest_path))
            continue
        entries = [item for item in entries_raw if isinstance(item, dict)]
        entry_count += len(entries)
        report = _audit_manifest_entries(manifest_path=manifest_path, entries=entries)
        malformed_titles.extend(report["malformed_titles"])
        metadata_gaps.extend(report["metadata_gaps"])

    if missing_manifest_files:
        status = "fail"
    elif malformed_titles or metadata_gaps:
        status = "warn"
    else:
        status = "pass"
    return {
        "status": status,
        "index": str(index_path),
        "manifest_count": len(index),
        "entry_count": entry_count,
        "missing_manifest_files": missing_manifest_files,
        "malformed_titles": malformed_titles,
        "metadata_gaps": metadata_gaps,
    }


def _audit_linear_workspace(team: str) -> dict[str, Any]:
    try:
        team_id = linear_workspace._resolve_team_id(team)
        projects = linear_workspace._fetch_projects(team_id)
        issues = linear_workspace._fetch_issues(team_id, None)
        states = linear_workspace.graphql(
            linear_workspace.QUERY_STATES, {"teamId": team_id}
        )["workflowStates"]["nodes"]
        labels = linear_workspace.graphql(QUERY_LABELS, {"teamId": team_id})[
            "issueLabels"
        ]["nodes"]
        if not labels:
            labels = linear_workspace.graphql(QUERY_LABELS_GLOBAL)["issueLabels"][
                "nodes"
            ]
    except Exception as exc:
        return {"status": "fail", "error": str(exc)}

    state_counts: dict[str, int] = {}
    project_counts: dict[str, int] = {}
    missing_project: list[str] = []
    missing_priority: list[str] = []
    seeded_missing_metadata: list[dict[str, Any]] = []
    malformed_titles: list[dict[str, Any]] = []
    duplicate_titles: dict[str, int] = {}

    for issue in issues:
        identifier = str(issue.get("identifier") or "")
        title = str(issue.get("title") or "").strip()
        state_name = str((issue.get("state") or {}).get("name") or "Unknown")
        state_counts[state_name] = state_counts.get(state_name, 0) + 1

        project_obj = issue.get("project")
        project_name = (
            str((project_obj or {}).get("name") or "")
            if isinstance(project_obj, dict)
            else str(project_obj or "")
        ).strip()
        if project_name:
            project_counts[project_name] = project_counts.get(project_name, 0) + 1
        else:
            missing_project.append(identifier)

        priority = issue.get("priority")
        if not isinstance(priority, int):
            missing_priority.append(identifier)

        title_flags = _title_hygiene_flags(title)
        if title_flags:
            malformed_titles.append(
                {"identifier": identifier, "title": title, "flags": title_flags}
            )

        normalized_title = linear_workspace._title_key(title)
        if normalized_title:
            duplicate_titles[normalized_title] = (
                duplicate_titles.get(normalized_title, 0) + 1
            )

        description = str(issue.get("description") or "")
        if SEED_HEADER not in description:
            continue
        metadata = _extract_metadata_block(description)
        missing = _missing_metadata_keys(metadata)
        if missing:
            seeded_missing_metadata.append(
                {"identifier": identifier, "missing": missing}
            )

    duplicate_title_count = sum(
        count - 1 for count in duplicate_titles.values() if count > 1
    )
    in_progress = int(state_counts.get("In Progress", 0))
    in_review = int(state_counts.get("In Review", 0))
    active_flow = (in_progress + in_review) > 0
    status = "pass"
    if missing_priority:
        status = "fail"
    elif (
        missing_project
        or seeded_missing_metadata
        or malformed_titles
        or not active_flow
    ):
        status = "warn"
    return {
        "status": status,
        "team": team,
        "issue_count": len(issues),
        "project_count": len(projects),
        "workflow_state_count": len(states),
        "label_count": len(labels),
        "state_counts": state_counts,
        "project_counts": project_counts,
        "missing_project": missing_project,
        "missing_priority": missing_priority,
        "seeded_missing_metadata": seeded_missing_metadata,
        "malformed_titles": malformed_titles,
        "duplicate_title_count": duplicate_title_count,
        "active_execution_flow": active_flow,
    }


def _audit_lin_cli_compat(env_file: Path) -> dict[str, Any]:
    lin_path = shutil.which("lin")
    if not lin_path:
        return {
            "status": "warn",
            "lin_installed": False,
            "reason": "lin_not_installed",
            "recommended_cli": "tools/linear_workspace.py",
        }

    api_key = _resolve_linear_api_key(env_file)  # secret-guard: allow
    if not api_key:
        return {
            "status": "warn",
            "lin_installed": True,
            "lin_path": lin_path,
            "reason": "missing_linear_api_key",
            "recommended_cli": "tools/linear_workspace.py",
        }

    try:
        probe_proc = subprocess.run(
            [lin_path, "--help"],
            check=False,
            capture_output=True,
            text=True,
            timeout=5.0,
        )
        lin_help_ok = probe_proc.returncode == 0
    except Exception as exc:
        return {
            "status": "warn",
            "lin_installed": True,
            "lin_path": lin_path,
            "reason": "lin_help_probe_failed",
            "error": str(exc),
            "recommended_cli": "tools/linear_workspace.py",
        }

    try:
        with _temporary_linear_api_key(api_key):
            data = linear_workspace.graphql(QUERY_PROJECT_TYPE_FIELDS)
        type_node = data.get("__type") or {}
        fields = type_node.get("fields") or []
        field_names = {
            str(field.get("name") or "").strip()
            for field in fields
            if isinstance(field, dict)
        }
    except Exception as exc:
        return {
            "status": "warn",
            "lin_installed": True,
            "lin_path": lin_path,
            "reason": "schema_probe_failed",
            "error": str(exc),
            "lin_help_ok": lin_help_ok,
            "recommended_cli": "tools/linear_workspace.py",
        }

    has_project_milestone = "milestone" in field_names
    return {
        "status": "pass" if has_project_milestone else "warn",
        "lin_installed": True,
        "lin_path": lin_path,
        "lin_help_ok": lin_help_ok,
        "project_field_milestone_present": has_project_milestone,
        "reason": "ok" if has_project_milestone else "schema_missing_project_milestone",
        "recommended_cli": "tools/linear_workspace.py",
    }


def _formal_suite_command(mode: str) -> list[str]:
    mode_key = mode.strip().lower()
    command = [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "tools/check_formal_methods.py",
        "--json-only",
    ]
    if mode_key == "inventory":
        command.append("--inventory")
    elif mode_key == "lean":
        command.append("--lean")
    elif mode_key == "quint":
        command.append("--quint")
    return command


def _formal_suite_has_toolchain_mismatch(payload: dict[str, Any]) -> bool:
    checks = payload.get("checks")
    if not isinstance(checks, dict):
        return False
    quint = checks.get("quint")
    if not isinstance(quint, dict):
        return False
    diagnostics = quint.get("diagnostics")
    if isinstance(diagnostics, dict) and bool(
        diagnostics.get("runtime_mismatch_detected")
    ):
        return True
    errors = quint.get("errors")
    if not isinstance(errors, list):
        return False
    return any("quint_runtime_toolchain_mismatch" in str(item) for item in errors)


def _formal_suite_missing_java_runtime(payload: dict[str, Any]) -> bool:
    checks = payload.get("checks")
    if not isinstance(checks, dict):
        return False
    quint = checks.get("quint")
    if not isinstance(quint, dict):
        return False
    diagnostics = quint.get("diagnostics")
    if isinstance(diagnostics, dict) and bool(diagnostics.get("java_runtime_missing")):
        return True
    errors = quint.get("errors")
    if not isinstance(errors, list):
        return False
    return any("quint_java_runtime_missing" in str(item) for item in errors)


def _audit_formal_suite(repo_root: Path, mode: str) -> dict[str, Any]:
    mode_key = mode.strip().lower()
    if mode_key == "off":
        return {
            "status": "info",
            "mode": mode_key,
            "reason": "disabled",
            "command": [],
            "returncode": 0,
            "report": None,
        }
    if mode_key not in FORMAL_SUITE_MODES:
        return {
            "status": "fail",
            "mode": mode_key,
            "reason": "invalid_mode",
            "command": [],
            "returncode": 1,
            "report": None,
        }

    command = _formal_suite_command(mode_key)
    try:
        proc = subprocess.run(
            command,
            cwd=repo_root,
            check=False,
            capture_output=True,
            text=True,
            timeout=1800,
        )
    except Exception as exc:
        return {
            "status": "fail",
            "mode": mode_key,
            "reason": "execution_failed",
            "command": command,
            "returncode": 1,
            "error": str(exc),
            "report": None,
        }

    report: dict[str, Any] | None = None
    stdout = (proc.stdout or "").strip()
    if stdout:
        try:
            parsed = json.loads(stdout)
            if isinstance(parsed, dict):
                report = parsed
        except Exception:
            report = None

    status = "pass"
    reason = "ok"
    if report is None:
        status = "fail"
        reason = "invalid_report"
    elif not bool(report.get("ok")):
        if _formal_suite_has_toolchain_mismatch(report):
            status = "warn"
            reason = "toolchain_mismatch"
        elif _formal_suite_missing_java_runtime(report):
            status = "fail"
            reason = "java_runtime_missing"
        else:
            status = "fail"
            reason = "gate_failed"

    return {
        "status": status,
        "mode": mode_key,
        "reason": reason,
        "command": command,
        "returncode": int(proc.returncode),
        "stdout_tail": stdout[-2000:],
        "stderr_tail": (proc.stderr or "").strip()[-2000:],
        "report": report,
    }


def _audit_env_and_volume(
    *,
    env_file: Path,
    ext_root: Path,
) -> dict[str, Any]:
    env_values = _load_env_file(env_file)
    missing_keys = [key for key in REQUIRED_ENV_KEYS if not env_values.get(key)]
    ext_mounted = ext_root.exists() and ext_root.is_dir()
    status = "pass"
    if not ext_mounted:
        status = "fail"
    elif not env_file.exists() or missing_keys:
        status = "warn"
    return {
        "status": status,
        "env_file": str(env_file),
        "env_file_exists": env_file.exists(),
        "ext_root": str(ext_root),
        "ext_root_mounted": ext_mounted,
        "missing_env_keys": missing_keys,
        "has_linear_api_key": bool(env_values.get("LINEAR_API_KEY")),
    }


def _runtime_env(env_file: Path) -> dict[str, str]:
    merged = _load_env_file(env_file)
    for key, value in os.environ.items():
        if value:
            merged[key] = value
    return merged


def _audit_storage_layout(env_file: Path) -> dict[str, Any]:
    env_values = _runtime_env(env_file)
    parent_root = resolve_symphony_parent_root(env_values)
    store_root = resolve_symphony_store_root(env_values)
    log_root = symphony_log_root(env_values)
    state_root = symphony_state_root(env_values)
    workspace_root = symphony_workspace_root(env_values)
    durable_root = symphony_durable_root(env_values)
    dlq_file = symphony_dlq_events_file(env_values)
    taste_events_file = symphony_taste_memory_events_file(env_values)
    taste_distillations_dir = symphony_taste_memory_distillations_dir(env_values)
    tool_promotion_events_file = symphony_tool_promotion_events_file(env_values)
    tool_promotion_distillations_dir = symphony_tool_promotion_distillations_dir(
        env_values
    )
    security_events = symphony_security_events_file(env_values)
    api_token_file = symphony_api_token_file(env_values)

    checks = {
        "parent_root_exists": parent_root.exists(),
        "store_under_parent": is_within(store_root, parent_root),
        "logs_under_store": is_within(log_root, store_root),
        "state_under_store": is_within(state_root, store_root),
        "workspaces_under_store": is_within(workspace_root, store_root),
        "durable_under_state": is_within(durable_root, state_root),
        "dlq_under_state": is_within(dlq_file, state_root),
        "taste_events_under_state": is_within(taste_events_file, state_root),
        "taste_distillations_under_state": is_within(
            taste_distillations_dir, state_root
        ),
        "tool_promotion_events_under_state": is_within(
            tool_promotion_events_file, state_root
        ),
        "tool_promotion_distillations_under_state": is_within(
            tool_promotion_distillations_dir, state_root
        ),
        "security_events_under_logs": is_within(security_events, log_root),
        "api_token_under_state": is_within(api_token_file, state_root),
    }
    violations = [name for name, ok in checks.items() if not ok]
    status = "pass" if not violations else "fail"
    return {
        "status": status,
        "parent_root": str(parent_root),
        "parent_root_exists": parent_root.exists(),
        "store_root": str(store_root),
        "store_root_exists": store_root.exists(),
        "project_key": env_values.get("MOLT_SYMPHONY_PROJECT_KEY", "molt"),
        "log_root": str(log_root),
        "state_root": str(state_root),
        "workspace_root": str(workspace_root),
        "durable_root": str(durable_root),
        "dlq_file": str(dlq_file),
        "taste_events_file": str(taste_events_file),
        "taste_distillations_dir": str(taste_distillations_dir),
        "tool_promotion_events_file": str(tool_promotion_events_file),
        "tool_promotion_distillations_dir": str(tool_promotion_distillations_dir),
        "security_events_file": str(security_events),
        "api_token_file": str(api_token_file),
        "checks": checks,
        "violations": violations,
    }


def _audit_dlq_health(
    path: Path,
    *,
    limit: int = 200,
    max_open_failures: int = DEFAULT_MAX_DLQ_OPEN_FAILURES,
    max_recurring: int = DEFAULT_MAX_DLQ_RECURRING_FINGERPRINTS,
) -> dict[str, Any]:
    queue = DeadLetterQueue(path)
    summary = queue.summary(limit=max(limit, 0))
    health = summary.get("health") if isinstance(summary, dict) else {}
    if not isinstance(health, dict):
        health = {}
    open_failure_count = _safe_int(health.get("open_failure_count"))
    recurring_open = health.get("recurring_open_fingerprints")
    recurring_count = len(recurring_open) if isinstance(recurring_open, dict) else 0
    recommended_replay_target = queue.recommended_replay_target(limit=max(limit, 0))
    if open_failure_count > max_open_failures or recurring_count > max_recurring:
        status = "warn"
        reason = "backlog_high"
    elif open_failure_count > 0 or recurring_count > 0:
        status = "warn"
        reason = "backlog_present"
    else:
        status = "pass"
        reason = "clear"
    return {
        "status": status,
        "reason": reason,
        "path": str(path),
        "summary": summary,
        "health": health,
        "recommended_replay_target": recommended_replay_target,
        "max_open_failures": max_open_failures,
        "max_recurring_open_fingerprints": max_recurring,
    }


def _audit_tool_promotion(
    *,
    events_path: Path,
    distillations_dir: Path,
    limit: int = 200,
) -> dict[str, Any]:
    store = ToolPromotionStore(
        events_path=events_path, distillations_dir=distillations_dir
    )
    rows = store.load(limit=max(limit, 0))
    latest = rows[-1] if rows else None
    latest_distillation_path = None
    latest_ready_candidate_count = 0
    latest_candidate_count = 0
    latest_manifest_count = 0
    ready_candidates: list[dict[str, Any]] = []
    manifest_batch: dict[str, Any] | None = None
    if isinstance(latest, dict):
        latest_distillation_path = latest.get("path")
        latest_ready_candidate_count = _safe_int(latest.get("ready_candidate_count"))
        latest_candidate_count = _safe_int(latest.get("candidate_count"))
        latest_manifest_count = _safe_int(latest.get("manifest_count"))
    if isinstance(latest_distillation_path, str) and latest_distillation_path:
        distillation_path = Path(latest_distillation_path).expanduser().resolve()
        if distillation_path.exists():
            try:
                payload = json.loads(distillation_path.read_text(encoding="utf-8"))
            except Exception:
                payload = None
            if isinstance(payload, dict):
                ready_payload = payload.get("ready_candidates")
                if isinstance(ready_payload, list):
                    ready_candidates = [
                        row for row in ready_payload if isinstance(row, dict)
                    ][:10]
                batch = payload.get("manifest_batch")
                if isinstance(batch, dict):
                    manifest_batch = batch
                    latest_manifest_count = _safe_int(batch.get("manifest_count"))
    if latest_ready_candidate_count > 0:
        status = "pass"
        reason = "ready_candidates_present"
    elif latest_candidate_count > 0:
        status = "info"
        reason = "candidates_present_not_ready"
    elif rows:
        status = "info"
        reason = "history_present"
    else:
        status = "info"
        reason = "no_candidates_yet"
    return {
        "status": status,
        "reason": reason,
        "events_path": str(events_path),
        "distillations_dir": str(distillations_dir),
        "event_count": len(rows),
        "latest": latest,
        "latest_distillation_path": latest_distillation_path,
        "latest_candidate_count": latest_candidate_count,
        "latest_ready_candidate_count": latest_ready_candidate_count,
        "latest_manifest_count": latest_manifest_count,
        "ready_candidates": ready_candidates,
        "manifest_batch": manifest_batch,
    }


def _record_finding(
    findings: list[dict[str, Any]],
    *,
    severity: str,
    code: str,
    message: str,
    details: Any = None,
) -> None:
    row: dict[str, Any] = {"severity": severity, "code": code, "message": message}
    if details is not None:
        row["details"] = details
    findings.append(row)


def _collect_findings(report: dict[str, Any]) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    env = report["sections"]["environment"]
    if not bool(env.get("ext_root_mounted")):
        _record_finding(
            findings,
            severity="fail",
            code="ext_root_missing",
            message="External volume root is not mounted.",
            details=env.get("ext_root"),
        )
    if bool(env.get("missing_env_keys")):
        _record_finding(
            findings,
            severity="warn",
            code="env_missing_keys",
            message="Runtime env file is missing required Symphony keys.",
            details=env.get("missing_env_keys"),
        )
    if not bool(env.get("has_linear_api_key")):
        _record_finding(
            findings,
            severity="fail",
            code="linear_api_key_missing",
            message="LINEAR_API_KEY is missing; Linear audit and orchestration cannot run.",
        )

    storage = report["sections"].get("storage_layout") or {}
    if storage.get("violations"):
        _record_finding(
            findings,
            severity="fail",
            code="symphony_storage_layout_invalid",
            message=(
                "Symphony long-lived logs/state are not isolated under the canonical "
                "shared Symphony parent/store root."
            ),
            details={
                "parent_root": storage.get("parent_root"),
                "store_root": storage.get("store_root"),
                "violations": storage.get("violations"),
            },
        )

    docs = report["sections"]["docs_and_tools"]
    if docs.get("missing_docs"):
        _record_finding(
            findings,
            severity="fail",
            code="docs_missing",
            message="Required Symphony docs are missing.",
            details=docs["missing_docs"],
        )
    if docs.get("missing_tools"):
        _record_finding(
            findings,
            severity="fail",
            code="tools_missing",
            message="Required Symphony harness tools are missing.",
            details=docs["missing_tools"],
        )
    if not bool(docs.get("has_human_authority_gate")):
        _record_finding(
            findings,
            severity="warn",
            code="human_gate_missing",
            message="Human authority/escalation gate is not explicit in docs.",
        )
    else:
        _record_finding(
            findings,
            severity="info",
            code="human_gate_present",
            message=(
                "Human authority gate is explicitly present; full zero-human operation "
                "is intentionally not the current governance model."
            ),
        )

    harness = (report.get("sections") or {}).get("harness_engineering") or {}
    missing_harness_artifacts = harness.get("missing_artifacts") or []
    critical_harness_artifacts = harness.get("critical_missing_artifacts") or []
    if missing_harness_artifacts:
        _record_finding(
            findings,
            severity="fail" if critical_harness_artifacts else "warn",
            code="harness_artifacts_missing",
            message=(
                "Harness engineering artifacts are missing; recursive improvement "
                "infrastructure is incomplete."
            ),
            details={
                "missing": missing_harness_artifacts,
                "critical_missing": critical_harness_artifacts,
            },
        )
    missing_principles = harness.get("missing_principles") or []
    if missing_principles:
        _record_finding(
            findings,
            severity="warn",
            code="harness_principles_missing",
            message=(
                "Harness engineering document is missing one or more required "
                "principle mappings."
            ),
            details=missing_principles,
        )
    score = harness.get("score")
    target_score = harness.get("target_score")
    if isinstance(score, int):
        if score < int(target_score or 90):
            _record_finding(
                findings,
                severity="warn",
                code="harness_score_below_target",
                message=(
                    "Harness engineering score is below target; strengthen artifacts "
                    "and principle coverage before autonomous scale-up."
                ),
                details={"score": score, "target_score": target_score},
            )
        else:
            _record_finding(
                findings,
                severity="info",
                code="harness_score_meets_target",
                message="Harness engineering score meets target.",
                details={"score": score, "target_score": target_score},
            )

    dspy = (report.get("sections") or {}).get("dspy_routing") or {}
    if bool(dspy.get("enabled")):
        if dspy.get("status") != "pass":
            _record_finding(
                findings,
                severity="warn",
                code="dspy_routing_not_ready",
                message=(
                    "DSPy routing is enabled but not fully configured; linear_hygiene "
                    "will fall back to heuristic routing."
                ),
                details={
                    "reason": dspy.get("reason"),
                    "model_configured": dspy.get("model_configured"),
                    "api_key_present": dspy.get("api_key_present"),
                    "module_available": dspy.get("module_available"),
                    "pydantic_available": dspy.get("pydantic_available"),
                    "api_key_env": dspy.get("api_key_env"),
                },
            )
        else:
            _record_finding(
                findings,
                severity="info",
                code="dspy_routing_ready",
                message="DSPy routing is enabled and ready for linear_hygiene.",
                details={
                    "model": dspy.get("model"),
                    "api_key_env": dspy.get("api_key_env"),
                },
            )
    else:
        _record_finding(
            findings,
            severity="info",
            code="dspy_routing_disabled",
            message=(
                "DSPy routing is disabled; linear_hygiene uses deterministic "
                "heuristic routing."
            ),
        )

    launchd = report["sections"]["launchd"]
    if not bool(launchd.get("main_loaded")):
        _record_finding(
            findings,
            severity="warn",
            code="launchd_main_not_loaded",
            message="Main Symphony launchd service is not loaded.",
        )
    if not bool(launchd.get("watchdog_loaded")):
        _record_finding(
            findings,
            severity="warn",
            code="launchd_watchdog_not_loaded",
            message="Symphony watchdog launchd service is not loaded.",
        )

    durable = report["sections"]["durable_memory"]
    checks = durable.get("checks") or {}
    jsonl_check = checks.get("jsonl_readable") or {}
    if not bool(jsonl_check.get("ok")):
        _record_finding(
            findings,
            severity="fail",
            code="durable_jsonl_unreadable",
            message="Durable JSONL store is unreadable.",
            details=jsonl_check,
        )
    duckdb_check = checks.get("duckdb_readable") or {}
    if bool(duckdb_check.get("warning")):
        _record_finding(
            findings,
            severity="warn",
            code="durable_duckdb_locked",
            message=(
                "Durable DuckDB file is lock-contended by active writer; read path is "
                "currently in warning-only mode."
            ),
            details=duckdb_check.get("reason"),
        )
    if not bool(duckdb_check.get("ok")):
        _record_finding(
            findings,
            severity="fail",
            code="durable_duckdb_unreadable",
            message="Durable DuckDB store is unreadable.",
            details=duckdb_check,
        )

    dlq = (report.get("sections") or {}).get("dlq_health") or {}
    dlq_health = dlq.get("health") if isinstance(dlq, dict) else {}
    if not isinstance(dlq_health, dict):
        dlq_health = {}
    open_failure_count = _safe_int(dlq_health.get("open_failure_count"))
    recurring_open = dlq_health.get("recurring_open_fingerprints")
    recurring_open_count = (
        len(recurring_open) if isinstance(recurring_open, dict) else 0
    )
    if open_failure_count > _safe_int(dlq.get("max_open_failures"), default=3):
        _record_finding(
            findings,
            severity="warn",
            code="dlq_backlog_high",
            message="DLQ open-failure backlog is above the configured threshold.",
            details={
                "open_failure_count": open_failure_count,
                "max_open_failures": dlq.get("max_open_failures"),
                "latest_open_failure": dlq_health.get("latest_open_failure"),
            },
        )
    elif open_failure_count > 0:
        _record_finding(
            findings,
            severity="warn",
            code="dlq_backlog_present",
            message="DLQ contains unresolved recursive-loop failures awaiting replay or repair.",
            details={
                "open_failure_count": open_failure_count,
                "latest_open_failure": dlq_health.get("latest_open_failure"),
            },
        )
    else:
        _record_finding(
            findings,
            severity="info",
            code="dlq_backlog_clear",
            message="DLQ has no unresolved recursive-loop failures.",
        )
    if recurring_open_count > 0:
        _record_finding(
            findings,
            severity="warn",
            code="dlq_recurring_failures",
            message="DLQ contains recurring unresolved failure fingerprints.",
            details={
                "recurring_open_fingerprints": recurring_open,
                "max_recurring_open_fingerprints": dlq.get(
                    "max_recurring_open_fingerprints"
                ),
            },
        )
    replay_success_count = _safe_int(dlq_health.get("replay_success_count"))
    replay_failure_count = _safe_int(dlq_health.get("replay_failure_count"))
    if replay_failure_count > replay_success_count and replay_failure_count > 0:
        _record_finding(
            findings,
            severity="warn",
            code="dlq_replay_health_low",
            message="DLQ replay attempts are failing more often than they are succeeding.",
            details={
                "replay_success_count": replay_success_count,
                "replay_failure_count": replay_failure_count,
                "recommended_replay_target": dlq.get("recommended_replay_target"),
            },
        )
    if replay_success_count > 0:
        _record_finding(
            findings,
            severity="info",
            code="dlq_replay_success_present",
            message="DLQ replay history includes successful recovery attempts.",
            details={"replay_success_count": replay_success_count},
        )

    tool_promotion = (report.get("sections") or {}).get("tool_promotion") or {}
    ready_candidate_count = _safe_int(
        tool_promotion.get("latest_ready_candidate_count")
    )
    latest_candidate_count = _safe_int(tool_promotion.get("latest_candidate_count"))
    if ready_candidate_count > 0:
        _record_finding(
            findings,
            severity="info",
            code="tool_promotion_candidates_ready",
            message="Tool-promotion distillation has ready candidates for explicit extraction.",
            details={
                "ready_candidate_count": ready_candidate_count,
                "latest_distillation_path": tool_promotion.get(
                    "latest_distillation_path"
                ),
            },
        )
    elif latest_candidate_count > 0:
        _record_finding(
            findings,
            severity="info",
            code="tool_promotion_candidates_observed",
            message="Tool-promotion distillation has emerging candidates that are not ready yet.",
            details={
                "candidate_count": latest_candidate_count,
                "latest_distillation_path": tool_promotion.get(
                    "latest_distillation_path"
                ),
            },
        )
    latest_manifest_count = _safe_int(tool_promotion.get("latest_manifest_count"))
    if latest_manifest_count > 0:
        _record_finding(
            findings,
            severity="info",
            code="tool_promotion_manifests_ready",
            message="Tool-promotion manifests were generated for reviewable candidate promotion.",
            details={
                "latest_manifest_count": latest_manifest_count,
                "manifests_dir": (tool_promotion.get("manifest_batch") or {}).get(
                    "manifests_dir"
                )
                if isinstance(tool_promotion.get("manifest_batch"), dict)
                else None,
            },
        )

    trend = (report.get("sections") or {}).get("trend_analysis") or {}
    if trend:
        harness = trend.get("harness") if isinstance(trend, dict) else None
        if isinstance(harness, dict) and bool(harness.get("regressed")):
            _record_finding(
                findings,
                severity="warn",
                code="harness_score_regressed",
                message=(
                    "Harness engineering score regressed vs previous baseline window."
                ),
                details={
                    "latest": harness.get("latest"),
                    "previous": harness.get("previous"),
                    "delta": harness.get("delta"),
                    "max_harness_score_drop": trend.get("max_harness_score_drop"),
                },
            )

        active_flow = trend.get("active_flow") if isinstance(trend, dict) else None
        if isinstance(active_flow, dict) and bool(active_flow.get("ratio_low")):
            _record_finding(
                findings,
                severity="warn",
                code="active_flow_ratio_low",
                message=(
                    "Active execution flow ratio is below trend threshold across "
                    "recent readiness snapshots."
                ),
                details={
                    "ratio": active_flow.get("ratio"),
                    "min_active_flow_ratio": trend.get("min_active_flow_ratio"),
                    "window": trend.get("window"),
                    "point_count": trend.get("point_count"),
                },
            )

        formal_trend = trend.get("formal_suite") if isinstance(trend, dict) else None
        if isinstance(formal_trend, dict) and bool(formal_trend.get("ratio_low")):
            _record_finding(
                findings,
                severity="warn",
                code="formal_pass_ratio_low",
                message=(
                    "Formal-suite pass ratio is below trend threshold across recent "
                    "readiness snapshots."
                ),
                details={
                    "pass_ratio": formal_trend.get("pass_ratio"),
                    "min_formal_pass_ratio": trend.get("min_formal_pass_ratio"),
                    "window": trend.get("window"),
                    "point_count": trend.get("point_count"),
                },
            )

        durable_trend = trend.get("durable_growth") if isinstance(trend, dict) else None
        if isinstance(durable_trend, dict) and bool(durable_trend.get("recurring")):
            _record_finding(
                findings,
                severity="warn",
                code="durable_growth_recurring",
                message=(
                    "Durable artifact growth breaches are recurring across recent "
                    "baseline windows."
                ),
                details={
                    "breach_count": len(durable_trend.get("breaches") or []),
                    "breach_interval_count": durable_trend.get("breach_interval_count"),
                    "max_durable_growth_ratio": trend.get("max_durable_growth_ratio"),
                    "window": trend.get("window"),
                },
            )
        if trend.get("status") == "pass":
            _record_finding(
                findings,
                severity="info",
                code="readiness_trend_stable",
                message=(
                    "Readiness trend checks are stable across the configured "
                    "historical window."
                ),
                details={
                    "window": trend.get("window"),
                    "point_count": trend.get("point_count"),
                },
            )

    manifest = report["sections"]["manifest_index"]
    if manifest.get("missing_manifest_files"):
        _record_finding(
            findings,
            severity="fail",
            code="manifest_files_missing",
            message="Linear manifest index contains missing/invalid manifest files.",
            details=manifest["missing_manifest_files"],
        )
    malformed_manifest_titles = manifest.get("malformed_titles") or []
    if malformed_manifest_titles:
        _record_finding(
            findings,
            severity="warn",
            code="manifest_titles_malformed",
            message="Linear manifests include malformed titles that reduce issue quality.",
            details={
                "count": len(malformed_manifest_titles),
                "sample": malformed_manifest_titles[:10],
            },
        )
    metadata_gaps = manifest.get("metadata_gaps") or []
    if metadata_gaps:
        _record_finding(
            findings,
            severity="warn",
            code="manifest_metadata_gaps",
            message="Linear manifests include entries missing canonical metadata keys.",
            details={"count": len(metadata_gaps), "sample": metadata_gaps[:10]},
        )

    linear = report["sections"]["linear_workspace"]
    if linear.get("status") == "fail":
        _record_finding(
            findings,
            severity="fail",
            code="linear_audit_failed",
            message="Linear workspace audit call failed.",
            details=linear.get("error"),
        )
    else:
        if linear.get("missing_project"):
            _record_finding(
                findings,
                severity="warn",
                code="linear_missing_project",
                message="Linear issues exist without project assignment.",
                details={
                    "count": len(linear["missing_project"]),
                    "sample": linear["missing_project"][:20],
                },
            )
        if linear.get("seeded_missing_metadata"):
            _record_finding(
                findings,
                severity="warn",
                code="linear_seeded_metadata_gaps",
                message=(
                    "Auto-seeded Linear issues are missing canonical metadata block keys."
                ),
                details={
                    "count": len(linear["seeded_missing_metadata"]),
                    "sample": linear["seeded_missing_metadata"][:20],
                },
            )
        if linear.get("malformed_titles"):
            _record_finding(
                findings,
                severity="warn",
                code="linear_titles_malformed",
                message="Linear issues include malformed titles.",
                details={
                    "count": len(linear["malformed_titles"]),
                    "sample": linear["malformed_titles"][:20],
                },
            )
        if not bool(linear.get("active_execution_flow")):
            _record_finding(
                findings,
                severity="warn",
                code="linear_no_active_flow",
                message=(
                    "No issues are currently in In Progress or In Review; execution flow "
                    "is queue-heavy and may stall recursive learning."
                ),
                details=linear.get("state_counts"),
            )
        if int(linear.get("label_count", 0)) <= 3:
            _record_finding(
                findings,
                severity="warn",
                code="linear_labels_minimal",
                message=(
                    "Linear label taxonomy is minimal; add operational labels "
                    "(area/*, risk/*, evidence/*, blocker/*) for stronger automation routing."
                ),
                details={"label_count": linear.get("label_count")},
            )

    cli_compat = report["sections"]["linear_cli_compat"]
    if not bool(cli_compat.get("lin_installed")):
        _record_finding(
            findings,
            severity="info",
            code="lin_not_installed",
            message=(
                "lin CLI is not installed; use tools/linear_workspace.py for all "
                "automation operations."
            ),
        )
    elif cli_compat.get("reason") == "schema_missing_project_milestone":
        _record_finding(
            findings,
            severity="info",
            code="lin_schema_incompatible",
            message=(
                "lin CLI appears incompatible with current Linear schema "
                "(Project.milestone missing). Use tools/linear_workspace.py."
            ),
            details=cli_compat,
        )
    elif cli_compat.get("status") != "pass":
        _record_finding(
            findings,
            severity="info",
            code="lin_health_warn",
            message="lin CLI compatibility probe reported warnings.",
            details=cli_compat,
        )

    formal = report["sections"]["formal_suite"]
    if formal.get("status") == "fail":
        if formal.get("reason") == "java_runtime_missing":
            _record_finding(
                findings,
                severity="fail",
                code="formal_suite_java_runtime_missing",
                message=(
                    "Formalization suite failed because Java runtime is missing for "
                    "Quint/Apalache verification."
                ),
                details={
                    "mode": formal.get("mode"),
                    "reason": formal.get("reason"),
                    "returncode": formal.get("returncode"),
                },
            )
        else:
            _record_finding(
                findings,
                severity="fail",
                code="formal_suite_failed",
                message="Formalization suite gate failed.",
                details={
                    "mode": formal.get("mode"),
                    "reason": formal.get("reason"),
                    "returncode": formal.get("returncode"),
                },
            )
    elif formal.get("status") == "warn":
        _record_finding(
            findings,
            severity="warn",
            code="formal_suite_toolchain_mismatch",
            message=(
                "Formalization suite reported toolchain mismatch (likely Node/Quint "
                "runtime incompatibility); verification signal is degraded."
            ),
            details={
                "mode": formal.get("mode"),
                "reason": formal.get("reason"),
                "returncode": formal.get("returncode"),
            },
        )
    elif formal.get("status") == "pass":
        _record_finding(
            findings,
            severity="info",
            code="formal_suite_pass",
            message="Formalization suite check passed for current mode.",
            details={"mode": formal.get("mode")},
        )
        formal_report = formal.get("report")
        if isinstance(formal_report, dict):
            checks = formal_report.get("checks")
            quint = checks.get("quint") if isinstance(checks, dict) else None
            diagnostics = quint.get("diagnostics") if isinstance(quint, dict) else None
            if isinstance(diagnostics, dict) and bool(diagnostics.get("fallback_used")):
                _record_finding(
                    findings,
                    severity="info",
                    code="formal_suite_fallback_used",
                    message=(
                        "Quint formal checks required Node fallback execution; "
                        "formal signal is preserved but toolchain mismatch remains."
                    ),
                    details={
                        "node": diagnostics.get("node"),
                        "fallback_prefix": diagnostics.get("fallback_prefix"),
                    },
                )
            node_info = (
                diagnostics.get("node") if isinstance(diagnostics, dict) else None
            )
            node_major = (
                int(node_info.get("major"))
                if isinstance(node_info, dict)
                and isinstance(node_info.get("major"), int)
                else None
            )
            if isinstance(node_major, int) and node_major >= 25:
                _record_finding(
                    findings,
                    severity="warn",
                    code="formal_suite_node_major_mismatch",
                    message=(
                        "System Node version is >=25; Quint currently requires fallback "
                        "to preserve formal-suite signal."
                    ),
                    details={
                        "node": node_info,
                        "fallback_prefix": diagnostics.get("fallback_prefix")
                        if isinstance(diagnostics, dict)
                        else None,
                    },
                )
    return findings


def _apply_strict_autonomy(findings: list[dict[str, Any]]) -> list[dict[str, Any]]:
    normalized: list[dict[str, Any]] = []
    for item in findings:
        row = dict(item)
        code = str(row.get("code") or "")
        severity = str(row.get("severity") or "info")
        if code in STRICT_AUTONOMY_FAIL_CODES and severity == "warn":
            row["severity"] = "fail"
            row["strict_autonomy_promoted"] = True
        normalized.append(row)
    return normalized


def _overall_status(findings: list[dict[str, Any]]) -> str:
    severities = {str(item.get("severity")) for item in findings}
    if "fail" in severities:
        return "fail"
    if "warn" in severities:
        return "warn"
    return "pass"


def _linear_priority_value(priority_label: str) -> int | None:
    mapping = {"P0": 1, "P1": 2, "P2": 3, "P3": 4}
    return mapping.get(priority_label.strip().upper())


def _truncate_text(value: str, *, limit: int = 72) -> str:
    compact = re.sub(r"\s+", " ", value.strip())
    if len(compact) <= limit:
        return compact
    return compact[: max(limit - 3, 0)].rstrip() + "..."


def _dlq_replay_command_for_report(report: dict[str, Any]) -> str | None:
    section = (report.get("sections") or {}).get("dlq_health") or {}
    target = (
        section.get("recommended_replay_target") if isinstance(section, dict) else None
    )
    if not isinstance(target, dict):
        return None
    fingerprint = str(target.get("fingerprint") or "").strip()
    if not fingerprint:
        return None
    return (
        "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py replay "
        f"--fingerprint {fingerprint} --dry-run"
    )


def _build_improvement_issue_specs(
    report: dict[str, Any],
    *,
    max_tool_candidates: int = 3,
) -> list[dict[str, Any]]:
    findings = report.get("findings")
    finding_codes = (
        {str(row.get("code") or "") for row in findings if isinstance(row, dict)}
        if isinstance(findings, list)
        else set()
    )
    specs: list[dict[str, Any]] = []

    dlq_section = (report.get("sections") or {}).get("dlq_health") or {}
    dlq_target = (
        dlq_section.get("recommended_replay_target")
        if isinstance(dlq_section, dict)
        else None
    )
    if finding_codes & {
        "dlq_backlog_high",
        "dlq_backlog_present",
        "dlq_recurring_failures",
        "dlq_replay_health_low",
    }:
        fingerprint = (
            str(dlq_target.get("fingerprint") or "").strip()
            if isinstance(dlq_target, dict)
            else ""
        )
        name = (
            str(dlq_target.get("name") or "").strip()
            if isinstance(dlq_target, dict)
            else ""
        )
        replay_cmd = _dlq_replay_command_for_report(report)
        description_lines = [
            "Auto-seeded from Symphony readiness improvement loop.",
            "",
            "- area: tooling",
            "- owner: symphony",
            "- milestone: TL2",
            "- priority: P1",
            "- status: planned",
            "- source: tools/symphony_readiness_audit.py",
            "",
            "Symphony readiness detected unresolved DLQ backlog that should be replayed or repaired.",
        ]
        if fingerprint:
            description_lines.append(f"- recommended fingerprint: `{fingerprint}`")
        if name:
            description_lines.append(f"- latest failing step: `{name}`")
        description_lines.append(
            f"- dlq path: `{str(dlq_section.get('path') or '').strip()}`"
        )
        if replay_cmd:
            description_lines.append(f"- replay command: `{replay_cmd}`")
        specs.append(
            {
                "title": "[P1][TL2] Symphony DLQ replay backlog",
                "description": "\n".join(description_lines).strip(),
                "priority": _linear_priority_value("P1"),
                "canonical_key": "symphony-dlq-replay-backlog",
            }
        )

    tool_section = (report.get("sections") or {}).get("tool_promotion") or {}
    ready_candidates = (
        tool_section.get("ready_candidates") if isinstance(tool_section, dict) else None
    )
    manifest_batch = (
        tool_section.get("manifest_batch") if isinstance(tool_section, dict) else None
    )
    manifests_by_id: dict[str, str] = {}
    if isinstance(manifest_batch, dict):
        raw_manifests = manifest_batch.get("manifests")
        if isinstance(raw_manifests, list):
            for row in raw_manifests:
                if not isinstance(row, dict):
                    continue
                candidate_id = str(row.get("candidate_id") or "").strip()
                path = str(row.get("path") or "").strip()
                if candidate_id and path:
                    manifests_by_id[candidate_id] = path
    if "tool_promotion_candidates_ready" in finding_codes and isinstance(
        ready_candidates, list
    ):
        for candidate in [row for row in ready_candidates if isinstance(row, dict)][
            : max(0, int(max_tool_candidates))
        ]:
            candidate_id = str(candidate.get("candidate_id") or "").strip()
            command = str(candidate.get("command") or "").strip()
            if not candidate_id or not command:
                continue
            manifest_path = manifests_by_id.get(candidate_id, "")
            description_lines = [
                "Auto-seeded from Symphony readiness improvement loop.",
                "",
                "- area: tooling",
                "- owner: symphony",
                "- milestone: TL2",
                "- priority: P2",
                "- status: planned",
                "- source: tools/symphony_readiness_audit.py",
                "",
                "Symphony observed a recurring successful action that is ready for explicit promotion.",
                f"- candidate id: `{candidate_id}`",
                f"- command: `{command}`",
                f"- success count: `{int(candidate.get('success_count') or 0)}`",
            ]
            if manifest_path:
                description_lines.append(f"- manifest: `{manifest_path}`")
            title = f"[P2][TL2] Symphony tool promotion: {_truncate_text(candidate_id, limit=48)}"
            specs.append(
                {
                    "title": title,
                    "description": "\n".join(description_lines).strip(),
                    "priority": _linear_priority_value("P2"),
                    "canonical_key": f"symphony-tool-promotion-{candidate_id}",
                }
            )
    return specs


def _sync_improvement_issues(
    *,
    report: dict[str, Any],
    team: str,
    env_file: Path,
    project_ref: str | None,
    apply: bool,
    max_tool_candidates: int,
) -> dict[str, Any]:
    desired_issues = _build_improvement_issue_specs(
        report,
        max_tool_candidates=max_tool_candidates,
    )
    if not desired_issues:
        return {
            "status": "skipped",
            "reason": "no_desired_issues",
            "desired_issue_count": 0,
            "create_count": 0,
            "update_count": 0,
            "skip_count": 0,
            "errors": [],
            "desired_issues": [],
        }

    api_key = _resolve_linear_api_key(env_file)  # secret-guard: allow
    if not api_key:
        return {
            "status": "skipped",
            "reason": "missing_linear_api_key",
            "desired_issue_count": len(desired_issues),
            "create_count": 0,
            "update_count": 0,
            "skip_count": 0,
            "errors": [],
            "desired_issues": desired_issues,
        }

    try:
        with _temporary_linear_api_key(api_key):
            team_id = linear_workspace._resolve_team_id(team)
            project_id = (
                linear_workspace._resolve_project_id(team_id, project_ref)
                if project_ref
                else None
            )
            existing_issues = linear_workspace._fetch_issues(team_id, project_id)

            issues_by_title = {
                linear_workspace._title_key(str(issue.get("title") or "")): issue
                for issue in existing_issues
                if isinstance(issue, dict) and str(issue.get("title") or "").strip()
            }

            creates: list[dict[str, Any]] = []
            updates: list[dict[str, Any]] = []
            skipped: list[dict[str, Any]] = []
            errors: list[str] = []

            for spec in desired_issues:
                title = str(spec.get("title") or "").strip()
                description = str(spec.get("description") or "").strip()
                priority = spec.get("priority")
                key = linear_workspace._title_key(title)
                existing = issues_by_title.get(key)
                if existing is None:
                    planned = {
                        "title": title,
                        "priority": priority,
                        "project_ref": project_ref,
                        "canonical_key": spec.get("canonical_key"),
                    }
                    if not apply:
                        creates.append(planned)
                        continue
                    input_payload: dict[str, Any] = {
                        "teamId": team_id,
                        "title": title,
                        "description": description,
                    }
                    if isinstance(priority, int):
                        input_payload["priority"] = priority
                    if project_id:
                        input_payload["projectId"] = project_id
                    try:
                        data = linear_workspace.graphql(
                            linear_workspace.MUTATION_ISSUE_CREATE,
                            {"input": input_payload},
                        )
                        result = data["issueCreate"]
                        if not result.get("success"):
                            raise RuntimeError("issueCreate returned success=false")
                        creates.append(result["issue"])
                    except Exception as exc:
                        errors.append(f"create:{title}:{exc}")
                    continue

                existing_description = str(existing.get("description") or "").strip()
                existing_priority = existing.get("priority")
                existing_project = existing.get("project")
                existing_project_id = (
                    str(existing_project.get("id") or "").strip()
                    if isinstance(existing_project, dict)
                    else ""
                )
                input_payload: dict[str, Any] = {}
                if existing_description != description:
                    input_payload["description"] = description
                if isinstance(priority, int) and priority != existing_priority:
                    input_payload["priority"] = priority
                if project_id and project_id != existing_project_id:
                    input_payload["projectId"] = project_id
                if not input_payload:
                    skipped.append(
                        {
                            "issue": str(
                                existing.get("identifier") or existing.get("id") or ""
                            ),
                            "title": title,
                            "canonical_key": spec.get("canonical_key"),
                        }
                    )
                    continue
                if not apply:
                    updates.append(
                        {
                            "issue": str(
                                existing.get("identifier") or existing.get("id") or ""
                            ),
                            "title": title,
                            "fields": sorted(input_payload.keys()),
                            "canonical_key": spec.get("canonical_key"),
                        }
                    )
                    continue
                try:
                    data = linear_workspace.graphql(
                        linear_workspace.MUTATION_ISSUE_UPDATE,
                        {"id": str(existing["id"]), "input": input_payload},
                    )
                    result = data["issueUpdate"]
                    if not result.get("success"):
                        raise RuntimeError("issueUpdate returned success=false")
                    updates.append(result["issue"])
                except Exception as exc:
                    errors.append(f"update:{title}:{exc}")
    except Exception as exc:
        return {
            "status": "error",
            "reason": "linear_sync_failed",
            "project_ref": project_ref,
            "desired_issue_count": len(desired_issues),
            "create_count": 0,
            "update_count": 0,
            "skip_count": 0,
            "errors": [str(exc)],
            "desired_issues": desired_issues,
        }

    return {
        "status": "applied" if apply else "dry_run",
        "reason": "ok",
        "project_ref": project_ref,
        "desired_issue_count": len(desired_issues),
        "create_count": len(creates),
        "update_count": len(updates),
        "skip_count": len(skipped),
        "errors": errors,
        "desired_issues": desired_issues,
        "creates": creates,
        "updates": updates,
        "skipped": skipped,
    }


def _synthesize_next_tranche(report: dict[str, Any]) -> dict[str, Any]:
    findings = report.get("findings") or []
    actionable = [
        row
        for row in findings
        if str(row.get("severity") or "info") in {"fail", "warn"}
    ]
    action_specs: dict[str, dict[str, Any]] = {
        "ext_root_missing": {
            "id": "restore_external_volume",
            "priority": "P0",
            "title": "Restore external artifact volume mount",
            "why": "Build/test evidence loops are blocked without external artifact routing.",
            "commands": [
                "tools/throughput_env.sh --apply",
            ],
        },
        "env_missing_keys": {
            "id": "repair_runtime_env_contract",
            "priority": "P0",
            "title": "Repair Symphony runtime env contract",
            "why": "Missing env keys can silently break autonomous orchestration lanes.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_bootstrap.py --sync-env",
            ],
        },
        "linear_api_key_missing": {
            "id": "restore_linear_auth",
            "priority": "P0",
            "title": "Restore Linear API auth in runtime env",
            "why": "Linear hygiene and dispatch cannot run without API auth.",
            "commands": [
                "grep '^LINEAR_API_KEY=' ops/linear/runtime/symphony.env",
            ],
        },
        "symphony_storage_layout_invalid": {
            "id": "repair_symphony_storage_layout",
            "priority": "P0",
            "title": "Repair canonical Symphony storage layout",
            "why": "Long-lived Symphony logs/state must live under a project-specific store root, not a sibling project tree.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_bootstrap.py --sync-env",
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --strict-autonomy --fail-on warn",
            ],
        },
        "dspy_routing_not_ready": {
            "id": "stabilize_dspy_routing",
            "priority": "P1",
            "title": "Stabilize DSPy routing readiness",
            "why": "DSPy route selection should be either fully ready or explicitly disabled.",
            "commands": [
                "uv sync --group dev --python 3.12",
                "PYTHONPATH=src uv run --group dev --python 3.12 python3 tools/linear_hygiene.py apply-routing --team Moltlang --apply",
            ],
        },
        "harness_score_below_target": {
            "id": "raise_harness_score",
            "priority": "P0",
            "title": "Raise harness score to target (>=90)",
            "why": "Harness score below target weakens recursive improvement reliability.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --strict-autonomy --fail-on warn",
            ],
        },
        "harness_score_regressed": {
            "id": "arrest_harness_regression",
            "priority": "P0",
            "title": "Arrest harness score regression trend",
            "why": "Negative harness trajectory indicates governance or tooling drift.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --strict-autonomy --fail-on warn",
            ],
        },
        "active_flow_ratio_low": {
            "id": "restore_execution_flow",
            "priority": "P1",
            "title": "Restore active execution flow ratio",
            "why": "Queue-heavy states reduce throughput and learning feedback loops.",
            "commands": [
                "PYTHONPATH=src uv run --group dev --python 3.12 python3 tools/linear_hygiene.py ensure-active-flow --team Moltlang --apply",
            ],
        },
        "formal_pass_ratio_low": {
            "id": "stabilize_formal_lane",
            "priority": "P1",
            "title": "Stabilize formal verification pass ratio",
            "why": "Repeated formal-suite failures reduce trust in autonomy-critical changes.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/check_formal_methods.py --json-only",
            ],
        },
        "durable_growth_budget_exceeded": {
            "id": "trim_durable_growth",
            "priority": "P1",
            "title": "Trim durable memory growth budget breach",
            "why": "Unbounded durable growth can degrade observability and storage health.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py prune --keep-latest 20 --max-age-days 30",
            ],
        },
        "durable_growth_recurring": {
            "id": "stabilize_durable_growth_trend",
            "priority": "P1",
            "title": "Stabilize recurring durable growth trend",
            "why": "Recurring growth breaches indicate systemic telemetry retention drift.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py check",
            ],
        },
        "durable_duckdb_locked": {
            "id": "reduce_duckdb_lock_contention",
            "priority": "P2",
            "title": "Reduce DuckDB writer lock contention",
            "why": "Persistent lock contention can hide durability health regressions.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_durable_admin.py summary",
            ],
        },
        "dlq_backlog_high": {
            "id": "drain_dlq_backlog",
            "priority": "P0",
            "title": "Drain DLQ backlog and replay failed autonomy steps",
            "why": "Unresolved recursive-loop failures reduce loop closure and compound drift.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20",
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py replay --fingerprint <fingerprint> --dry-run",
            ],
        },
        "dlq_backlog_present": {
            "id": "triage_dlq_backlog",
            "priority": "P1",
            "title": "Triage unresolved DLQ failures",
            "why": "A small unresolved DLQ backlog still blocks deterministic self-repair learning.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20",
            ],
        },
        "dlq_recurring_failures": {
            "id": "stabilize_recurring_dlq_failures",
            "priority": "P1",
            "title": "Stabilize recurring DLQ fingerprints",
            "why": "Recurring unresolved failures indicate a missing repair, retry, or tool abstraction.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20",
            ],
        },
        "dlq_replay_health_low": {
            "id": "repair_dlq_replay_health",
            "priority": "P1",
            "title": "Repair DLQ replay health",
            "why": "Replay attempts are failing more often than succeeding, so the self-repair loop is not compounding yet.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_dlq.py summary --limit 20",
            ],
        },
        "tool_promotion_candidates_ready": {
            "id": "promote_recurring_actions_to_tools",
            "priority": "P1",
            "title": "Promote recurring successful actions into explicit tools or hooks",
            "why": "Repeated successful shell actions should become durable Symphony capabilities.",
            "commands": [
                "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_tool_promotion.py distill --limit 200 --min-success-count 3",
            ],
        },
    }

    chosen: dict[str, dict[str, Any]] = {}
    for row in actionable:
        code = str(row.get("code") or "")
        spec = action_specs.get(code)
        if spec is None:
            continue
        key = str(spec["id"])
        existing = chosen.get(key)
        if existing is None:
            chosen[key] = dict(spec)
            chosen[key]["trigger_codes"] = [code]
        else:
            existing_codes = existing.get("trigger_codes") or []
            if code not in existing_codes:
                existing_codes.append(code)
                existing["trigger_codes"] = existing_codes

    actions = list(chosen.values())
    if not actions:
        actions.append(
            {
                "id": "maintain_green_baseline",
                "priority": "P2",
                "title": "Maintain green readiness baseline and expand optimization lanes",
                "why": "System is stable; use spare capacity for proactive improvements.",
                "commands": [
                    "PYTHONPATH=src uv run --python 3.12 python3 tools/symphony_readiness_audit.py --strict-autonomy --fail-on warn --formal-suite all",
                ],
                "trigger_codes": [],
            }
        )

    actions.sort(
        key=lambda row: (
            _PRIORITY_ORDER.get(str(row.get("priority") or "P3"), 3),
            str(row.get("title") or ""),
        )
    )
    replay_command = _dlq_replay_command_for_report(report)
    if replay_command:
        for row in actions:
            action_id = str(row.get("id") or "")
            if action_id not in {
                "drain_dlq_backlog",
                "triage_dlq_backlog",
                "stabilize_recurring_dlq_failures",
                "repair_dlq_replay_health",
            }:
                continue
            commands = row.get("commands")
            if not isinstance(commands, list):
                continue
            if replay_command not in commands:
                commands.append(replay_command)
    return {
        "generated_at": str(report.get("generated_at") or _utc_now()),
        "overall_status": str(report.get("overall_status") or "unknown"),
        "action_count": len(actions),
        "status": "action_required" if actionable else "stable",
        "actions": actions,
    }


def _as_markdown(report: dict[str, Any]) -> str:
    findings = report.get("findings") or []
    lines = [
        "# Symphony Readiness Audit",
        "",
        f"- Generated: `{report.get('generated_at')}`",
        f"- Overall status: `{report.get('overall_status')}`",
        "",
        "## Section Status",
    ]
    for section, payload in (report.get("sections") or {}).items():
        lines.append(f"- `{section}`: `{payload.get('status', 'unknown')}`")

    lines.extend(["", "## Findings"])
    if not findings:
        lines.append("- none")
    else:
        for row in findings:
            severity = row.get("severity", "info")
            code = row.get("code", "uncategorized")
            message = row.get("message", "")
            lines.append(f"- `{severity}` `{code}`: {message}")
    improvement = report.get("improvement_issue_sync") or {}
    if isinstance(improvement, dict):
        lines.extend(["", "## Improvement Issue Sync"])
        lines.append(f"- Status: `{improvement.get('status', 'unknown')}`")
        lines.append(f"- Desired issues: `{improvement.get('desired_issue_count', 0)}`")
        lines.append(f"- Planned creates: `{improvement.get('create_count', 0)}`")
        lines.append(f"- Planned updates: `{improvement.get('update_count', 0)}`")
    next_tranche = report.get("next_tranche") or {}
    lines.extend(["", "## Next Tranche"])
    actions = next_tranche.get("actions") if isinstance(next_tranche, dict) else None
    if not isinstance(actions, list) or not actions:
        lines.append("- none")
    else:
        for item in actions:
            if not isinstance(item, dict):
                continue
            priority = str(item.get("priority") or "P3")
            title = str(item.get("title") or "").strip()
            why = str(item.get("why") or "").strip()
            if not title:
                continue
            lines.append(f"- `{priority}` {title}: {why}")
    lines.append("")
    return "\n".join(lines)


def _safe_int(value: Any, *, default: int = 0) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    try:
        return int(str(value))
    except Exception:
        return default


def _safe_float(value: Any, *, default: float = 0.0) -> float:
    if isinstance(value, bool):
        return float(int(value))
    if isinstance(value, (float, int)):
        return float(value)
    try:
        return float(str(value))
    except Exception:
        return default


def _optional_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if value is None:
        return None
    try:
        return int(str(value))
    except Exception:
        return None


def _iso_compact(timestamp: str) -> str:
    raw = timestamp.strip()
    if raw.endswith("Z"):
        raw = raw[:-1]
    return re.sub(r"[-:.]", "", raw) + "Z"


def _snapshot_from_report(report: dict[str, Any]) -> dict[str, Any]:
    sections = report.get("sections") or {}
    linear = sections.get("linear_workspace") or {}
    durable = sections.get("durable_memory") or {}
    files = durable.get("files") or {}
    harness = sections.get("harness_engineering") or {}
    formal = sections.get("formal_suite") or {}
    return {
        "captured_at": str(report.get("generated_at") or _utc_now()),
        "readiness_overall_status": str(report.get("overall_status") or "unknown"),
        "harness_score": _safe_int(harness.get("score")),
        "harness_target": _safe_int(harness.get("target_score"), default=90),
        "linear_issue_count": _optional_int(linear.get("issue_count")),
        "linear_project_count": _optional_int(linear.get("project_count")),
        "linear_label_count": _optional_int(linear.get("label_count")),
        "linear_active_execution_flow": bool(linear.get("active_execution_flow")),
        "formal_suite_status": str(formal.get("status") or "unknown"),
        "formal_suite_mode": str(formal.get("mode") or "unknown"),
        "durable_status": str(durable.get("status") or "unknown"),
        "durable_jsonl_size": _safe_int((files.get("jsonl") or {}).get("size_bytes")),
        "durable_duckdb_size": _safe_int((files.get("duckdb") or {}).get("size_bytes")),
        "durable_parquet_size": _safe_int(
            (files.get("parquet") or {}).get("size_bytes")
        ),
    }


def _write_baseline_markdown(path: Path, snapshot: dict[str, Any]) -> None:
    issue_count = snapshot.get("linear_issue_count")
    project_count = snapshot.get("linear_project_count")
    label_count = snapshot.get("linear_label_count")
    lines = [
        "# Symphony Harness Baseline",
        "",
        f"- Captured: `{snapshot['captured_at']}`",
        f"- Readiness overall: `{snapshot['readiness_overall_status']}`",
        f"- Harness score: `{snapshot['harness_score']}` / `{snapshot['harness_target']}`",
        (
            "- Linear issues/projects/labels: "
            f"`{issue_count if issue_count is not None else 'n/a'}` / "
            f"`{project_count if project_count is not None else 'n/a'}` / "
            f"`{label_count if label_count is not None else 'n/a'}`"
        ),
        f"- Active execution flow: `{snapshot['linear_active_execution_flow']}`",
        (
            f"- Formal suite: `{snapshot['formal_suite_status']}` "
            f"(`{snapshot['formal_suite_mode']}`)"
        ),
        f"- Durable memory status: `{snapshot['durable_status']}`",
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8")


def _load_previous_baseline(
    history_dir: Path, current_captured_at: str
) -> dict[str, Any] | None:
    if not history_dir.exists():
        return None
    rows: list[tuple[str, dict[str, Any]]] = []
    for path in sorted(history_dir.glob("baseline_*.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        if not isinstance(payload, dict):
            continue
        captured_at = str(payload.get("captured_at") or "")
        if not captured_at or captured_at == current_captured_at:
            continue
        rows.append((captured_at, payload))
    if not rows:
        return None
    rows.sort(key=lambda item: item[0])
    return rows[-1][1]


def _load_baseline_history(
    history_dir: Path, *, limit: int | None = None
) -> list[dict[str, Any]]:
    if not history_dir.exists():
        return []
    rows: list[tuple[str, dict[str, Any]]] = []
    for path in sorted(history_dir.glob("baseline_*.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        if not isinstance(payload, dict):
            continue
        captured_at = str(payload.get("captured_at") or "").strip()
        if not captured_at:
            continue
        rows.append((captured_at, payload))
    rows.sort(key=lambda item: item[0])
    history = [payload for _, payload in rows]
    if limit is not None and limit > 0:
        history = history[-limit:]
    return history


def _calculate_trend_analysis(
    *,
    current_snapshot: dict[str, Any],
    baseline_history: list[dict[str, Any]],
    trend_window: int,
    max_harness_score_drop: int,
    min_active_flow_ratio: float,
    min_formal_pass_ratio: float,
    max_durable_growth_ratio: float,
) -> dict[str, Any]:
    series = [row for row in baseline_history if isinstance(row, dict)] + [
        current_snapshot
    ]
    if trend_window > 0:
        series = series[-trend_window:]

    point_count = len(series)
    active_values = [bool(row.get("linear_active_execution_flow")) for row in series]
    active_flow_ratio = (
        round(sum(1 for value in active_values if value) / point_count, 4)
        if point_count
        else 0.0
    )

    formal_values = [
        str(row.get("formal_suite_status") or "").strip().lower() for row in series
    ]
    formal_considered = [
        value for value in formal_values if value and value != "unknown"
    ]
    formal_pass_ratio = (
        round(
            sum(1 for value in formal_considered if value == "pass")
            / len(formal_considered),
            4,
        )
        if formal_considered
        else 1.0
    )

    harness_scores = [_safe_int(row.get("harness_score")) for row in series]
    previous_harness = harness_scores[-2] if len(harness_scores) >= 2 else None
    latest_harness = harness_scores[-1] if harness_scores else None
    harness_delta = (
        latest_harness - previous_harness
        if isinstance(latest_harness, int) and isinstance(previous_harness, int)
        else None
    )
    harness_regressed = bool(
        isinstance(harness_delta, int) and harness_delta < -max_harness_score_drop
    )

    durable_pairs: list[dict[str, Any]] = []
    for prev, cur in zip(series, series[1:], strict=False):
        for field, artifact in (
            ("durable_jsonl_size", "jsonl"),
            ("durable_duckdb_size", "duckdb"),
            ("durable_parquet_size", "parquet"),
        ):
            prev_size = _safe_int(prev.get(field))
            cur_size = _safe_int(cur.get(field))
            if prev_size <= 0:
                continue
            ratio = (cur_size - prev_size) / prev_size
            durable_pairs.append(
                {
                    "artifact": artifact,
                    "prev_captured_at": prev.get("captured_at"),
                    "captured_at": cur.get("captured_at"),
                    "delta_ratio": round(ratio, 6),
                    "delta_bytes": cur_size - prev_size,
                }
            )

    durable_breaches = [
        row for row in durable_pairs if row["delta_ratio"] > max_durable_growth_ratio
    ]
    breach_intervals = {
        (str(row.get("prev_captured_at") or ""), str(row.get("captured_at") or ""))
        for row in durable_breaches
    }
    recurring_durable_growth = len(breach_intervals) >= 2
    active_flow_ratio_low = active_flow_ratio < min_active_flow_ratio
    formal_pass_ratio_low = formal_pass_ratio < min_formal_pass_ratio

    status = (
        "warn"
        if harness_regressed
        or active_flow_ratio_low
        or formal_pass_ratio_low
        or recurring_durable_growth
        else "pass"
    )
    return {
        "status": status,
        "point_count": point_count,
        "window": trend_window,
        "max_harness_score_drop": max_harness_score_drop,
        "min_active_flow_ratio": round(min_active_flow_ratio, 4),
        "min_formal_pass_ratio": round(min_formal_pass_ratio, 4),
        "max_durable_growth_ratio": round(max_durable_growth_ratio, 6),
        "harness": {
            "scores": harness_scores,
            "latest": latest_harness,
            "previous": previous_harness,
            "delta": harness_delta,
            "regressed": harness_regressed,
        },
        "active_flow": {
            "values": active_values,
            "ratio": active_flow_ratio,
            "ratio_low": active_flow_ratio_low,
        },
        "formal_suite": {
            "values": formal_values,
            "pass_ratio": formal_pass_ratio,
            "ratio_low": formal_pass_ratio_low,
        },
        "durable_growth": {
            "pairs": durable_pairs,
            "breaches": durable_breaches,
            "breach_interval_count": len(breach_intervals),
            "recurring": recurring_durable_growth,
        },
    }


def _apply_durable_growth_gate(
    *,
    report: dict[str, Any],
    findings: list[dict[str, Any]],
    previous_baseline: dict[str, Any] | None,
    max_growth_ratio: float = DEFAULT_DURABLE_GROWTH_WARN_RATIO,
) -> None:
    if previous_baseline is None:
        _record_finding(
            findings,
            severity="info",
            code="durable_growth_baseline_missing",
            message=(
                "No previous readiness baseline found for durable growth comparison; "
                "growth gate will activate after baseline history has at least 2 points."
            ),
        )
        return

    current = _snapshot_from_report(report)
    growth_rows: list[dict[str, Any]] = []
    for field, label in (
        ("durable_jsonl_size", "jsonl"),
        ("durable_duckdb_size", "duckdb"),
        ("durable_parquet_size", "parquet"),
    ):
        prev = _safe_int(previous_baseline.get(field))
        curr = _safe_int(current.get(field))
        if prev <= 0:
            continue
        delta = curr - prev
        ratio = delta / prev
        growth_rows.append(
            {
                "artifact": label,
                "previous_bytes": prev,
                "current_bytes": curr,
                "delta_bytes": delta,
                "delta_ratio": round(ratio, 6),
            }
        )

    if not growth_rows:
        return

    breaches = [row for row in growth_rows if row["delta_ratio"] > max_growth_ratio]
    if breaches:
        _record_finding(
            findings,
            severity="warn",
            code="durable_growth_budget_exceeded",
            message=(
                "Durable memory artifacts exceeded the run-over-run growth budget "
                f"({int(max_growth_ratio * 100)}%)."
            ),
            details={
                "previous_captured_at": previous_baseline.get("captured_at"),
                "current_captured_at": current.get("captured_at"),
                "threshold_ratio": max_growth_ratio,
                "breaches": breaches,
            },
        )
    else:
        _record_finding(
            findings,
            severity="info",
            code="durable_growth_within_budget",
            message=(
                "Durable memory growth stayed within run-over-run budget "
                f"({int(max_growth_ratio * 100)}%)."
            ),
            details={
                "previous_captured_at": previous_baseline.get("captured_at"),
                "current_captured_at": current.get("captured_at"),
                "threshold_ratio": max_growth_ratio,
                "artifacts": growth_rows,
            },
        )


def _prune_baseline_history(history_dir: Path, retention_days: int) -> int:
    if retention_days <= 0 or not history_dir.exists():
        return 0
    cutoff = datetime.now(UTC) - timedelta(days=retention_days)
    removed = 0
    for path in sorted(history_dir.glob("baseline_*.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        if not isinstance(payload, dict):
            continue
        captured_raw = str(payload.get("captured_at") or "").strip()
        if not captured_raw:
            continue
        try:
            captured_at = datetime.fromisoformat(captured_raw.replace("Z", "+00:00"))
        except Exception:
            continue
        if captured_at < cutoff:
            path.unlink(missing_ok=True)
            removed += 1
    return removed


def _durable_growth_breach_finding(report: dict[str, Any]) -> dict[str, Any] | None:
    findings = report.get("findings")
    if not isinstance(findings, list):
        return None
    for row in findings:
        if not isinstance(row, dict):
            continue
        if str(row.get("code")) == "durable_growth_budget_exceeded":
            return row
    return None


def _post_growth_alert_comment(
    *,
    report: dict[str, Any],
    team: str,
    issue_ref: str,
    env_file: Path,
) -> dict[str, Any]:
    finding = _durable_growth_breach_finding(report)
    if finding is None:
        return {"status": "skipped", "reason": "no_growth_breach"}

    details = finding.get("details")
    if not isinstance(details, dict):
        return {"status": "skipped", "reason": "missing_details"}

    api_key = _resolve_linear_api_key(env_file)  # secret-guard: allow
    if not api_key:
        return {"status": "skipped", "reason": "missing_linear_api_key"}

    breaches = details.get("breaches")
    breach_rows = breaches if isinstance(breaches, list) else []
    if not breach_rows:
        return {"status": "skipped", "reason": "empty_breaches"}

    lines = [
        "Readiness durable growth alert",
        "",
        f"- Captured at: `{report.get('generated_at')}`",
        (
            f"- Threshold: `{int(float(details.get('threshold_ratio', 0.05)) * 100)}%` "
            "run-over-run"
        ),
        f"- Previous baseline: `{details.get('previous_captured_at')}`",
        "",
        "Breaches:",
    ]
    for item in breach_rows:
        if not isinstance(item, dict):
            continue
        artifact = item.get("artifact")
        ratio = float(item.get("delta_ratio") or 0.0) * 100.0
        delta_bytes = _safe_int(item.get("delta_bytes"))
        current_bytes = _safe_int(item.get("current_bytes"))
        lines.append(
            (
                f"- `{artifact}`: +{ratio:.2f}% "
                f"(`{delta_bytes}` bytes, now `{current_bytes}`)"
            )
        )
    lines.extend(
        [
            "",
            "Action requested: triage source of growth and confirm expected vs unexpected expansion.",
        ]
    )
    body = "\n".join(lines)

    with _temporary_linear_api_key(api_key):
        team_id = linear_workspace._resolve_team_id(team)
        issue_id = linear_workspace._resolve_issue_id(team_id, issue_ref)
        data = linear_workspace.graphql(
            MUTATION_COMMENT_CREATE, {"input": {"issueId": issue_id, "body": body}}
        )
    result = data.get("commentCreate") if isinstance(data, dict) else None
    if not isinstance(result, dict) or not result.get("success"):
        raise RuntimeError("commentCreate returned success=false")
    comment = result.get("comment") if isinstance(result.get("comment"), dict) else {}
    return {
        "status": "posted",
        "issue": issue_ref,
        "comment_id": comment.get("id"),
    }


def _persist_harness_metrics(
    ext_root: Path,
    report: dict[str, Any],
    *,
    retention_days: int,
    path_env: dict[str, str] | None = None,
) -> dict[str, Any]:
    del ext_root
    linear_section = (report.get("sections") or {}).get("linear_workspace") or {}
    if str(linear_section.get("status") or "unknown") != "pass":
        return {"status": "skipped", "reason": "linear_workspace_not_pass"}

    snapshot = _snapshot_from_report(report)
    readiness_history_dir = symphony_readiness_dir(path_env) / "history"
    metrics_dir = symphony_metrics_dir(path_env)
    readiness_history_dir.mkdir(parents=True, exist_ok=True)
    metrics_dir.mkdir(parents=True, exist_ok=True)

    stamp = _iso_compact(snapshot["captured_at"])
    history_path = readiness_history_dir / f"baseline_{stamp}.json"
    history_written = False
    if not history_path.exists():
        history_path.write_text(
            json.dumps(snapshot, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        history_written = True

    baseline_md = metrics_dir / "harness_baseline_latest.md"
    _write_baseline_markdown(baseline_md, snapshot)

    csv_path = metrics_dir / "harness_timeseries.csv"
    existing_rows: list[dict[str, str]] = []
    seen_captured_at: set[str] = set()
    if csv_path.exists():
        with csv_path.open("r", encoding="utf-8", newline="") as handle:
            reader = csv.DictReader(handle)
            for row in reader:
                if not isinstance(row, dict):
                    continue
                captured_at = str(row.get("captured_at") or "").strip()
                if captured_at:
                    seen_captured_at.add(captured_at)
                existing_rows.append({str(k): str(v) for k, v in row.items()})

    captured_at = snapshot["captured_at"]
    appended_csv_row = False
    if captured_at not in seen_captured_at:
        row = {
            field: str(snapshot.get(field, "")) for field in HARNESS_TIMESERIES_FIELDS
        }
        existing_rows.append(row)
        appended_csv_row = True

    with csv_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(HARNESS_TIMESERIES_FIELDS))
        writer.writeheader()
        writer.writerows(existing_rows)

    pruned = _prune_baseline_history(readiness_history_dir, retention_days)
    return {
        "status": "ok",
        "history_written": history_written,
        "csv_row_appended": appended_csv_row,
        "history_pruned": pruned,
        "history_path": str(history_path),
        "csv_path": str(csv_path),
    }


def _exit_code_for(findings: list[dict[str, Any]], fail_on: str) -> int:
    severities = {str(item.get("severity")) for item in findings}
    if fail_on == "none":
        return 0
    if fail_on == "warn":
        return 1 if "warn" in severities or "fail" in severities else 0
    return 1 if "fail" in severities else 0


def run_audit(
    *,
    repo_root: Path,
    team: str,
    env_file: Path,
    index_path: Path,
    ext_root: Path,
    durable_root: Path,
    strict_autonomy: bool,
    formal_suite_mode: str = "inventory",
    trend_window: int = DEFAULT_TREND_WINDOW,
    max_harness_score_drop: int = DEFAULT_MAX_HARNESS_SCORE_DROP,
    min_active_flow_ratio: float = DEFAULT_MIN_ACTIVE_FLOW_RATIO,
    min_formal_pass_ratio: float = DEFAULT_MIN_FORMAL_PASS_RATIO,
    max_durable_growth_ratio: float = DEFAULT_DURABLE_GROWTH_WARN_RATIO,
    sync_improvement_issues: bool = False,
    improvement_issue_project: str | None = None,
    improvement_issue_limit: int = 3,
) -> dict[str, Any]:
    path_env = _runtime_env(env_file)
    api_key = _resolve_linear_api_key(env_file)  # secret-guard: allow
    with _temporary_linear_api_key(api_key):
        linear_workspace_section = _audit_linear_workspace(team)
        linear_cli_compat_section = _audit_lin_cli_compat(env_file)

    report: dict[str, Any] = {
        "generated_at": _utc_now(),
        "repo_root": str(repo_root),
        "sections": {
            "environment": _audit_env_and_volume(env_file=env_file, ext_root=ext_root),
            "storage_layout": _audit_storage_layout(env_file),
            "docs_and_tools": _audit_docs_and_tools(repo_root),
            "harness_engineering": _audit_harness_engineering(repo_root),
            "dspy_routing": _audit_dspy_routing(env_file),
            "launchd": _audit_launchd(),
            "durable_memory": _audit_durable_memory(durable_root),
            "dlq_health": _audit_dlq_health(symphony_dlq_events_file(path_env)),
            "manifest_index": _audit_manifest_index(index_path),
            "linear_workspace": linear_workspace_section,
            "linear_cli_compat": linear_cli_compat_section,
            "formal_suite": _audit_formal_suite(repo_root, formal_suite_mode),
            "tool_promotion": _audit_tool_promotion(
                events_path=symphony_tool_promotion_events_file(path_env),
                distillations_dir=symphony_tool_promotion_distillations_dir(path_env),
            ),
        },
    }
    history_dir = symphony_readiness_dir(path_env) / "history"
    trend_history = _load_baseline_history(history_dir, limit=max(2, trend_window))
    current_snapshot = _snapshot_from_report(report)
    report["sections"]["trend_analysis"] = _calculate_trend_analysis(
        current_snapshot=current_snapshot,
        baseline_history=trend_history,
        trend_window=max(2, trend_window),
        max_harness_score_drop=max_harness_score_drop,
        min_active_flow_ratio=min_active_flow_ratio,
        min_formal_pass_ratio=min_formal_pass_ratio,
        max_durable_growth_ratio=max_durable_growth_ratio,
    )
    previous_baseline = _load_previous_baseline(
        history_dir, str(report.get("generated_at") or "")
    )
    findings = _collect_findings(report)
    _apply_durable_growth_gate(
        report=report,
        findings=findings,
        previous_baseline=previous_baseline,
        max_growth_ratio=max_durable_growth_ratio,
    )
    if strict_autonomy:
        findings = _apply_strict_autonomy(findings)
    report["findings"] = findings
    report["overall_status"] = _overall_status(findings)
    report["strict_autonomy"] = bool(strict_autonomy)
    report["trend_config"] = {
        "window": max(2, trend_window),
        "max_harness_score_drop": max_harness_score_drop,
        "min_active_flow_ratio": min_active_flow_ratio,
        "min_formal_pass_ratio": min_formal_pass_ratio,
        "max_durable_growth_ratio": max_durable_growth_ratio,
    }
    report["next_tranche"] = _synthesize_next_tranche(report)
    report["improvement_issue_sync"] = _sync_improvement_issues(
        report=report,
        team=team,
        env_file=env_file,
        project_ref=improvement_issue_project,
        apply=bool(sync_improvement_issues),
        max_tool_candidates=max(0, int(improvement_issue_limit)),
    )
    return report


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Comprehensive Symphony readiness audit for Linear, docs/tools, "
            "harness engineering quality, launchd wiring, and durable telemetry."
        )
    )
    parser.add_argument("--team", default="Moltlang")
    parser.add_argument("--repo-root", default=".")
    parser.add_argument("--env-file", default="ops/linear/runtime/symphony.env")
    parser.add_argument("--index", default="ops/linear/manifests/index.json")
    parser.add_argument(
        "--ext-root",
        default=os.environ.get("MOLT_EXT_ROOT") or str(resolve_molt_ext_root()),
    )
    parser.add_argument(
        "--durable-root",
        default=os.environ.get("MOLT_SYMPHONY_DURABLE_ROOT")
        or str(symphony_durable_root()),
    )
    parser.add_argument(
        "--output-json",
        default=None,
        help="Write JSON report to path (default: Symphony readiness log root under the canonical store).",
    )
    parser.add_argument(
        "--output-md",
        default=None,
        help="Write Markdown summary to path (default: Symphony readiness log root under the canonical store).",
    )
    parser.add_argument(
        "--fail-on",
        choices=["fail", "warn", "none"],
        default="fail",
        help="Exit non-zero on fail/warn findings.",
    )
    parser.add_argument(
        "--strict-autonomy",
        action="store_true",
        help=(
            "Promote autonomy-critical warnings (metadata/title drift and no active flow) "
            "to failures."
        ),
    )
    parser.add_argument(
        "--formal-suite",
        choices=list(FORMAL_SUITE_MODES),
        default="inventory",
        help=(
            "Formalization suite mode for readiness signal. "
            "Use all/lean/quint for deeper checks; off disables this section."
        ),
    )
    parser.add_argument(
        "--trend-window",
        type=int,
        default=DEFAULT_TREND_WINDOW,
        help="Historical readiness points to include for trend analysis.",
    )
    parser.add_argument(
        "--max-harness-score-drop",
        type=int,
        default=DEFAULT_MAX_HARNESS_SCORE_DROP,
        help="Warn when harness score drops more than this amount vs prior baseline.",
    )
    parser.add_argument(
        "--min-active-flow-ratio",
        type=float,
        default=DEFAULT_MIN_ACTIVE_FLOW_RATIO,
        help="Minimum acceptable active-execution-flow ratio across trend window.",
    )
    parser.add_argument(
        "--min-formal-pass-ratio",
        type=float,
        default=DEFAULT_MIN_FORMAL_PASS_RATIO,
        help="Minimum acceptable formal-suite pass ratio across trend window.",
    )
    parser.add_argument(
        "--max-durable-growth-ratio",
        type=float,
        default=DEFAULT_DURABLE_GROWTH_WARN_RATIO,
        help="Maximum allowed durable artifact run-over-run growth ratio.",
    )
    parser.add_argument(
        "--output-next-tranche-json",
        default=None,
        help=(
            "Write synthesized next-tranche plan JSON (default: "
            "Symphony readiness log root under the canonical store)"
        ),
    )
    parser.add_argument(
        "--output-next-tranche-md",
        default=None,
        help=(
            "Write synthesized next-tranche plan Markdown (default: "
            "Symphony readiness log root under the canonical store)"
        ),
    )
    parser.add_argument(
        "--baseline-retention-days",
        type=int,
        default=90,
        help=(
            "Retention window for readiness history baselines. "
            "Older baseline_*.json files are pruned after each successful metrics persist."
        ),
    )
    parser.add_argument(
        "--growth-alert-issue",
        default=None,
        help=(
            "Optional Linear issue identifier to comment when durable growth budget is exceeded "
            "(example: MOL-211)."
        ),
    )
    parser.add_argument(
        "--sync-improvement-issues",
        action="store_true",
        help=(
            "Create/update improvement issues for DLQ backlog and tool-promotion candidates. "
            "Without this flag, readiness emits only a dry-run sync plan."
        ),
    )
    parser.add_argument(
        "--improvement-issue-project",
        default=None,
        help=(
            "Optional Linear project ref (name, slugId, or id) used when syncing "
            "improvement issues."
        ),
    )
    parser.add_argument(
        "--improvement-issue-limit",
        type=int,
        default=3,
        help="Maximum number of tool-promotion candidate issues to plan/sync.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo_root = Path(args.repo_root).resolve()
    env_file = Path(args.env_file).resolve()
    index_path = Path(args.index).resolve()
    ext_root = Path(args.ext_root).expanduser().resolve()
    durable_root = Path(args.durable_root).expanduser().resolve()

    report = run_audit(
        repo_root=repo_root,
        team=str(args.team),
        env_file=env_file,
        index_path=index_path,
        ext_root=ext_root,
        durable_root=durable_root,
        strict_autonomy=bool(args.strict_autonomy),
        formal_suite_mode=str(args.formal_suite),
        trend_window=max(2, int(args.trend_window)),
        max_harness_score_drop=max(0, int(args.max_harness_score_drop)),
        min_active_flow_ratio=max(0.0, min(1.0, float(args.min_active_flow_ratio))),
        min_formal_pass_ratio=max(0.0, min(1.0, float(args.min_formal_pass_ratio))),
        max_durable_growth_ratio=max(0.0, float(args.max_durable_growth_ratio)),
        sync_improvement_issues=bool(args.sync_improvement_issues),
        improvement_issue_project=(
            str(args.improvement_issue_project).strip()
            if args.improvement_issue_project
            else None
        ),
        improvement_issue_limit=max(0, int(args.improvement_issue_limit)),
    )

    runtime_env = _runtime_env(env_file)
    readiness_root = symphony_readiness_dir(runtime_env)
    output_json = (
        Path(str(args.output_json)).expanduser().resolve()
        if args.output_json
        else (readiness_root / "latest.json")
    )
    output_md = (
        Path(str(args.output_md)).expanduser().resolve()
        if args.output_md
        else (readiness_root / "latest.md")
    )
    output_next_tranche_json = (
        Path(str(args.output_next_tranche_json)).expanduser().resolve()
        if args.output_next_tranche_json
        else (readiness_root / "next_tranche.json")
    )
    output_next_tranche_md = (
        Path(str(args.output_next_tranche_md)).expanduser().resolve()
        if args.output_next_tranche_md
        else (readiness_root / "next_tranche.md")
    )

    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    output_md.parent.mkdir(parents=True, exist_ok=True)
    output_md.write_text(_as_markdown(report), encoding="utf-8")
    next_tranche = report.get("next_tranche") or {}
    output_next_tranche_json.parent.mkdir(parents=True, exist_ok=True)
    output_next_tranche_json.write_text(
        json.dumps(next_tranche, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    output_next_tranche_md.parent.mkdir(parents=True, exist_ok=True)
    next_lines = [
        "# Symphony Next Tranche",
        "",
        f"- Generated: `{next_tranche.get('generated_at', report.get('generated_at'))}`",
        f"- Overall status: `{next_tranche.get('overall_status', report.get('overall_status'))}`",
        f"- Action count: `{next_tranche.get('action_count', 0)}`",
        "",
    ]
    actions = next_tranche.get("actions")
    if isinstance(actions, list) and actions:
        next_lines.append("## Actions")
        for row in actions:
            if not isinstance(row, dict):
                continue
            next_lines.append(
                f"- `{row.get('priority', 'P3')}` {row.get('title', 'Unnamed action')}: {row.get('why', '')}"
            )
    else:
        next_lines.append("- No actions generated.")
    output_next_tranche_md.write_text("\n".join(next_lines) + "\n", encoding="utf-8")
    metrics_result = _persist_harness_metrics(
        ext_root,
        report,
        retention_days=max(0, int(args.baseline_retention_days)),
        path_env=runtime_env,
    )
    print(
        f"Harness metrics persistence: {json.dumps(metrics_result, sort_keys=True)}",
        file=sys.stderr,
    )
    if args.growth_alert_issue:
        try:
            alert_result = _post_growth_alert_comment(
                report=report,
                team=str(args.team),
                issue_ref=str(args.growth_alert_issue),
                env_file=env_file,
            )
            print(
                f"Growth alert hook: {json.dumps(alert_result, sort_keys=True)}",
                file=sys.stderr,
            )
        except Exception as exc:
            print(
                (f"Growth alert hook failed: {type(exc).__name__}: {exc}"),
                file=sys.stderr,
            )

    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"Wrote JSON report: {output_json}", file=sys.stderr)
    print(f"Wrote Markdown report: {output_md}", file=sys.stderr)
    print(f"Wrote Next Tranche JSON: {output_next_tranche_json}", file=sys.stderr)
    print(f"Wrote Next Tranche Markdown: {output_next_tranche_md}", file=sys.stderr)
    return _exit_code_for(report["findings"], str(args.fail_on))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
