from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import UTC, datetime
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

REQUIRED_METADATA_KEYS = ("area", "owner", "milestone", "priority", "status", "source")
SEED_HEADER = "Auto-seeded from Molt roadmap/status TODO contracts."
_META_LINE_RE = re.compile(r"^\s*[-*]\s*([a-z_]+)\s*:\s*(.+?)\s*$")

STRICT_AUTONOMY_FAIL_CODES = {
    "manifest_titles_malformed",
    "linear_seeded_metadata_gaps",
    "linear_titles_malformed",
    "linear_no_active_flow",
}
FORMAL_SUITE_MODES = ("off", "inventory", "lean", "quint", "all")

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
    if isinstance(diagnostics, dict) and bool(diagnostics.get("runtime_mismatch_detected")):
        return True
    errors = quint.get("errors")
    if not isinstance(errors, list):
        return False
    return any("quint_runtime_toolchain_mismatch" in str(item) for item in errors)


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
            diagnostics = (
                quint.get("diagnostics") if isinstance(quint, dict) else None
            )
            if isinstance(diagnostics, dict) and bool(
                diagnostics.get("fallback_used")
            ):
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
            "launchd": _audit_launchd(),
            "durable_memory": _audit_durable_memory(durable_root),
            "manifest_index": _audit_manifest_index(index_path),
            "linear_workspace": linear_workspace_section,
            "linear_cli_compat": linear_cli_compat_section,
            "formal_suite": _audit_formal_suite(repo_root, formal_suite_mode),
        },
    }
    findings = _collect_findings(report)
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
            "launchd wiring, and durable telemetry."
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

    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"Wrote JSON report: {output_json}", file=sys.stderr)
    print(f"Wrote Markdown report: {output_md}", file=sys.stderr)
    return _exit_code_for(report["findings"], str(args.fail_on))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
