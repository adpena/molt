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
    "tools/linear_workspace.py",
    "tools/symphony_perf.py",
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

DURABLE_GROWTH_WARN_RATIO = 0.05


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
        "durable_parquet_size": _safe_int((files.get("parquet") or {}).get("size_bytes")),
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


def _apply_durable_growth_gate(
    *,
    report: dict[str, Any],
    findings: list[dict[str, Any]],
    previous_baseline: dict[str, Any] | None,
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

    breaches = [
        row for row in growth_rows if row["delta_ratio"] > DURABLE_GROWTH_WARN_RATIO
    ]
    if breaches:
        _record_finding(
            findings,
            severity="warn",
            code="durable_growth_budget_exceeded",
            message=(
                "Durable memory artifacts exceeded the run-over-run growth budget "
                f"({int(DURABLE_GROWTH_WARN_RATIO * 100)}%)."
            ),
            details={
                "previous_captured_at": previous_baseline.get("captured_at"),
                "current_captured_at": current.get("captured_at"),
                "threshold_ratio": DURABLE_GROWTH_WARN_RATIO,
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
                f"({int(DURABLE_GROWTH_WARN_RATIO * 100)}%)."
            ),
            details={
                "previous_captured_at": previous_baseline.get("captured_at"),
                "current_captured_at": current.get("captured_at"),
                "threshold_ratio": DURABLE_GROWTH_WARN_RATIO,
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
    ext_root: Path, report: dict[str, Any], *, retention_days: int
) -> dict[str, Any]:
    linear_section = (report.get("sections") or {}).get("linear_workspace") or {}
    if str(linear_section.get("status") or "unknown") != "pass":
        return {"status": "skipped", "reason": "linear_workspace_not_pass"}

    snapshot = _snapshot_from_report(report)
    readiness_history_dir = ext_root / "logs" / "symphony" / "readiness" / "history"
    metrics_dir = ext_root / "logs" / "symphony" / "metrics"
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
        row = {field: str(snapshot.get(field, "")) for field in HARNESS_TIMESERIES_FIELDS}
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
) -> dict[str, Any]:
    api_key = _resolve_linear_api_key(env_file)  # secret-guard: allow
    with _temporary_linear_api_key(api_key):
        linear_workspace_section = _audit_linear_workspace(team)
        linear_cli_compat_section = _audit_lin_cli_compat(env_file)

    report = {
        "generated_at": _utc_now(),
        "repo_root": str(repo_root),
        "sections": {
            "environment": _audit_env_and_volume(env_file=env_file, ext_root=ext_root),
            "docs_and_tools": _audit_docs_and_tools(repo_root),
            "harness_engineering": _audit_harness_engineering(repo_root),
            "dspy_routing": _audit_dspy_routing(env_file),
            "launchd": _audit_launchd(),
            "durable_memory": _audit_durable_memory(durable_root),
            "manifest_index": _audit_manifest_index(index_path),
            "linear_workspace": linear_workspace_section,
            "linear_cli_compat": linear_cli_compat_section,
            "formal_suite": _audit_formal_suite(repo_root, formal_suite_mode),
        },
    }
    history_dir = ext_root / "logs" / "symphony" / "readiness" / "history"
    previous_baseline = _load_previous_baseline(
        history_dir, str(report.get("generated_at") or "")
    )
    findings = _collect_findings(report)
    _apply_durable_growth_gate(
        report=report, findings=findings, previous_baseline=previous_baseline
    )
    if strict_autonomy:
        findings = _apply_strict_autonomy(findings)
    report["findings"] = findings
    report["overall_status"] = _overall_status(findings)
    report["strict_autonomy"] = bool(strict_autonomy)
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
        default=os.environ.get("MOLT_EXT_ROOT") or "/Volumes/APDataStore/Molt",
    )
    parser.add_argument(
        "--durable-root",
        default=os.environ.get("MOLT_SYMPHONY_DURABLE_ROOT")
        or "/Volumes/APDataStore/Molt/logs/symphony/durable_memory",
    )
    parser.add_argument(
        "--output-json",
        default=None,
        help="Write JSON report to path (default: <ext-root>/logs/symphony/readiness/latest.json)",
    )
    parser.add_argument(
        "--output-md",
        default=None,
        help="Write Markdown summary to path (default: <ext-root>/logs/symphony/readiness/latest.md)",
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
    )

    readiness_root = ext_root / "logs" / "symphony" / "readiness"
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

    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    output_md.parent.mkdir(parents=True, exist_ok=True)
    output_md.write_text(_as_markdown(report), encoding="utf-8")
    metrics_result = _persist_harness_metrics(
        ext_root, report, retention_days=max(0, int(args.baseline_retention_days))
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
                (
                    "Growth alert hook failed: "
                    f"{type(exc).__name__}: {exc}"
                ),
                file=sys.stderr,
            )

    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"Wrote JSON report: {output_json}", file=sys.stderr)
    print(f"Wrote Markdown report: {output_md}", file=sys.stderr)
    return _exit_code_for(report["findings"], str(args.fail_on))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
