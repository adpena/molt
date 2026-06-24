# ruff: noqa: E402
import argparse
import datetime as dt
import json
import os
import sys
import threading
from dataclasses import replace
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
_TOOLS_ROOT = Path(__file__).resolve().parent
_SRC_ROOT = REPO_ROOT / "src"
for _path_root in (_TOOLS_ROOT, _SRC_ROOT):
    _path_text = str(_path_root)
    if _path_root.exists() and _path_text not in sys.path:
        sys.path.insert(0, _path_text)

import harness_memory_guard
import bench as bench_tool
from molt import backend_daemon_custody as daemon_custody

from bench_friends_context import REPO_ROOT
from bench_friends_types import (
    MAX_FAILURE_DETAIL_RECORDS,
    MAX_FAILURE_MESSAGE_CHARS,
    RUNNER_NAME_RE,
    SUPPORTED_RUNNER_ROLES,
    SUPPORTED_SEMANTIC_MODES,
    BenchInterrupted,
    BenchSignalScope,
    PhaseResult,
    RunnerResult,
    RunnerSpec,
    SourceCustody,
    SuiteAcquisition,
    SuiteResult,
    SuiteSpec,
)
from bench_friends_env import (
    _base_run_env,
    _default_output_root,
    _emit_progress,
    _external_root,
    _materialize_output_env_paths,
    _path_is_under,
    _project_python,
)
from bench_friends_manifest import (
    _apply_runner_filter,
    _load_manifest,
    _optional_str,
    _parse_command_list,
    _parse_env,
    _parse_keyed_path_overrides,
    _parse_keyed_str_overrides,
    _parse_runners,
    _parse_single_command,
    _parse_suite,
    _resolve_env,
    _resolve_tokenized,
    _select_suites,
    _validate_override_targets,
)
from bench_friends_phase import (
    _as_float,
    _bounded_failure_text,
    _extract_structured_elapsed,
    _guard_status,
    _guarded_phase_diagnostics,
    _molt_failure_reason_suffix,
    _molt_failure_with_log_refs,
    _parse_stdout_json,
    _rss_record_payload,
    _run_command,
    _metric_slug,
)
from bench_friends_custody import (
    _acquire_suite,
    _combine_suite_reasons,
    _is_placeholder_ref,
    _post_run_source_custody_failure_reason,
    _run_git,
    _verify_git_source_custody,
)
from bench_friends_runner import (
    _run_prepare_steps,
    _run_runner,
    _suite_metrics,
    _suite_status,
)
from bench_friends_output import (
    _append_event_jsonl,
    _cleanup_backend_daemons,
    _custody_artifacts,
    _daemon_record_to_dict,
    _failure_details_path,
    _format_optional,
    _git_rev,
    _interrupted_payload,
    _molt_failure_detail_records,
    _phase_from_dict,
    _phase_to_dict,
    _render_existing_results_json,
    _render_summary_markdown,
    _runner_from_dict,
    _runner_to_dict,
    _source_custody_from_dict,
    _source_custody_to_dict,
    _suite_from_dict,
    _suite_to_dict,
    _write_failure_details_jsonl,
    _write_run_outputs,
)

__all__ = [
    "REPO_ROOT",
    "SUPPORTED_SEMANTIC_MODES",
    "SUPPORTED_RUNNER_ROLES",
    "RUNNER_NAME_RE",
    "MAX_FAILURE_DETAIL_RECORDS",
    "MAX_FAILURE_MESSAGE_CHARS",
    "RunnerSpec",
    "SourceCustody",
    "SuiteAcquisition",
    "SuiteSpec",
    "PhaseResult",
    "RunnerResult",
    "SuiteResult",
    "BenchInterrupted",
    "BenchSignalScope",
    "_emit_progress",
    "_external_root",
    "_default_output_root",
    "_project_python",
    "_path_is_under",
    "_materialize_output_env_paths",
    "_base_run_env",
    "_load_manifest",
    "_parse_suite",
    "_optional_str",
    "_parse_env",
    "_parse_command_list",
    "_parse_runners",
    "_parse_single_command",
    "_resolve_tokenized",
    "_resolve_env",
    "_select_suites",
    "_parse_keyed_path_overrides",
    "_parse_keyed_str_overrides",
    "_validate_override_targets",
    "_apply_runner_filter",
    "_parse_stdout_json",
    "_metric_slug",
    "_as_float",
    "_extract_structured_elapsed",
    "_rss_record_payload",
    "_guard_status",
    "_guarded_phase_diagnostics",
    "_molt_failure_reason_suffix",
    "_bounded_failure_text",
    "_molt_failure_with_log_refs",
    "_run_command",
    "_run_git",
    "_is_placeholder_ref",
    "_verify_git_source_custody",
    "_acquire_suite",
    "_post_run_source_custody_failure_reason",
    "_combine_suite_reasons",
    "_run_prepare_steps",
    "_run_runner",
    "_suite_metrics",
    "_suite_status",
    "_git_rev",
    "_format_optional",
    "_render_summary_markdown",
    "_runner_to_dict",
    "_phase_from_dict",
    "_phase_to_dict",
    "_runner_from_dict",
    "_source_custody_to_dict",
    "_source_custody_from_dict",
    "_suite_to_dict",
    "_suite_from_dict",
    "_render_existing_results_json",
    "_append_event_jsonl",
    "_daemon_record_to_dict",
    "_cleanup_backend_daemons",
    "_interrupted_payload",
    "_failure_details_path",
    "_custody_artifacts",
    "_molt_failure_detail_records",
    "_write_failure_details_jsonl",
    "_write_run_outputs",
    "_parser",
    "main",
    "os",
    "sys",
    "harness_memory_guard",
    "bench_tool",
    "daemon_custody",
]

def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run Molt against friend-owned benchmark suites using a pinned "
            "manifest and reproducible command protocol."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("bench/friends/manifest.toml"),
        help="Path to friend benchmark manifest TOML.",
    )
    parser.add_argument(
        "--suite",
        action="append",
        default=[],
        help="Run only selected suite id (repeatable).",
    )
    parser.add_argument(
        "--include-disabled",
        action="store_true",
        help="Include suites marked enabled=false in manifest.",
    )
    parser.add_argument(
        "--runner",
        action="append",
        default=[],
        help="Run only selected runner name (repeatable).",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=None,
        help="Output root directory. Default: bench/results/friends/<timestamp>.",
    )
    parser.add_argument(
        "--repos-root",
        type=Path,
        default=Path("bench/friends/repos"),
        help="Local cache root for git-based friend suites.",
    )
    parser.add_argument(
        "--suite-root",
        action="append",
        default=[],
        metavar="SUITE=PATH",
        help=(
            "Override a suite root with an explicit checkout/path. Repeatable; "
            "git suites still require clean-tree and repo-ref verification."
        ),
    )
    parser.add_argument(
        "--repo-ref",
        action="append",
        default=[],
        metavar="SUITE=REF",
        help=(
            "Override a git suite repo_ref without editing the manifest. Repeatable; "
            "the resolved ref must match checked-out HEAD."
        ),
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=None,
        help="Override repeat count for all suites.",
    )
    parser.add_argument(
        "--timeout-sec",
        type=int,
        default=None,
        help="Override command timeout for all suites.",
    )
    parser.add_argument(
        "--checkout",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Clone/checkout/fetch git suites as needed.",
    )
    parser.add_argument(
        "--fetch",
        action="store_true",
        help="Fetch updates before checkout for git suites.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Plan and emit artifacts without executing real commands.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop after the first suite failure.",
    )
    parser.add_argument(
        "--summary-out",
        type=Path,
        default=None,
        help="Override summary markdown output path.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Override JSON output path.",
    )
    parser.add_argument(
        "--render-existing-json",
        type=Path,
        default=None,
        help=(
            "Render summary markdown from an existing friend results.json without "
            "running benchmark workloads or rewriting the JSON artifact."
        ),
    )
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help="Also write docs/benchmarks/friend_summary.md from this run.",
    )
    return parser


def main() -> int:
    args = _parser().parse_args()
    if args.render_existing_json is not None:
        incompatible: list[str] = []
        if args.suite:
            incompatible.append("--suite")
        if args.include_disabled:
            incompatible.append("--include-disabled")
        if args.runner:
            incompatible.append("--runner")
        if args.output_root is not None:
            incompatible.append("--output-root")
        if args.suite_root:
            incompatible.append("--suite-root")
        if args.repo_ref:
            incompatible.append("--repo-ref")
        if args.repeat is not None:
            incompatible.append("--repeat")
        if args.timeout_sec is not None:
            incompatible.append("--timeout-sec")
        if args.checkout is not None:
            incompatible.append("--checkout/--no-checkout")
        if args.fetch:
            incompatible.append("--fetch")
        if args.dry_run:
            incompatible.append("--dry-run")
        if args.fail_fast:
            incompatible.append("--fail-fast")
        if args.json_out is not None:
            incompatible.append("--json-out")
        if incompatible:
            joined = ", ".join(sorted(incompatible))
            print(
                f"--render-existing-json cannot be combined with workload options: {joined}",
                file=sys.stderr,
            )
            return 2
        try:
            summary_out, _summary_text = _render_existing_results_json(
                results_json=args.render_existing_json.resolve(),
                summary_out=args.summary_out,
                update_doc=args.update_doc,
            )
        except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
            print(f"failed to render existing friend results: {exc}", file=sys.stderr)
            return 2
        print(f"Rendered summary: {summary_out}")
        if args.update_doc:
            print("Updated docs/benchmarks/friend_summary.md")
        return 0

    if args.checkout is None:
        args.checkout = True

    manifest_path = args.manifest.resolve()
    metadata, suites = _load_manifest(manifest_path)
    suites_by_id = {suite.id: suite for suite in suites}
    try:
        suite_root_overrides = _parse_keyed_path_overrides(
            args.suite_root, "--suite-root"
        )
        repo_ref_overrides = _parse_keyed_str_overrides(args.repo_ref, "--repo-ref")
        _validate_override_targets(
            suites_by_id=suites_by_id,
            suite_root_overrides=suite_root_overrides,
            repo_ref_overrides=repo_ref_overrides,
        )
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    selected = _select_suites(
        suites,
        suite_filters=set(args.suite),
        include_disabled=args.include_disabled,
    )
    if not selected:
        print("No suites selected. Use --include-disabled or --suite.", file=sys.stderr)
        return 2
    runner_filters = {runner.strip() for runner in args.runner if runner.strip()}
    if any(not runner.strip() for runner in args.runner):
        print("--runner must not be empty", file=sys.stderr)
        return 2
    try:
        selected = [_apply_runner_filter(suite, runner_filters) for suite in selected]
    except ValueError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    if args.repeat is not None and args.repeat <= 0:
        print("--repeat must be positive", file=sys.stderr)
        return 2
    if args.timeout_sec is not None and args.timeout_sec <= 0:
        print("--timeout-sec must be positive", file=sys.stderr)
        return 2

    run_started = dt.datetime.now(dt.timezone.utc)
    output_root = (args.output_root or _default_output_root()).resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    repos_root = args.repos_root.resolve()

    run_env = _base_run_env()
    run_env.setdefault(
        "MOLT_GUARD_PROFILE_LOG",
        str(output_root / "memory_guard" / "commands.jsonl"),
    )
    limits = harness_memory_guard.limits_from_env("MOLT_BENCH", run_env)

    suite_results: list[SuiteResult] = []
    backend_daemon_cleanup: list[dict[str, Any]] = []
    memory_guard_incidents: list[dict[str, Any]] = []
    output_lock = threading.RLock()
    interrupted: BenchInterrupted | None = None
    overall_rc = 0

    def write_outputs_locked() -> tuple[Path, Path, str]:
        with output_lock:
            return _write_run_outputs(
                output_root=output_root,
                args=args,
                metadata=metadata,
                manifest_path=manifest_path,
                run_started=run_started,
                runner_filters=runner_filters,
                suite_root_overrides=suite_root_overrides,
                repo_ref_overrides=repo_ref_overrides,
                suite_results=list(suite_results),
                limits=limits,
                interrupted=interrupted,
                backend_daemon_cleanup=list(backend_daemon_cleanup),
                memory_guard_incidents=list(memory_guard_incidents),
                run_env=run_env,
            )

    def record_sentinel_violation(
        _violation: Any,
        _limits: Any,
        payload: dict[str, Any],
    ) -> None:
        incident = {
            "event": payload.get("event", "repo_process_guard_tripped"),
            "recorded_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "guard_started_at": payload.get("guard_started_at"),
            "observed_at": payload.get("observed_at"),
            "elapsed_s": payload.get("elapsed_s"),
            "violation": payload.get("violation"),
            "limits": payload.get("limits"),
            "active_pgids": payload.get("active_pgids"),
            "kill_scope": payload.get("kill_scope"),
            "victim_pgid": payload.get("victim_pgid"),
            "victim_command": payload.get("victim_command"),
            "action": payload.get("action"),
        }
        with output_lock:
            memory_guard_incidents.append(incident)
            _write_run_outputs(
                output_root=output_root,
                args=args,
                metadata=metadata,
                manifest_path=manifest_path,
                run_started=run_started,
                runner_filters=runner_filters,
                suite_root_overrides=suite_root_overrides,
                repo_ref_overrides=repo_ref_overrides,
                suite_results=list(suite_results),
                limits=limits,
                interrupted=interrupted,
                backend_daemon_cleanup=list(backend_daemon_cleanup),
                memory_guard_incidents=list(memory_guard_incidents),
                run_env=run_env,
            )

    try:
        with BenchSignalScope():
            with harness_memory_guard.repo_process_sentinel(
                repo_root=REPO_ROOT,
                artifact_root=output_root,
                label="bench_friends",
                limits=limits,
                on_violation=record_sentinel_violation,
            ):
                try:
                    for suite in selected:
                        suite_timeout = args.timeout_sec or suite.timeout_sec
                        suite_repeat = args.repeat or suite.repeat
                        suite = SuiteSpec(
                            **{
                                **suite.__dict__,
                                "timeout_sec": suite_timeout,
                                "repeat": suite_repeat,
                            }
                        )
                        if suite.id in repo_ref_overrides:
                            suite = replace(
                                suite, repo_ref=repo_ref_overrides[suite.id]
                            )
                        try:
                            acquisition = _acquire_suite(
                                suite,
                                repos_root=repos_root,
                                suite_root_override=suite_root_overrides.get(suite.id),
                                checkout=args.checkout,
                                fetch=args.fetch,
                                timeout_sec=suite.timeout_sec,
                                dry_run=args.dry_run,
                                limits=limits,
                            )
                            suite_root = acquisition.suite_root
                            suite_workdir = acquisition.suite_workdir
                            source_custody = acquisition.custody
                            resolved_ref = source_custody.head_ref
                            suite_logs = output_root / "logs" / suite.id
                            tokens = {
                                "repo_root": str(Path.cwd().resolve()),
                                "suite_root": str(suite_root.resolve()),
                                "suite_workdir": str(suite_workdir.resolve()),
                                "output_root": str(output_root),
                                "pathsep": os.pathsep,
                                "python": sys.executable,
                                "project_python": _project_python(),
                            }
                            suite_env = run_env.copy()
                            suite_env.update(_resolve_env(suite.env, tokens))
                            if not args.dry_run:
                                _materialize_output_env_paths(
                                    suite_env,
                                    output_root=output_root,
                                )
                            prep_ok, prep_reason = _run_prepare_steps(
                                suite,
                                suite_workdir=suite_workdir,
                                suite_env=suite_env,
                                tokens=tokens,
                                timeout_sec=suite.timeout_sec,
                                logs_dir=suite_logs,
                                dry_run=args.dry_run,
                                limits=limits,
                            )
                            runners: dict[str, RunnerResult] = {}
                            if prep_ok:
                                for runner_name, runner_spec in suite.runners.items():
                                    runners[runner_name] = _run_runner(
                                        runner_spec,
                                        suite=suite,
                                        suite_workdir=suite_workdir,
                                        suite_env=suite_env,
                                        tokens=tokens,
                                        logs_dir=suite_logs,
                                        dry_run=args.dry_run,
                                        limits=limits,
                                    )
                            else:
                                for runner_name in suite.runners:
                                    runners[runner_name] = RunnerResult(
                                        name=runner_name,
                                        role=suite.runners[runner_name].role,
                                        status="failed",
                                        reason=prep_reason,
                                    )
                            post_run_source_custody = source_custody
                            post_run_custody_reason = None
                            if suite.source == "git" and not args.dry_run:
                                try:
                                    post_run_source_custody = _verify_git_source_custody(
                                        suite,
                                        repo_dir=suite_root,
                                        requested_ref=suite.repo_ref
                                        or source_custody.requested_ref
                                        or "",
                                        timeout_sec=suite.timeout_sec,
                                        dry_run=args.dry_run,
                                        limits=limits,
                                        suite_root_overridden=source_custody.suite_root_overridden,
                                        verification="post_run_git_ref_and_clean_tree",
                                        raise_on_dirty=False,
                                    )
                                    post_run_custody_reason = (
                                        _post_run_source_custody_failure_reason(
                                            suite,
                                            post_run_source_custody,
                                        )
                                    )
                                except Exception as exc:  # noqa: BLE001
                                    post_run_source_custody = replace(
                                        source_custody,
                                        git_clean=False,
                                        verification="post_run_git_ref_and_clean_tree_failed",
                                    )
                                    post_run_custody_reason = (
                                        f"suite {suite.id}: post-run source "
                                        f"custody check failed: {exc}"
                                    )
                            status, reason = _suite_status(runners)
                            if prep_reason and not reason:
                                reason = prep_reason
                            if post_run_custody_reason:
                                status = "failed"
                                reason = _combine_suite_reasons(
                                    reason,
                                    post_run_custody_reason,
                                )
                            metrics = _suite_metrics(runners)
                            suite_result = SuiteResult(
                                id=suite.id,
                                friend=suite.friend,
                                display_name=suite.display_name,
                                semantic_mode=suite.semantic_mode,
                                source=suite.source,
                                suite_root=str(suite_root),
                                suite_workdir=str(suite_workdir),
                                resolved_ref=resolved_ref,
                                requested_ref=post_run_source_custody.requested_ref,
                                source_custody=post_run_source_custody,
                                status=status,
                                reason=reason,
                                adapter_notes=suite.adapter_notes,
                                tags=suite.tags,
                                runners=runners,
                                metrics=metrics,
                            )
                            suite_results.append(suite_result)
                            if status == "failed":
                                overall_rc = 1
                                if args.fail_fast:
                                    break
                        except Exception as exc:  # noqa: BLE001
                            suite_result = SuiteResult(
                                id=suite.id,
                                friend=suite.friend,
                                display_name=suite.display_name,
                                semantic_mode=suite.semantic_mode,
                                source=suite.source,
                                suite_root="",
                                suite_workdir="",
                                resolved_ref=None,
                                requested_ref=suite.repo_ref,
                                source_custody=SourceCustody(
                                    source=suite.source,
                                    requested_ref=suite.repo_ref,
                                    expected_ref=None,
                                    head_ref=None,
                                    ref_verified=False
                                    if suite.source == "git"
                                    else None,
                                    git_clean=False if suite.source == "git" else None,
                                    git_status_porcelain=None,
                                    git_ignored_artifacts=None,
                                    suite_root_overridden=suite.id
                                    in suite_root_overrides,
                                    verification="not_acquired",
                                ),
                                status="failed",
                                reason=str(exc),
                                adapter_notes=suite.adapter_notes,
                                tags=suite.tags,
                                runners={},
                                metrics=_suite_metrics({}),
                            )
                            suite_results.append(suite_result)
                            overall_rc = 1
                            if args.fail_fast:
                                break
                except BenchInterrupted as exc:
                    interrupted = exc
                    raise
                finally:
                    cleanup_reason = (
                        "interrupted" if interrupted is not None else "harness_exit"
                    )
                    backend_daemon_cleanup.append(
                        _cleanup_backend_daemons(
                            run_env=run_env,
                            output_root=output_root,
                            reason=cleanup_reason,
                        )
                    )
    except BenchInterrupted as exc:
        interrupted = exc
        overall_rc = 128 + exc.signum
        print(f"bench_friends: interrupted by {exc.signame}", file=sys.stderr)

    if any(event.get("status") == "failed" for event in backend_daemon_cleanup):
        overall_rc = overall_rc or 1

    json_out, summary_out, summary_text = write_outputs_locked()

    if args.update_doc:
        doc_out = Path("docs/benchmarks/friend_summary.md").resolve()
        doc_out.parent.mkdir(parents=True, exist_ok=True)
        doc_out.write_text(summary_text, encoding="utf-8")

    print(f"Wrote JSON: {json_out}")
    print(f"Wrote summary: {summary_out}")
    if args.update_doc:
        print("Updated docs/benchmarks/friend_summary.md")
    return overall_rc


if __name__ == "__main__":
    raise SystemExit(main())
