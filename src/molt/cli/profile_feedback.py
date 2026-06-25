from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from molt.cli.models import PgoProfileSummary, RuntimeFeedbackSummary
from molt.cli.output import fail as _fail


def _pgo_hotspot_entries(
    hotspots: Any, warnings: list[str]
) -> list[tuple[str, float | None]]:
    entries: list[tuple[str, float | None]] = []
    if hotspots is None:
        return entries
    if isinstance(hotspots, dict):
        for name, score in hotspots.items():
            if not isinstance(name, str) or not name:
                continue
            score_val = score if isinstance(score, (int, float)) else None
            entries.append((name, float(score_val) if score_val is not None else None))
        return entries
    if isinstance(hotspots, list):
        for entry in hotspots:
            if isinstance(entry, str) and entry:
                entries.append((entry, None))
                continue
            if isinstance(entry, (list, tuple)) and entry:
                name = entry[0]
                score = entry[1] if len(entry) > 1 else None
                if isinstance(name, str) and name:
                    score_val = score if isinstance(score, (int, float)) else None
                    entries.append(
                        (name, float(score_val) if score_val is not None else None)
                    )
                continue
            if isinstance(entry, dict):
                name = (
                    entry.get("symbol")
                    or entry.get("name")
                    or entry.get("func")
                    or entry.get("function")
                )
                if not isinstance(name, str) or not name:
                    continue
                score = entry.get("score")
                if score is None:
                    score = entry.get("time_ms")
                if score is None:
                    score = entry.get("time_us")
                if score is None:
                    score = entry.get("count")
                score_val = score if isinstance(score, (int, float)) else None
                entries.append(
                    (name, float(score_val) if score_val is not None else None)
                )
                continue
        return entries
    warnings.append("PGO profile hotspots must be a list or object; ignoring.")
    return entries


def _extract_hot_functions(profile: dict[str, Any], warnings: list[str]) -> list[str]:
    entries = _pgo_hotspot_entries(profile.get("hotspots"), warnings)
    if not entries:
        return []
    has_score = any(score is not None for _, score in entries)
    if has_score:
        entries = sorted(
            entries,
            key=lambda item: (-(item[1] or 0.0), item[0]),
        )
    else:
        entries = sorted(entries, key=lambda item: item[0])
    seen: set[str] = set()
    hot: list[str] = []
    for name, _score in entries:
        if name in seen:
            continue
        seen.add(name)
        hot.append(name)
    return hot


def _extract_runtime_feedback_hot_functions(
    payload: dict[str, Any], warnings: list[str]
) -> list[str]:
    raw = payload.get("hot_functions")
    if raw is None:
        return []
    entries = _pgo_hotspot_entries(raw, warnings)
    if not entries:
        return []
    has_score = any(score is not None for _, score in entries)
    if has_score:
        entries = sorted(entries, key=lambda item: (-(item[1] or 0.0), item[0]))
    else:
        entries = sorted(entries, key=lambda item: item[0])
    seen: set[str] = set()
    hot: list[str] = []
    for name, _score in entries:
        if name in seen:
            continue
        seen.add(name)
        hot.append(name)
    return hot


def _load_pgo_profile(
    project_root: Path,
    profile_path: str,
    warnings: list[str],
    json_output: bool,
    command: str,
) -> tuple[PgoProfileSummary | None, Path | None, int | None]:
    path = Path(profile_path).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    if not path.exists():
        return (
            None,
            None,
            _fail(f"PGO profile not found: {path}", json_output, command=command),
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            _fail(
                f"Failed to read PGO profile {path}: {exc}",
                json_output,
                command=command,
            ),
        )
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile JSON at {path}:{exc.lineno}:{exc.colno}: {exc.msg}",
                json_output,
                command=command,
            ),
        )
    if not isinstance(payload, dict):
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: expected a JSON object.",
                json_output,
                command=command,
            ),
        )
    errors: list[str] = []
    version = payload.get("molt_profile_version")
    if not isinstance(version, str):
        errors.append("missing molt_profile_version")
    elif version != "0.1":
        errors.append(f"unsupported molt_profile_version {version}")
    python_impl = payload.get("python_implementation")
    if not isinstance(python_impl, str) or not python_impl:
        errors.append("missing python_implementation")
    python_version = payload.get("python_version")
    if not isinstance(python_version, str) or not python_version:
        errors.append("missing python_version")
    platform_meta = payload.get("platform")
    if not isinstance(platform_meta, dict):
        errors.append("missing platform")
    else:
        if not isinstance(platform_meta.get("os"), str):
            errors.append("platform.os must be a string")
        if not isinstance(platform_meta.get("arch"), str):
            errors.append("platform.arch must be a string")
    run_meta = payload.get("run_metadata")
    if not isinstance(run_meta, dict):
        errors.append("missing run_metadata")
    else:
        if not isinstance(run_meta.get("entrypoint"), str):
            errors.append("run_metadata.entrypoint must be a string")
        argv = run_meta.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            errors.append("run_metadata.argv must be a list of strings")
        if not isinstance(run_meta.get("env_fingerprint"), str):
            errors.append("run_metadata.env_fingerprint must be a string")
        if not isinstance(run_meta.get("inputs_fingerprint"), str):
            errors.append("run_metadata.inputs_fingerprint must be a string")
        duration_ms = run_meta.get("duration_ms")
        if not isinstance(duration_ms, (int, float)) or duration_ms < 0:
            errors.append("run_metadata.duration_ms must be a non-negative number")
    if errors:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: " + "; ".join(errors),
                json_output,
                command=command,
            ),
        )
    hot_functions = _extract_hot_functions(payload, warnings)
    # Extract branch-level PGO counters (optional).
    branch_counts: dict[str, dict[str, int]] | None = None
    raw_branch_counts = payload.get("branch_counts")
    if isinstance(raw_branch_counts, dict):
        branch_counts = {}
        for key, entry in raw_branch_counts.items():
            if isinstance(entry, dict):
                taken = entry.get("taken", 0)
                not_taken = entry.get("not_taken", 0)
                if isinstance(taken, int) and isinstance(not_taken, int):
                    branch_counts[key] = {"taken": taken, "not_taken": not_taken}
    # Extract call-count PGO data (optional).
    call_counts: dict[str, int] | None = None
    raw_call_counts = payload.get("call_counts")
    if isinstance(raw_call_counts, dict):
        call_counts = {}
        for key, val in raw_call_counts.items():
            if isinstance(val, int):
                call_counts[key] = val
            elif isinstance(val, dict) and isinstance(val.get("calls"), int):
                call_counts[key] = val["calls"]
    # Extract loop iteration counts (optional).
    loop_counts: dict[str, dict[str, float | int]] | None = None
    raw_loop_counts = payload.get("loop_counts")
    if isinstance(raw_loop_counts, dict):
        loop_counts = {}
        for key, entry in raw_loop_counts.items():
            if isinstance(entry, dict):
                avg = entry.get("avg_iterations", 0.0)
                mx = entry.get("max_iterations", 0)
                if isinstance(avg, (int, float)) and isinstance(mx, int):
                    loop_counts[key] = {
                        "avg_iterations": float(avg),
                        "max_iterations": mx,
                    }
    digest = hashlib.sha256(raw).hexdigest()
    summary = PgoProfileSummary(
        version=version,
        hash=digest,
        hot_functions=hot_functions,
        branch_counts=branch_counts if branch_counts else None,
        call_counts=call_counts if call_counts else None,
        loop_counts=loop_counts if loop_counts else None,
    )
    return summary, path, None


def _load_runtime_feedback(
    project_root: Path,
    feedback_path: str,
    warnings: list[str],
    json_output: bool,
    command: str,
) -> tuple[RuntimeFeedbackSummary | None, Path | None, int | None]:
    path = Path(feedback_path).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    if not path.exists():
        return (
            None,
            None,
            _fail(
                f"Runtime feedback artifact not found: {path}",
                json_output,
                command=command,
            ),
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            _fail(
                f"Failed to read runtime feedback artifact {path}: {exc}",
                json_output,
                command=command,
            ),
        )
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return (
            None,
            None,
            _fail(
                "Invalid runtime feedback JSON at "
                f"{path}:{exc.lineno}:{exc.colno}: {exc.msg}",
                json_output,
                command=command,
            ),
        )
    if not isinstance(payload, dict):
        return (
            None,
            None,
            _fail(
                f"Invalid runtime feedback artifact {path}: expected a JSON object.",
                json_output,
                command=command,
            ),
        )
    errors: list[str] = []
    schema_version = payload.get("schema_version")
    if not isinstance(schema_version, int):
        errors.append("missing schema_version")
    if payload.get("kind") != "runtime_feedback":
        errors.append(f"unexpected kind {payload.get('kind')!r}")
    if not isinstance(payload.get("profile"), dict):
        errors.append("missing profile")
    if not isinstance(payload.get("hot_paths"), dict):
        errors.append("missing hot_paths")
    if not isinstance(payload.get("deopt_reasons"), dict):
        errors.append("missing deopt_reasons")
    if errors:
        return (
            None,
            None,
            _fail(
                f"Invalid runtime feedback artifact {path}: " + "; ".join(errors),
                json_output,
                command=command,
            ),
        )
    hot_functions = _extract_runtime_feedback_hot_functions(payload, warnings)
    digest = hashlib.sha256(raw).hexdigest()
    summary = RuntimeFeedbackSummary(
        schema_version=schema_version,
        hash=digest,
        hot_functions=hot_functions,
    )
    return summary, path, None
