"""Proof logs, notebooks, submission evidence, and durable projections."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shlex
import sqlite3
import sys
import traceback
from pathlib import Path
from typing import Any, Mapping, Sequence

from tools.proof_queue_pkg import (
    command_admission,
    diagnostics,
    state,
    supervisor_custody,
)

_NON_EXECUTED_RECEIPT_STATUSES = frozenset({"queued", "dispatched", "blocked"})


def _notebooks_root(args: argparse.Namespace) -> Path:
    return (
        Path(args.notebooks_root)
        if getattr(args, "notebooks_root", None)
        else state._logs_root(args).parent / "notebooks"
    )


def _run_payload_with_notes(
    conn: sqlite3.Connection, rows: list[sqlite3.Row]
) -> list[dict[str, object]]:
    payload = [_row_to_payload(row) for row in rows]
    run_ids = [str(item["run_id"]) for item in payload]
    notes = state._notes_for_run_ids(conn, run_ids)
    edges = state._edges_for_run_ids(conn, run_ids)
    for row, item in zip(rows, payload, strict=True):
        run_notes = notes.get(str(item["run_id"]), [])
        run_edges = edges.get(str(item["run_id"]), {"parents": [], "children": []})
        item["notes"] = run_notes
        item["note_kind_counts"] = state._note_kind_counts(run_notes)
        item["dag"] = {
            "parents": run_edges["parents"],
            "children": run_edges["children"],
            "parent_kind_counts": state._edge_kind_counts(run_edges["parents"]),
            "child_kind_counts": state._edge_kind_counts(run_edges["children"]),
        }
        item["diagnostics"] = diagnostics._run_diagnostics(row)
    return payload


def _marimo_notebook_text(run: dict[str, object]) -> str:
    run_json = json.dumps(run, indent=2, sort_keys=True)
    return f'''# /// script
# dependencies = [
#   "marimo",
# ]
# ///
import marimo

__generated_with = "molt proof_queue"
app = marimo.App(width="medium")


@app.cell
def _():
    import json
    from pathlib import Path
    import marimo as mo

    run = json.loads({run_json!r})
    notes = run.get("notes", [])
    return Path, mo, notes, run


@app.cell
def _(mo, run):
    git = run.get("git", {{}})
    head = git.get("head", "unknown")
    dirty = "dirty" if git.get("dirty") else "clean"
    note_counts = run.get("note_kind_counts", {{}})
    dag = run.get("dag", {{}})
    parent_counts = dag.get("parent_kind_counts", {{}})
    child_counts = dag.get("child_kind_counts", {{}})
    note_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in note_counts.items()
    ) or "none"
    parent_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in parent_counts.items()
    ) or "none"
    child_summary = ", ".join(
        f"{{kind}}={{count}}" for kind, count in child_counts.items()
    ) or "none"
    receipt = run.get("proof_receipt", {{}})
    receipt_state = run.get("proof_receipt_state", {{}})
    interpreters = receipt.get("python_interpreters", {{}})
    control_python = interpreters.get("queue_control_plane", {{}})
    proof_python = interpreters.get("proof_command", {{}})
    control_summary = control_python.get("version", "not executed")
    proof_summary = proof_python.get(
        "version", receipt_state.get("status", "not executed")
    )
    mo.md(
        f"""
# Proof run `{{run["run_id"]}}`

- logical id: `{{run["logical_id"]}}`
- status: `{{run["status"]}}`, return code: `{{run["returncode"]}}`
- git: `{{head}}` (`{{dirty}}`)
- contention key: `{{run["contention_key"]}}`
- notes: {{note_summary}}
- parents: {{parent_summary}}
- children: {{child_summary}}
- queue control-plane Python: `{{control_summary}}`
- proof-command Python: `{{proof_summary}}`
- reason: {{run["reason"]}}
"""
    )
    return


@app.cell
def _(run):
    run
    return


@app.cell
def _(notes):
    notes
    return


@app.cell
def _(Path, run):
    log_path = Path(run["log_path"])
    if log_path.exists():
        log_tail = "\\n".join(
            log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-120:]
        )
    else:
        log_tail = ""
    log_tail
    return


if __name__ == "__main__":
    app.run()
'''


def _write_marimo_notebook(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    run_id: str,
    output: str | None = None,
) -> Path:
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        "SELECT * FROM proof_runs WHERE run_id = ?", (run_id,)
    ).fetchone()
    if row is None:
        raise SystemExit(f"unknown proof run {run_id!r}")
    run = _run_payload_with_notes(conn, [row])[0]
    path = Path(output) if output else _notebooks_root(args) / f"{run_id}.py"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(_marimo_notebook_text(run), encoding="utf-8")
    return path


def _queue_peak_rss_bytes(summary_path: object) -> int:
    try:
        payload = json.loads(Path(str(summary_path)).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, TypeError):
        return 0
    for key in ("peak_total", "peak"):
        record = payload.get(key)
        if isinstance(record, dict) and isinstance(record.get("rss_kb"), int):
            return int(record["rss_kb"]) * 1024
    return 0


def _queue_proof_receipt(
    row: sqlite3.Row | Mapping[str, Any],
) -> dict[str, object]:
    status = str(row["status"])
    returncode = row["returncode"]
    succeeded = status == "passed" and returncode == 0
    command_id = f"queue.{row['logical_id']}"
    argv = json.loads(row["command_json"])
    context_raw = state._row_value(row, "receipt_context_json")
    if not isinstance(context_raw, str) or not context_raw:
        raise ValueError("executed proof run has no persisted receipt context")
    context = json.loads(context_raw)
    if not isinstance(context, dict):
        raise ValueError("proof run receipt context is malformed")
    envelope_raw = state._row_value(row, "command_envelope_json")
    envelope = json.loads(str(envelope_raw))
    if not isinstance(envelope, dict):
        raise ValueError("proof run admission envelope is malformed")
    context_envelope = context.get("command_envelope")
    if context.get("terminal_evidence_sha256") is not None:
        if context_envelope != envelope:
            raise ValueError(
                "terminal proof receipt envelope differs from immutable admission"
            )
        envelope_digest = hashlib.sha256(
            json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if context.get("command_envelope_sha256") != envelope_digest:
            raise ValueError("terminal proof receipt envelope digest mismatch")
    if succeeded:
        requested_toolchains = (
            envelope.get("toolchains") if isinstance(envelope, dict) else None
        )
        captured_toolchains = context.get("toolchains")
        toolchain_custody = context.get("toolchain_custody")
        if (
            not isinstance(requested_toolchains, list)
            or not requested_toolchains
            or not isinstance(captured_toolchains, dict)
            or set(captured_toolchains) != set(requested_toolchains)
            or not isinstance(toolchain_custody, dict)
            or toolchain_custody.get("identical") is not True
        ):
            raise ValueError(
                "passed proof run has no complete stable toolchain closure"
            )
        terminal_digest = context.get("terminal_evidence_sha256")
        source_custody = context.get("source_custody")
        guard_receipt = context.get("guard_receipt")
        process_closure = envelope.get("process_closure")
        platform_custody = context.get("platform_process_custody")
        if not isinstance(terminal_digest, str) or not re.fullmatch(
            r"[0-9a-f]{64}", terminal_digest
        ):
            raise ValueError("passed proof run has no terminal evidence digest")
        if context.get("run_id") != row["run_id"]:
            raise ValueError("passed proof run receipt has a substituted run identity")
        nonce_hash = context.get("execution_nonce_sha256")
        if not isinstance(nonce_hash, str) or not re.fullmatch(
            r"[0-9a-f]{64}", nonce_hash
        ):
            raise ValueError("passed proof run has no execution nonce digest")
        if (
            not isinstance(source_custody, dict)
            or source_custody.get("evidence_eligible") is not True
        ):
            raise ValueError("passed proof run is not source-custody eligible")
        if not isinstance(guard_receipt, dict) or not isinstance(
            guard_receipt.get("sha256"), str
        ):
            raise ValueError("passed proof run has no guard receipt identity")
        if (
            sys.platform == "win32"
            and isinstance(process_closure, dict)
            and process_closure.get("descendants") == "declared-toolchains"
            and (
                not isinstance(platform_custody, dict)
                or platform_custody.get("identical") is not True
                or not isinstance(platform_custody.get("prelaunch"), list)
                or not platform_custody["prelaunch"]
            )
        ):
            raise ValueError(
                "passed proof run has no stable platform process-image custody"
            )
        expected_terminal = supervisor_custody.terminal_evidence_sha256(
            context,
            run_id=str(row["run_id"]),
            returncode=int(returncode),
        )
        if terminal_digest != expected_terminal:
            raise ValueError("passed proof run terminal evidence digest mismatch")
    environment = context.get("environment")
    if not isinstance(environment, dict):
        raise ValueError("proof run receipt environment is malformed")
    return {
        **context,
        "authority_kind": "proof-queue-dynamic-command",
        "family": "heavy_queue",
        "commands": [
            {
                "id": command_id,
                "family": "heavy_queue",
                "cell": f"{environment.get('os')}-{environment.get('arch')}-dynamic",
                "argv": argv,
                "cwd": row["cwd"],
                "dependencies": [],
                "tiers": ["queued-heavy"],
                "resource_class": row["resource_family"],
                "timeout_seconds": None,
                "started_at": row["started_at"],
                "duration_seconds": row["elapsed_s"] or 0.0,
                "peak_rss_bytes": _queue_peak_rss_bytes(row["summary_json"]),
                "cache_disposition": "unknown",
                "status": "success" if succeeded else status,
                "returncode": returncode,
            }
        ],
        "executed_partitions": [command_id] if succeeded else [],
        "status": "success" if succeeded else status,
    }


def _row_to_payload(
    row: sqlite3.Row,
) -> dict[str, object]:
    env_overrides = json.loads(row["env_json"])
    if not isinstance(env_overrides, dict) or any(
        not isinstance(name, str) or not isinstance(value, str)
        for name, value in env_overrides.items()
    ):
        raise ValueError(f"proof run {row['run_id']} has invalid env authority")
    payload = {
        "run_id": row["run_id"],
        "logical_id": row["logical_id"],
        "reason": row["reason"],
        "status": row["status"],
        "returncode": row["returncode"],
        "command": json.loads(row["command_json"]),
        "cwd": row["cwd"],
        "resource_family": row["resource_family"],
        "contention_key": row["contention_key"],
        "scopes": json.loads(row["scopes_json"]),
        "env_override_names": sorted(env_overrides, key=str.casefold),
        "git": json.loads(row["git_json"]),
        "log_path": row["log_path"],
        "summary_json": row["summary_json"],
        "guard_pid": row["guard_pid"],
        "started_at": row["started_at"],
        "finished_at": row["finished_at"],
        "elapsed_s": row["elapsed_s"],
    }
    authority_raw = state._row_value(row, "command_envelope_json")
    if isinstance(authority_raw, str) and authority_raw:
        authority = json.loads(authority_raw)
        if isinstance(authority, dict) and authority:
            payload["command_envelope"] = authority
    # A submission projection records intent, not execution evidence. Fingerprinting
    # the submitter here misattributes its toolchains to work that may execute on a
    # different host. Completion/running projections are regenerated by the runner
    # and attach the execution-host receipt then.
    receipt_context_raw = state._row_value(row, "receipt_context_json")
    if isinstance(receipt_context_raw, str) and receipt_context_raw:
        receipt_context = json.loads(receipt_context_raw)
        if (
            isinstance(receipt_context, dict)
            and receipt_context.get("schema") == state.UNATTESTED_RECEIPT_CONTEXT_SCHEMA
        ):
            payload["proof_receipt_state"] = receipt_context
        elif str(row["status"]) not in _NON_EXECUTED_RECEIPT_STATUSES:
            payload["proof_receipt"] = _queue_proof_receipt(row)
    return payload


def _write_failed_run_log(
    log_path: Path,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    lines: Sequence[str],
) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    append = log_path.exists() and log_path.stat().st_size > 0
    with log_path.open("a" if append else "w", encoding="utf-8") as log:
        if append:
            print("\n--- proof_queue terminal failure ---", file=log)
        print(f"proof_queue run_id={run_id}", file=log)
        print(f"logical_id={logical_id}", file=log)
        print(f"reason={reason}", file=log)
        print(f"cwd={repo_root}", file=log)
        print(f"command={shlex.join(command)}", file=log)
        print("", file=log)
        for line in lines:
            print(line, file=log)


def _write_queued_submission_log(
    log_path: Path,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    resource_family: str,
    contention_key: str,
    scopes: Sequence[str],
    env_overrides: Mapping[str, str],
    depends_on: Sequence[str],
) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    append = log_path.exists() and log_path.stat().st_size > 0
    with log_path.open("a" if append else "w", encoding="utf-8") as log:
        if append:
            print("\n--- proof_queue queued submission ---", file=log)
        print(f"proof_queue run_id={run_id}", file=log)
        print("status=queued", file=log)
        print(f"logical_id={logical_id}", file=log)
        print(f"reason={reason}", file=log)
        print(f"cwd={repo_root}", file=log)
        print(f"resource_family={resource_family}", file=log)
        print(f"contention_key={contention_key}", file=log)
        print(f"command={shlex.join(command)}", file=log)
        print(
            "command_envelope="
            + json.dumps(command_admission.admission_envelope(command), sort_keys=True),
            file=log,
        )
        if scopes:
            print(f"scopes={json.dumps(list(scopes), sort_keys=True)}", file=log)
        if env_overrides:
            print(
                "env_override_names="
                + json.dumps(sorted(env_overrides, key=str.casefold)),
                file=log,
            )
        if depends_on:
            print(
                f"depends_on={json.dumps(list(depends_on), sort_keys=True)}", file=log
            )
        print("", file=log)
        print("No proof command has launched for this queued row.", file=log)


def _append_queue_infra_log(
    log_path: Path,
    *,
    run_id: str,
    phase: str,
    exc: BaseException,
    fatal: bool,
) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    severity = "fatal" if fatal else "nonfatal"
    with log_path.open("a", encoding="utf-8") as log:
        print("", file=log)
        print(
            f"proof queue {severity} infrastructure failure during {phase}:",
            file=log,
        )
        print(f"run_id={run_id}", file=log)
        print(f"{type(exc).__name__}: {exc}", file=log)
        print("", file=log)
        traceback.print_exception(type(exc), exc, exc.__traceback__, file=log)


def _try_insert_queue_infra_note(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    log_path: Path,
    phase: str,
    exc: BaseException,
    fatal: bool,
) -> None:
    severity = "fatal" if fatal else "nonfatal"
    try:
        state._insert_note(
            conn,
            run_id=run_id,
            body=(
                f"queue {severity} infrastructure failure during {phase}: "
                f"{type(exc).__name__}: {exc}"
            ),
            kind="finding",
            author=state._default_note_author(),
        )
    except Exception as note_exc:
        _append_queue_infra_log(
            log_path,
            run_id=run_id,
            phase=f"{phase} note append",
            exc=note_exc,
            fatal=False,
        )


def _try_write_marimo_notebook(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    run_id: str,
    *,
    log_path: Path,
    phase: str,
    output: str | None = None,
) -> Path | None:
    try:
        return _write_marimo_notebook(args, conn, run_id, output)
    except Exception as exc:
        _append_queue_infra_log(
            log_path,
            run_id=run_id,
            phase=phase,
            exc=exc,
            fatal=False,
        )
        _try_insert_queue_infra_note(
            conn,
            run_id=run_id,
            log_path=log_path,
            phase=phase,
            exc=exc,
            fatal=False,
        )
        print(
            (
                f"warning: notebook projection failed for {run_id} during "
                f"{phase}; log: {log_path}"
            ),
            file=sys.stderr,
        )
        return None


def _fail_preexecution_run(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    log_path: Path,
    exc: BaseException,
    phase: str,
) -> int:
    now = state._utc_now()
    state._update_run(
        conn,
        run_id,
        status="failed",
        returncode=2,
        started_at=now,
        finished_at=now,
        elapsed_s=0.0,
        receipt_context_json=json.dumps(
            state._unattested_receipt_context(
                status="not-executed",
                phase=phase,
                reason=f"{type(exc).__name__}: {exc}",
            ),
            sort_keys=True,
        ),
    )
    lines = [
        f"proof queue fatal infrastructure failure during {phase}:",
        f"{type(exc).__name__}: {exc}",
        "",
        *traceback.format_exception(type(exc), exc, exc.__traceback__),
    ]
    _write_failed_run_log(
        log_path,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        repo_root=repo_root,
        command=command,
        lines=lines,
    )
    _try_insert_queue_infra_note(
        conn,
        run_id=run_id,
        log_path=log_path,
        phase=phase,
        exc=exc,
        fatal=True,
    )
    _try_write_marimo_notebook(
        args,
        conn,
        run_id,
        log_path=log_path,
        phase="terminal projection",
    )
    print(f"failed {run_id} rc=2")
    print(f"log: {log_path}")
    return 2


def _notebook_projection_expected(
    *,
    notes: list[dict[str, object]],
    dag: dict[str, list[dict[str, object]]],
) -> bool:
    return bool(notes or dag.get("parents") or dag.get("children"))
