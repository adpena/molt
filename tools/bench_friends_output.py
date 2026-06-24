import argparse
import datetime as dt
import json
import os
import platform
import subprocess
import sys
from pathlib import Path
from typing import Any

from bench_friends_context import REPO_ROOT
from bench_friends_phase import _bounded_failure_text
from bench_friends_types import (
    MAX_FAILURE_DETAIL_RECORDS,
    BenchInterrupted,
    PhaseResult,
    RunnerResult,
    SourceCustody,
    SuiteResult,
)

import harness_memory_guard
from molt import backend_daemon_custody as daemon_custody

def _git_rev() -> str | None:
    try:
        res = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None

def _format_optional(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}"


def _render_summary_markdown(
    *,
    run_started_at: str,
    manifest_path: Path,
    json_rel: str,
    suites: list[SuiteResult],
    interrupted: dict[str, Any] | None = None,
    backend_daemon_cleanup: list[dict[str, Any]] | None = None,
    memory_guard_incidents: list[dict[str, Any]] | None = None,
    custody_artifacts: dict[str, str] | None = None,
    molt_failure_details: dict[str, Any] | None = None,
) -> str:
    lines: list[str] = []
    lines.append("# Friend Benchmark Summary")
    lines.append("")
    lines.append(f"Generated: {run_started_at}")
    lines.append(f"Manifest: `{manifest_path}`")
    lines.append(f"JSON: `{json_rel}`")
    lines.append("")
    lines.append(
        "| Suite | Semantic Mode | Status | CPython s | PyPy s | Codon s | "
        "Nuitka s | Pyodide s | Friend s | Tinygrad s | NumPy s | Molt s | "
        "Molt/CPython | Molt/PyPy | Molt/Codon | Molt/Nuitka | Molt/Pyodide | "
        "Molt/Friend | Molt/NumPy |"
    )
    lines.append(
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | "
        "---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    )
    for suite in suites:
        m = suite.metrics
        lines.append(
            "| "
            f"{suite.id} | {suite.semantic_mode} | {suite.status} | "
            f"{_format_optional(m.get('cpython_median_s'))} | "
            f"{_format_optional(m.get('pypy_median_s'))} | "
            f"{_format_optional(m.get('codon_median_s'))} | "
            f"{_format_optional(m.get('nuitka_median_s'))} | "
            f"{_format_optional(m.get('pyodide_median_s'))} | "
            f"{_format_optional(m.get('friend_median_s'))} | "
            f"{_format_optional(m.get('tinygrad_median_s'))} | "
            f"{_format_optional(m.get('numpy_median_s'))} | "
            f"{_format_optional(m.get('molt_median_s'))} | "
            f"{_format_optional(m.get('molt_cpython_ratio'))} | "
            f"{_format_optional(m.get('molt_pypy_ratio'))} | "
            f"{_format_optional(m.get('molt_codon_ratio'))} | "
            f"{_format_optional(m.get('molt_nuitka_ratio'))} | "
            f"{_format_optional(m.get('molt_pyodide_ratio'))} | "
            f"{_format_optional(m.get('molt_vs_friend_speedup'))} | "
            f"{_format_optional(m.get('molt_vs_numpy_speedup'))} |"
        )

    lines.append("")
    lines.append("## Notes")
    lines.append(
        "- Semantic mode values: `runs_unmodified`, `requires_adapter`, "
        "`unsupported_by_molt`."
    )
    lines.append(
        "- Ratio columns (`Molt/*`) > 1.0 indicate Molt is faster on the suite median."
    )
    lines.append(
        "- Compile-vs-run separation is recorded per runner when build commands "
        "are configured."
    )

    artifacts = custody_artifacts or {}
    if artifacts:
        lines.append("")
        lines.append("## Custody Artifacts")
        for key in (
            "molt_failure_details_jsonl",
            "harness_command_profile_jsonl",
            "repo_process_sentinel_jsonl",
            "backend_daemon_cleanup_jsonl",
        ):
            value = artifacts.get(key)
            if value:
                lines.append(f"- `{key}`: `{value}`")

    failure_details = molt_failure_details or {}
    failure_records = failure_details.get("records", [])
    if isinstance(failure_records, list) and failure_records:
        lines.append("")
        lines.append("## Molt Failure Details")
        for record in failure_records:
            if not isinstance(record, dict):
                continue
            detail = record.get("detail")
            detail_text = f" detail=`{detail}`" if detail else ""
            lines.append(
                f"- `{record.get('suite')}` runner=`{record.get('runner')}` "
                f"phase=`{record.get('phase')}` status=`{record.get('status')}`"
                f"{detail_text}"
            )
            log_refs = record.get("log_refs")
            if isinstance(log_refs, list):
                for ref in log_refs[:4]:
                    if isinstance(ref, dict) and ref.get("path"):
                        lines.append(
                            f"  - {ref.get('kind', 'log')}: `{ref.get('path')}`"
                        )
        if failure_details.get("truncated"):
            lines.append(
                f"- Failure detail list truncated at {MAX_FAILURE_DETAIL_RECORDS} records."
            )

    failures = [s for s in suites if s.status != "ok"]
    if failures:
        lines.append("")
        lines.append("## Non-OK Suites")
        for suite in failures:
            reason = suite.reason or "no reason provided"
            lines.append(f"- `{suite.id}`: {suite.status} ({reason})")

    incidents = memory_guard_incidents or []
    if incidents:
        lines.append("")
        lines.append("## Memory Guard Incidents")
        for incident in incidents:
            violation = incident.get("violation")
            if not isinstance(violation, dict):
                violation = {}
            lines.append(
                f"- `{incident.get('event', 'incident')}` "
                f"reason=`{violation.get('reason', 'unknown')}` "
                f"pgid=`{violation.get('pgid', '')}` "
                f"rss=`{violation.get('peak_rss_gb', violation.get('total_rss_gb', ''))}`"
            )
    if interrupted is not None:
        lines.append("")
        lines.append("## Interruption")
        lines.append(
            f"- Signal: `{interrupted['signame']}` "
            f"({interrupted['signum']}); partial results were written."
        )
    cleanup_events = backend_daemon_cleanup or []
    if cleanup_events:
        lines.append("")
        lines.append("## Backend Daemon Cleanup")
        for event in cleanup_events:
            status = event.get("status", "unknown")
            reason = event.get("reason", "unknown")
            terminated = event.get("terminated_count", 0)
            lines.append(
                f"- `{status}` reason=`{reason}` terminated={terminated} "
                f"session=`{event.get('session_id', '')}`"
            )
    lines.append("")
    lines.append("Generated by `tools/bench_friends.py`.")
    return "\n".join(lines) + "\n"


def _runner_to_dict(result: RunnerResult) -> dict[str, Any]:
    return {
        "name": result.name,
        "role": result.role,
        "status": result.status,
        "reason": result.reason,
        "build": _phase_to_dict(result.build) if result.build else None,
        "runs": [_phase_to_dict(phase) for phase in result.runs],
        "run_samples_s": result.run_samples_s,
        "run_median_s": result.run_median_s,
        "run_mean_s": result.run_mean_s,
        "run_stdev_s": result.run_stdev_s,
        "structured_outputs": result.structured_outputs,
        "structured_samples_s": result.structured_samples_s,
        "structured_median_s": result.structured_median_s,
        "molt_failure": result.molt_failure,
    }


def _phase_from_dict(payload: dict[str, Any] | None) -> PhaseResult | None:
    if payload is None:
        return None
    return PhaseResult(
        cmd=list(payload.get("cmd") or []),
        returncode=int(payload.get("returncode", 0)),
        elapsed_s=float(payload.get("elapsed_s", 0.0)),
        timed_out=bool(payload.get("timed_out", False)),
        stdout_path=str(payload.get("stdout_path", "")),
        stderr_path=str(payload.get("stderr_path", "")),
        stdout_json=payload.get("stdout_json"),
        stdout_json_error=payload.get("stdout_json_error"),
        guard_status=payload.get("guard_status"),
        guard_violation=payload.get("guard_violation"),
        guard_limit_at_violation=payload.get("guard_limit_at_violation"),
        guard_orphaned_process_groups=list(
            payload.get("guard_orphaned_process_groups") or []
        ),
        guard_exit_signal=payload.get("guard_exit_signal"),
        guard_cargo_incremental_quarantine=payload.get(
            "guard_cargo_incremental_quarantine"
        ),
    )


def _phase_to_dict(phase: PhaseResult) -> dict[str, Any]:
    return {
        "cmd": phase.cmd,
        "returncode": phase.returncode,
        "elapsed_s": phase.elapsed_s,
        "timed_out": phase.timed_out,
        "stdout_path": phase.stdout_path,
        "stderr_path": phase.stderr_path,
        "stdout_json": phase.stdout_json,
        "stdout_json_error": phase.stdout_json_error,
        "guard_status": phase.guard_status,
        "guard_violation": phase.guard_violation,
        "guard_limit_at_violation": phase.guard_limit_at_violation,
        "guard_orphaned_process_groups": phase.guard_orphaned_process_groups,
        "guard_exit_signal": phase.guard_exit_signal,
        "guard_cargo_incremental_quarantine": phase.guard_cargo_incremental_quarantine,
        "molt_failure": phase.molt_failure,
    }


def _runner_from_dict(payload: dict[str, Any]) -> RunnerResult:
    return RunnerResult(
        name=str(payload["name"]),
        role=str(payload["role"]),
        status=str(payload["status"]),
        reason=payload.get("reason"),
        build=_phase_from_dict(payload.get("build")),
        runs=[
            phase
            for item in payload.get("runs", [])
            if (phase := _phase_from_dict(item)) is not None
        ],
        run_samples_s=[float(value) for value in payload.get("run_samples_s", [])],
        run_median_s=payload.get("run_median_s"),
        run_mean_s=payload.get("run_mean_s"),
        run_stdev_s=payload.get("run_stdev_s"),
        structured_outputs=list(payload.get("structured_outputs") or []),
        structured_samples_s=dict(payload.get("structured_samples_s") or {}),
        structured_median_s=dict(payload.get("structured_median_s") or {}),
    )


def _source_custody_to_dict(custody: SourceCustody) -> dict[str, Any]:
    return {
        "source": custody.source,
        "requested_ref": custody.requested_ref,
        "expected_ref": custody.expected_ref,
        "head_ref": custody.head_ref,
        "ref_verified": custody.ref_verified,
        "git_clean": custody.git_clean,
        "git_status_porcelain": custody.git_status_porcelain,
        "git_ignored_artifacts": custody.git_ignored_artifacts,
        "suite_root_overridden": custody.suite_root_overridden,
        "verification": custody.verification,
    }


def _source_custody_from_dict(payload: dict[str, Any]) -> SourceCustody:
    return SourceCustody(
        source=str(payload["source"]),
        requested_ref=payload.get("requested_ref"),
        expected_ref=payload.get("expected_ref"),
        head_ref=payload.get("head_ref"),
        ref_verified=payload.get("ref_verified"),
        git_clean=payload.get("git_clean"),
        git_status_porcelain=payload.get("git_status_porcelain"),
        git_ignored_artifacts=payload.get("git_ignored_artifacts"),
        suite_root_overridden=bool(payload.get("suite_root_overridden", False)),
        verification=str(payload["verification"]),
    )


def _suite_to_dict(suite: SuiteResult) -> dict[str, Any]:
    return {
        "id": suite.id,
        "friend": suite.friend,
        "display_name": suite.display_name,
        "semantic_mode": suite.semantic_mode,
        "source": suite.source,
        "suite_root": suite.suite_root,
        "suite_workdir": suite.suite_workdir,
        "resolved_ref": suite.resolved_ref,
        "requested_ref": suite.requested_ref,
        "source_custody": _source_custody_to_dict(suite.source_custody),
        "status": suite.status,
        "reason": suite.reason,
        "adapter_notes": suite.adapter_notes,
        "tags": suite.tags,
        "metrics": suite.metrics,
        "runners": {
            name: _runner_to_dict(result) for name, result in suite.runners.items()
        },
    }


def _suite_from_dict(payload: dict[str, Any]) -> SuiteResult:
    return SuiteResult(
        id=str(payload["id"]),
        friend=str(payload["friend"]),
        display_name=str(payload["display_name"]),
        semantic_mode=str(payload["semantic_mode"]),
        source=str(payload["source"]),
        suite_root=str(payload.get("suite_root", "")),
        suite_workdir=str(payload.get("suite_workdir", "")),
        resolved_ref=payload.get("resolved_ref"),
        requested_ref=payload.get("requested_ref"),
        source_custody=_source_custody_from_dict(payload["source_custody"]),
        status=str(payload["status"]),
        reason=payload.get("reason"),
        adapter_notes=payload.get("adapter_notes"),
        tags=list(payload.get("tags") or []),
        runners={
            str(name): _runner_from_dict(result)
            for name, result in dict(payload.get("runners") or {}).items()
        },
        metrics=dict(payload.get("metrics") or {}),
    )


def _render_existing_results_json(
    *,
    results_json: Path,
    summary_out: Path | None,
    update_doc: bool,
) -> tuple[Path, str]:
    payload = json.loads(results_json.read_text(encoding="utf-8"))
    if payload.get("schema_version") != 1:
        raise ValueError(
            f"unsupported friend benchmark results schema: {payload.get('schema_version')!r}"
        )
    suites = [_suite_from_dict(item) for item in payload.get("suites", [])]
    generated_at = str(payload["generated_at"])
    manifest_path = Path(str(payload["manifest_path"]))
    summary_path = (summary_out or (results_json.parent / "summary.md")).resolve()
    summary_path.parent.mkdir(parents=True, exist_ok=True)
    summary_text = _render_summary_markdown(
        run_started_at=generated_at,
        manifest_path=manifest_path,
        json_rel=str(results_json.resolve()),
        suites=suites,
        interrupted=payload.get("interrupted"),
        backend_daemon_cleanup=list(payload.get("backend_daemon_cleanup") or []),
        memory_guard_incidents=list(payload.get("memory_guard_incidents") or []),
    )
    summary_path.write_text(summary_text, encoding="utf-8")
    if update_doc:
        doc_out = Path("docs/benchmarks/friend_summary.md").resolve()
        doc_out.parent.mkdir(parents=True, exist_ok=True)
        doc_out.write_text(summary_text, encoding="utf-8")
    return summary_path, summary_text


def _append_event_jsonl(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, sort_keys=True) + "\n")


def _daemon_record_to_dict(
    record: daemon_custody.BackendDaemonIdentityRecord,
) -> dict[str, Any]:
    identity = record.identity
    return {
        "identity_path": str(record.path),
        "pid": identity.pid,
        "socket_path": str(identity.socket_path),
        "project_root": str(identity.project_root),
        "cargo_profile": identity.cargo_profile,
        "config_digest": identity.config_digest,
        "backend_bin": str(identity.backend_bin),
        "created_at": identity.created_at,
        "command": identity.command,
    }


def _cleanup_backend_daemons(
    *,
    run_env: dict[str, str],
    output_root: Path,
    reason: str,
) -> dict[str, Any]:
    event: dict[str, Any] = {
        "schema_version": 1,
        "event": "bench_friends_backend_daemon_cleanup",
        "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "reason": reason,
        "session_id": run_env.get("MOLT_SESSION_ID", ""),
        "project_root": str(REPO_ROOT),
        "status": "ok",
        "terminated": [],
        "terminated_count": 0,
    }
    try:
        terminated = daemon_custody.terminate_backend_daemons_for_session(
            run_env,
            project_root=REPO_ROOT,
            grace=1.0,
        )
    except Exception as exc:  # noqa: BLE001
        event["status"] = "failed"
        event["error"] = str(exc)
        print(
            "bench_friends: backend daemon cleanup failed: "
            f"reason={reason} error={exc}",
            file=sys.stderr,
        )
    else:
        event["terminated"] = [_daemon_record_to_dict(record) for record in terminated]
        event["terminated_count"] = len(terminated)
        if terminated:
            pids = ",".join(str(record.identity.pid) for record in terminated)
            print(
                "bench_friends: cleaned backend daemons: "
                f"reason={reason} count={len(terminated)} pids={pids}",
                file=sys.stderr,
            )
    _append_event_jsonl(
        output_root / "memory_guard" / "backend_daemon_cleanup.jsonl", event
    )
    return event


def _interrupted_payload(interrupted: BenchInterrupted | None) -> dict[str, Any] | None:
    if interrupted is None:
        return None
    return {
        "signum": interrupted.signum,
        "signame": interrupted.signame,
        "returncode": 128 + interrupted.signum,
        "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    }


def _failure_details_path(json_out: Path) -> Path:
    if json_out.name == "results.json":
        return json_out.with_name("molt_failure_details.jsonl")
    return json_out.with_name(f"{json_out.stem}_molt_failure_details.jsonl")


def _custody_artifacts(
    *,
    output_root: Path,
    json_out: Path,
    summary_out: Path,
    failure_details_path: Path,
    run_env: dict[str, str],
) -> dict[str, str]:
    memory_guard_root = output_root / "memory_guard"
    return {
        "results_json": str(json_out),
        "summary_md": str(summary_out),
        "molt_failure_details_jsonl": str(failure_details_path),
        "harness_command_profile_jsonl": str(
            harness_memory_guard.command_profile_log_path(run_env, repo_root=REPO_ROOT)
        ),
        "repo_process_sentinel_jsonl": str(
            memory_guard_root / "bench_friends_sentinel.jsonl"
        ),
        "backend_daemon_cleanup_jsonl": str(
            memory_guard_root / "backend_daemon_cleanup.jsonl"
        ),
    }


def _molt_failure_detail_records(
    suites: list[SuiteResult],
) -> dict[str, Any]:
    records: list[dict[str, Any]] = []
    total = 0
    for suite in suites:
        for runner_name, runner in sorted(suite.runners.items()):
            failure = runner.molt_failure
            if not isinstance(failure, dict):
                continue
            total += 1
            if len(records) >= MAX_FAILURE_DETAIL_RECORDS:
                continue
            records.append(
                {
                    "suite": suite.id,
                    "runner": runner_name,
                    "phase": failure.get("phase"),
                    "status": failure.get("status"),
                    "detail": failure.get("detail"),
                    "returncode": failure.get("returncode"),
                    "timed_out": failure.get("timed_out"),
                    "elapsed_s": failure.get("elapsed_s"),
                    "message": _bounded_failure_text(failure.get("message")),
                    "guard_violation": failure.get("guard_violation"),
                    "signal": failure.get("signal"),
                    "orphaned_process_groups": failure.get("orphaned_process_groups"),
                    "log_refs": failure.get("log_refs", []),
                }
            )
    return {
        "schema_version": 1,
        "total": total,
        "truncated": total > len(records),
        "max_records": MAX_FAILURE_DETAIL_RECORDS,
        "records": records,
    }


def _write_failure_details_jsonl(
    path: Path,
    failure_details: dict[str, Any],
) -> None:
    records = failure_details.get("records", [])
    if not isinstance(records, list):
        records = []
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for record in records:
            if isinstance(record, dict):
                handle.write(json.dumps(record, sort_keys=True) + "\n")


def _write_run_outputs(
    *,
    output_root: Path,
    args: argparse.Namespace,
    metadata: dict[str, Any],
    manifest_path: Path,
    run_started: dt.datetime,
    runner_filters: set[str],
    suite_root_overrides: dict[str, Path],
    repo_ref_overrides: dict[str, str],
    suite_results: list[SuiteResult],
    limits: harness_memory_guard.HarnessMemoryLimits,
    interrupted: BenchInterrupted | None,
    backend_daemon_cleanup: list[dict[str, Any]],
    memory_guard_incidents: list[dict[str, Any]],
    run_env: dict[str, str],
) -> tuple[Path, Path, str]:
    json_out = (args.json_out or (output_root / "results.json")).resolve()
    json_out.parent.mkdir(parents=True, exist_ok=True)
    summary_out = (args.summary_out or (output_root / "summary.md")).resolve()
    failure_details_path = _failure_details_path(json_out).resolve()
    custody_artifact_refs = _custody_artifacts(
        output_root=output_root,
        json_out=json_out,
        summary_out=summary_out,
        failure_details_path=failure_details_path,
        run_env=run_env,
    )
    molt_failure_details = _molt_failure_detail_records(suite_results)
    interrupt_payload = _interrupted_payload(interrupted)
    payload = {
        "schema_version": 1,
        "manifest_schema_version": metadata["schema_version"],
        "generated_at": run_started.isoformat(),
        "manifest_path": str(manifest_path),
        "git_rev": _git_rev(),
        "dry_run": args.dry_run,
        "partial": interrupted is not None,
        "interrupted": interrupt_payload,
        "backend_daemon_cleanup": backend_daemon_cleanup,
        "memory_guard_incidents": memory_guard_incidents,
        "custody_artifacts": custody_artifact_refs,
        "molt_failure_details": molt_failure_details,
        "memory_guard": harness_memory_guard.limits_summary(limits),
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
        },
        "options": {
            "include_disabled": args.include_disabled,
            "checkout": args.checkout,
            "fetch": args.fetch,
            "repeat_override": args.repeat,
            "timeout_override": args.timeout_sec,
            "runner_filter": sorted(runner_filters),
            "suite_root_overrides": {
                suite_id: str(path)
                for suite_id, path in sorted(suite_root_overrides.items())
            },
            "repo_ref_overrides": dict(sorted(repo_ref_overrides.items())),
        },
        "suites": [_suite_to_dict(suite) for suite in suite_results],
    }
    _write_failure_details_jsonl(failure_details_path, molt_failure_details)
    json_out.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")

    summary_out.parent.mkdir(parents=True, exist_ok=True)
    summary_text = _render_summary_markdown(
        run_started_at=run_started.isoformat(),
        manifest_path=manifest_path,
        json_rel=str(json_out),
        suites=suite_results,
        interrupted=interrupt_payload,
        backend_daemon_cleanup=backend_daemon_cleanup,
        memory_guard_incidents=memory_guard_incidents,
        custody_artifacts=custody_artifact_refs,
        molt_failure_details=molt_failure_details,
    )
    summary_out.write_text(summary_text, encoding="utf-8")
    return json_out, summary_out, summary_text
