#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
from dataclasses import dataclass
from datetime import UTC, datetime
import json
import os
import platform
from pathlib import Path
import subprocess
import sys
import threading
import time
from typing import Any, BinaryIO, Sequence, TextIO


REPO_ROOT = Path(__file__).resolve().parents[1]
LOG_ROOT = Path("logs/agents")
CODEX_STALL_ROOT = LOG_ROOT / "codex_stall"
CANONICAL_ARTIFACT_ROOTS = (
    Path("logs"),
    Path("tmp"),
    Path("bench/results"),
    Path("target"),
)
SCHEMA_VERSION = 1
ACTIVE_STATUSES = frozenset({"running", "paused", "blocked"})
BROAD_ROLE = "broad-sweep coordinator"
VALID_ROLES = (
    "implementer",
    "reducer",
    BROAD_ROLE,
    "perf custodian",
    "integrator",
)


@dataclass(frozen=True)
class ProofLaneRule:
    lane: str
    proof_role: str
    shared_target_root: str
    priority: str
    path_prefixes: tuple[str, ...]
    commands: tuple[str, ...]
    reason: str


@dataclass(frozen=True)
class CoordinationRecord:
    task: str
    path: Path
    payload: dict[str, Any]

    @property
    def status(self) -> str:
        return str(self.payload.get("status") or "")

    @property
    def proof_role(self) -> str:
        return str(self.payload.get("proof_role") or "")

    @property
    def planned_proof_lane(self) -> str:
        return str(self.payload.get("planned_proof_lane") or "")

    @property
    def shared_target_root(self) -> str:
        return str(self.payload.get("shared_target_root") or "")

    @property
    def active(self) -> bool:
        return self.status in ACTIVE_STATUSES

    @property
    def broad_coordinator(self) -> bool:
        return self.proof_role == BROAD_ROLE


@dataclass
class StreamTiming:
    name: str
    byte_count: int = 0
    chunk_count: int = 0
    first_output_offset_sec: float | None = None
    last_output_offset_sec: float | None = None
    max_idle_gap_sec: float = 0.0
    idle_spans: list[dict[str, float | str]] | None = None
    idle_spans_truncated: int = 0

    def __post_init__(self) -> None:
        if self.idle_spans is None:
            self.idle_spans = []

    def _record_span(
        self,
        *,
        kind: str,
        start_offset_sec: float,
        end_offset_sec: float,
        max_spans: int,
    ) -> None:
        duration = max(0.0, end_offset_sec - start_offset_sec)
        if len(self.idle_spans or ()) >= max_spans:
            self.idle_spans_truncated += 1
            return
        assert self.idle_spans is not None
        self.idle_spans.append(
            {
                "kind": kind,
                "start_offset_sec": round(start_offset_sec, 6),
                "end_offset_sec": round(end_offset_sec, 6),
                "duration_sec": round(duration, 6),
            }
        )

    def observe(
        self,
        *,
        offset_sec: float,
        byte_count: int,
        idle_threshold_sec: float,
        max_spans: int,
    ) -> None:
        if byte_count <= 0:
            return
        if self.first_output_offset_sec is None:
            idle_gap = offset_sec
            self.first_output_offset_sec = offset_sec
            if idle_gap >= idle_threshold_sec:
                self._record_span(
                    kind="first_output_gap",
                    start_offset_sec=0.0,
                    end_offset_sec=offset_sec,
                    max_spans=max_spans,
                )
        else:
            assert self.last_output_offset_sec is not None
            idle_gap = max(0.0, offset_sec - self.last_output_offset_sec)
            if idle_gap >= idle_threshold_sec:
                self._record_span(
                    kind="between_outputs",
                    start_offset_sec=self.last_output_offset_sec,
                    end_offset_sec=offset_sec,
                    max_spans=max_spans,
                )
        self.max_idle_gap_sec = max(self.max_idle_gap_sec, idle_gap)
        self.last_output_offset_sec = offset_sec
        self.byte_count += byte_count
        self.chunk_count += 1

    def finish(
        self,
        *,
        elapsed_sec: float,
        idle_threshold_sec: float,
        max_spans: int,
    ) -> dict[str, Any]:
        no_output = self.first_output_offset_sec is None
        if no_output:
            first_output_gap_sec = elapsed_sec
            self.max_idle_gap_sec = max(self.max_idle_gap_sec, elapsed_sec)
            if elapsed_sec >= idle_threshold_sec:
                self._record_span(
                    kind="no_output",
                    start_offset_sec=0.0,
                    end_offset_sec=elapsed_sec,
                    max_spans=max_spans,
                )
        else:
            first_output_gap_sec = self.first_output_offset_sec
            assert self.last_output_offset_sec is not None
            terminal_gap = max(0.0, elapsed_sec - self.last_output_offset_sec)
            self.max_idle_gap_sec = max(self.max_idle_gap_sec, terminal_gap)
            if terminal_gap >= idle_threshold_sec:
                self._record_span(
                    kind="terminal_idle",
                    start_offset_sec=self.last_output_offset_sec,
                    end_offset_sec=elapsed_sec,
                    max_spans=max_spans,
                )
        return {
            "name": self.name,
            "byte_count": self.byte_count,
            "chunk_count": self.chunk_count,
            "first_output_gap_sec": round(first_output_gap_sec, 6),
            "first_output_seen": not no_output,
            "last_output_offset_sec": (
                None
                if self.last_output_offset_sec is None
                else round(self.last_output_offset_sec, 6)
            ),
            "max_idle_gap_sec": round(self.max_idle_gap_sec, 6),
            "idle_spans": list(self.idle_spans or ()),
            "idle_spans_truncated": self.idle_spans_truncated,
        }


class CodexStallTelemetry:
    def __init__(
        self,
        *,
        idle_threshold_sec: float,
        max_spans: int,
        started_monotonic: float,
    ) -> None:
        self.idle_threshold_sec = idle_threshold_sec
        self.max_spans = max_spans
        self.started_monotonic = started_monotonic
        self._lock = threading.Lock()
        self._streams = {
            "combined": StreamTiming("combined"),
            "stdout": StreamTiming("stdout"),
            "stderr": StreamTiming("stderr"),
        }

    def observe(self, stream: str, byte_count: int) -> None:
        offset_sec = time.monotonic() - self.started_monotonic
        with self._lock:
            self._streams[stream].observe(
                offset_sec=offset_sec,
                byte_count=byte_count,
                idle_threshold_sec=self.idle_threshold_sec,
                max_spans=self.max_spans,
            )
            self._streams["combined"].observe(
                offset_sec=offset_sec,
                byte_count=byte_count,
                idle_threshold_sec=self.idle_threshold_sec,
                max_spans=self.max_spans,
            )

    def combined_idle_sec(self) -> tuple[float, bool]:
        elapsed = time.monotonic() - self.started_monotonic
        with self._lock:
            combined = self._streams["combined"]
            if combined.last_output_offset_sec is None:
                return (elapsed, True)
            return (max(0.0, elapsed - combined.last_output_offset_sec), False)

    def finish(self, elapsed_sec: float) -> dict[str, Any]:
        with self._lock:
            return {
                name: stream.finish(
                    elapsed_sec=elapsed_sec,
                    idle_threshold_sec=self.idle_threshold_sec,
                    max_spans=self.max_spans,
                )
                for name, stream in self._streams.items()
            }


PROOF_LANE_RULES = (
    ProofLaneRule(
        lane="agent_coordination",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=(
            "tools/agent_coordination.py",
            "tests/test_agent_coordination.py",
            "docs/ops/MULTI_AGENT_COORDINATION.md",
            "AGENTS.md",
        ),
        commands=(
            "uv run --python 3.12 python -m pytest -q tests/test_agent_coordination.py -p no:cacheprovider",
            "uv run --python 3.12 python tools/check_subprocess_guard_coverage.py",
        ),
        reason="coordination changes need focused protocol coverage plus subprocess-custody audit",
    ),
    ProofLaneRule(
        lane="subprocess_guard_coverage",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=("tools/check_subprocess_guard_coverage.py",),
        commands=(
            "uv run --python 3.12 python tools/check_subprocess_guard_coverage.py",
        ),
        reason="raw subprocess/signal policy changes must keep the static custody audit green",
    ),
    ProofLaneRule(
        lane="tir_type_refine",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=("runtime/molt-backend/src/tir/type_refine.rs",),
        commands=("cargo test -p molt-backend type_refine -- --nocapture",),
        reason="TIR type facts require direct solver regressions before broader differential proof",
    ),
    ProofLaneRule(
        lane="luau_backend",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=(
            "runtime/molt-backend/src/luau.rs",
            "tools/gen_luau_support_matrix.py",
            "tests/tools/test_gen_luau_support_matrix.py",
            "docs/spec/areas/compiler/luau_support_matrix.generated.md",
        ),
        commands=(
            "uv run --python 3.12 python -m pytest -q tests/tools/test_gen_luau_support_matrix.py -p no:cacheprovider",
            "cargo test -p molt-backend --features luau-backend test_compile_checked_lowers_call_async_poll_target_directly -- --nocapture",
        ),
        reason="Luau support claims need generated matrix coverage plus feature-enabled backend tests",
    ),
    ProofLaneRule(
        lane="molt_backend_targeted",
        proof_role="implementer",
        shared_target_root="target",
        priority="P2",
        path_prefixes=("runtime/molt-backend/src/",),
        commands=("cargo test -p molt-backend",),
        reason="backend code changes need at least package-level Rust validation after focused tests",
    ),
    ProofLaneRule(
        lane="frontend_targeted",
        proof_role="implementer",
        shared_target_root="target",
        priority="P2",
        path_prefixes=("src/molt/frontend/",),
        commands=(
            "uv run --python 3.12 python -m pytest -q tests/test_frontend_midend_passes.py -p no:cacheprovider",
        ),
        reason="frontend lowering changes should prove midend/frontend pass contracts",
    ),
    ProofLaneRule(
        lane="molt_gpu_targeted",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=("runtime/molt-gpu/src/", "runtime/molt-gpu/tests/"),
        commands=("cargo test -p molt-gpu",),
        reason="GPU primitive/runtime changes need focused crate-level Rust validation",
    ),
)


def utc_now() -> str:
    return datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def artifact_stamp() -> str:
    return datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")


def repo_relative(path: Path, repo_root: Path) -> str:
    try:
        return path.resolve().relative_to(repo_root.resolve()).as_posix()
    except ValueError:
        return str(path)


def read_git_identity(repo_root: Path) -> tuple[str, str]:
    git_dir = repo_root / ".git"
    head_path = git_dir / "HEAD"
    if not head_path.is_file():
        return ("unknown", "unknown")
    head = head_path.read_text(encoding="utf-8", errors="replace").strip()
    if head.startswith("ref: "):
        ref = head.removeprefix("ref: ").strip()
        commit_path = git_dir / ref
        packed_refs = git_dir / "packed-refs"
        commit = "unknown"
        if commit_path.is_file():
            commit = commit_path.read_text(encoding="utf-8", errors="replace").strip()
        elif packed_refs.is_file():
            for line in packed_refs.read_text(
                encoding="utf-8", errors="replace"
            ).splitlines():
                if not line or line.startswith("#") or line.startswith("^"):
                    continue
                sha, _, name = line.partition(" ")
                if name == ref:
                    commit = sha
                    break
        branch = ref.removeprefix("refs/heads/")
        return (branch, commit[:12] if commit != "unknown" else commit)
    return ("detached", head[:12] if head else "unknown")


def command_paths(name: str, environ: dict[str, str] | None = None) -> list[str]:
    env = environ if environ is not None else os.environ
    path_value = env.get("PATH", os.defpath)
    path_exts = [""]
    if os.name == "nt":
        configured_exts = env.get("PATHEXT", ".COM;.EXE;.BAT;.CMD")
        path_exts = [ext.lower() for ext in configured_exts.split(";") if ext]
        if Path(name).suffix:
            path_exts = [""]

    seen: set[str] = set()
    found: list[str] = []
    for directory in path_value.split(os.pathsep):
        if not directory:
            continue
        base = Path(directory)
        for ext in path_exts:
            candidate = base / f"{name}{ext}"
            if not candidate.is_file():
                continue
            if os.name != "nt" and not os.access(candidate, os.X_OK):
                continue
            key = str(candidate).lower() if os.name == "nt" else str(candidate)
            if key in seen:
                continue
            seen.add(key)
            found.append(str(candidate))
    return found


def command_path(name: str, environ: dict[str, str] | None = None) -> str | None:
    paths = command_paths(name, environ)
    return paths[0] if paths else None


def is_windows_app_execution_alias(path: str | None) -> bool:
    if not path:
        return False
    normalized = path.replace("/", "\\").lower()
    return "\\microsoft\\windowsapps\\" in normalized


def is_wsl_bash_shim(path: str | None) -> bool:
    if not path:
        return False
    normalized = path.replace("/", "\\").lower()
    return normalized.endswith("\\system32\\bash.exe") or normalized.endswith(
        "\\windowsapps\\bash.exe"
    )


def usable_command(path: str | None) -> bool:
    return bool(path) and not is_windows_app_execution_alias(path)


def bash_candidates(environ: dict[str, str] | None = None) -> list[str]:
    candidates = command_paths("bash", environ)
    if os.name == "nt":
        for path in (
            Path("C:/Program Files/Git/bin/bash.exe"),
            Path("C:/Program Files/Git/usr/bin/bash.exe"),
            Path("C:/Program Files (x86)/Git/bin/bash.exe"),
            Path("C:/Program Files (x86)/Git/usr/bin/bash.exe"),
        ):
            if path.is_file() and str(path) not in candidates:
                candidates.append(str(path))
    return candidates


def choose_bash(environ: dict[str, str] | None = None) -> str | None:
    for candidate in bash_candidates(environ):
        if not is_wsl_bash_shim(candidate):
            return candidate
    return None


def detect_python_command(environ: dict[str, str] | None = None) -> str:
    env = environ if environ is not None else os.environ
    explicit = env.get("PYTHON")
    if explicit:
        return explicit
    for candidate in ("python", "python3", "py"):
        if usable_command(command_path(candidate, env)):
            return candidate
    return sys.executable


def environment_snapshot(
    repo_root: Path,
    *,
    environ: dict[str, str] | None = None,
) -> dict[str, Any]:
    env = environ if environ is not None else os.environ
    release = platform.release()
    is_wsl = bool(env.get("WSL_DISTRO_NAME")) or "microsoft" in release.lower()
    python_path = command_path("python", env)
    python3_path = command_path("python3", env)
    py_path = command_path("py", env)
    bash_path = command_path("bash", env)
    usable_bash = choose_bash(env)
    return {
        "os_name": os.name,
        "sys_platform": sys.platform,
        "platform_system": platform.system(),
        "platform_release": release,
        "platform_machine": platform.machine(),
        "is_windows": os.name == "nt",
        "is_macos": sys.platform == "darwin",
        "is_linux": sys.platform.startswith("linux"),
        "is_wsl": is_wsl,
        "python_executable": sys.executable,
        "python_version": platform.python_version(),
        "recommended_python_command": detect_python_command(env),
        "uv": command_path("uv", env),
        "python": python_path,
        "python_usable": usable_command(python_path),
        "python3": python3_path,
        "python3_usable": usable_command(python3_path),
        "py": py_path,
        "py_usable": usable_command(py_path),
        "bash": bash_path,
        "bash_candidates": bash_candidates(env),
        "usable_bash": usable_bash,
        "posix_shell_available": usable_bash is not None,
        "shell": env.get("SHELL") or env.get("ComSpec") or "",
        "codex_shell": env.get("CODEX_SHELL", ""),
        "ci": env.get("CI", ""),
        "repo_root": str(repo_root),
    }


def normalize_repo_path(path: str | Path, repo_root: Path) -> str:
    candidate = Path(path)
    if candidate.is_absolute():
        try:
            return candidate.resolve().relative_to(repo_root.resolve()).as_posix()
        except ValueError:
            return candidate.as_posix()
    normalized = candidate.as_posix()
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return normalized


def git_status_paths(repo_root: Path) -> list[str]:
    try:
        proc = subprocess.run(
            ["git", "status", "--porcelain=v1", "-z"],
            cwd=str(repo_root),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return []
    if proc.returncode != 0:
        return []

    entries = proc.stdout.split(b"\0")
    paths: list[str] = []
    i = 0
    while i < len(entries):
        entry = entries[i]
        i += 1
        if not entry:
            continue
        text = entry.decode("utf-8", errors="surrogateescape")
        if len(text) < 4:
            continue
        status = text[:2]
        path = text[3:]
        if status.startswith("R") or status.endswith("R"):
            if i < len(entries) and entries[i]:
                path = entries[i].decode("utf-8", errors="surrogateescape")
                i += 1
        paths.append(normalize_repo_path(path, repo_root))
    return sorted(dict.fromkeys(paths))


def rule_matches_path(rule: ProofLaneRule, path: str) -> bool:
    for prefix in rule.path_prefixes:
        if prefix.endswith("/"):
            if path.startswith(prefix):
                return True
        elif path == prefix:
            return True
    return False


def differential_test_paths(paths: Sequence[str]) -> list[str]:
    return sorted(
        {
            path
            for path in paths
            if path.startswith("tests/differential/") and path.endswith(".py")
        }
    )


def proof_recommendations(
    paths: Sequence[str],
    repo_root: Path,
) -> list[dict[str, Any]]:
    normalized = [normalize_repo_path(path, repo_root) for path in paths]
    recommendations: list[dict[str, Any]] = []
    diff_paths = differential_test_paths(normalized)
    if diff_paths:
        recommendations.append(
            {
                "lane": "focused_differential",
                "proof_role": "reducer",
                "shared_target_root": "target",
                "priority": "P0",
                "reason": "changed differential fixtures should be run directly before broad sweeps",
                "covered_paths": diff_paths,
                "commands": [
                    "uv run --python 3.12 python tests/molt_diff.py "
                    + " ".join(diff_paths)
                ],
            }
        )

    for rule in PROOF_LANE_RULES:
        covered = sorted({path for path in normalized if rule_matches_path(rule, path)})
        if not covered:
            continue
        recommendations.append(
            {
                "lane": rule.lane,
                "proof_role": rule.proof_role,
                "shared_target_root": rule.shared_target_root,
                "priority": rule.priority,
                "reason": rule.reason,
                "covered_paths": covered,
                "commands": list(rule.commands),
            }
        )
    return recommendations


def proof_plan_payload(args: argparse.Namespace) -> dict[str, Any]:
    repo_root = args.repo_root.resolve()
    paths = [normalize_repo_path(path, repo_root) for path in args.paths]
    source = "explicit"
    if not paths:
        paths = git_status_paths(repo_root)
        source = "git-status"
    recommendations = proof_recommendations(paths, repo_root)
    return {
        "schema_version": SCHEMA_VERSION,
        "repo_root": str(repo_root),
        "source": source,
        "input_paths": paths,
        "recommendations": recommendations,
        "coordination": {
            "before_long_lane": "uv run --python 3.12 python tools/agent_coordination.py check",
            "init_template": "uv run --python 3.12 python tools/agent_coordination.py init <task> --role <role> --lane <lane>",
        },
    }


def print_text_proof_plan(payload: dict[str, Any]) -> None:
    print(
        "proof plan: {count} recommendation(s) from {source} path source".format(
            count=len(payload["recommendations"]),
            source=payload["source"],
        )
    )
    if not payload["input_paths"]:
        print("- no changed or explicit paths; no focused proof lane recommended")
        return
    print("paths:")
    for path in payload["input_paths"]:
        print(f"- {path}")
    for item in payload["recommendations"]:
        print(
            "\n[{priority}] {lane}: role={role} target={target}".format(
                priority=item["priority"],
                lane=item["lane"],
                role=item["proof_role"],
                target=item["shared_target_root"],
            )
        )
        print(f"reason: {item['reason']}")
        print("covered:")
        for path in item["covered_paths"]:
            print(f"- {path}")
        print("commands:")
        for command in item["commands"]:
            print(f"- {command}")


def validate_task_name(task: str) -> str:
    normalized = task.strip().replace("\\", "/").strip("/")
    if not normalized or normalized in {".", ".."}:
        raise ValueError("task name must not be empty")
    parts = normalized.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        raise ValueError(f"task name must stay under logs/agents: {task!r}")
    return normalized


def task_dir(repo_root: Path, task: str) -> Path:
    return repo_root / LOG_ROOT / validate_task_name(task)


def build_record(
    *,
    repo_root: Path,
    task: str,
    report_path: Path,
    role: str,
    lane: str,
    status: str,
    target_root: str,
    owned_paths: Sequence[str],
    agent: str | None,
    session: str | None,
    created_at: str,
) -> dict[str, Any]:
    if role not in VALID_ROLES:
        raise ValueError(f"unknown proof role: {role}")
    branch, commit = read_git_identity(repo_root)
    session_id = session or os.environ.get("MOLT_SESSION_ID") or task
    agent_id = agent or os.environ.get("MOLT_AGENT_ID") or session_id
    base = task_dir(repo_root, task)
    return {
        "schema_version": SCHEMA_VERSION,
        "task": task,
        "created_at_utc": created_at,
        "updated_at_utc": created_at,
        "agent": agent_id,
        "session_id": session_id,
        "repo_root": str(repo_root),
        "branch": branch,
        "commit": commit,
        "status": status,
        "proof_role": role,
        "planned_proof_lane": lane,
        "shared_target_root": target_root,
        "owned_paths": list(owned_paths),
        "artifact_roots": ["target/", "tmp/", "logs/", "bench/results/"],
        "environment": environment_snapshot(repo_root),
        "report_path": repo_relative(report_path, repo_root),
        "progress_log": repo_relative(base / "progress.log", repo_root),
        "artifacts_dir": repo_relative(base / "artifacts", repo_root),
    }


def render_report(record: dict[str, Any]) -> str:
    owned_paths = record["owned_paths"] or ["TBD"]
    owned_lines = "\n".join(f"  - {path}" for path in owned_paths)
    artifact_lines = "\n".join(
        f"  - {path}" for path in record.get("artifact_roots", ())
    )
    environment = record.get("environment", {})
    return f"""# Agent Progress Report

## Meta
- Task: {record["task"]}
- Agent: {record["agent"]}
- Repo: {record["repo_root"]}
- Branch/Commit: {record["branch"]} / {record["commit"]}
- Session: {record["session_id"]}
- Status: {record["status"]}

## Coordination
- Protocol: docs/ops/MULTI_AGENT_COORDINATION.md
- Coordination JSON: logs/agents/{record["task"]}/coordination.json
- Proof role: {record["proof_role"]}
- Planned proof lane: {record["planned_proof_lane"] or "TBD"}
- Shared target root: {record["shared_target_root"]}
- Broad lane ownership checked: TBD
- Owned files/directories:
{owned_lines}
- Canonical artifact roots:
{artifact_lines}

## Environment
- Platform: {environment.get("platform_system", "unknown")} {environment.get("platform_release", "")} {environment.get("platform_machine", "")}
- Python executable: {environment.get("python_executable", "unknown")}
- Recommended Python command: {environment.get("recommended_python_command", "python")}
- uv: {environment.get("uv") or "not found"}
- POSIX shell: {environment.get("usable_bash") or "not found"}

## Summary
- Initialized task directory.

## Outputs
- Artifacts:
  - {record["artifacts_dir"]}
- Logs:
  - {record["progress_log"]}

## Next Steps
1. Read docs/ops/MULTI_AGENT_COORDINATION.md.
2. Fill coordination fields.
3. Write plan and first falsifying command.
4. Start implementation or proof lane.

## Resume Instructions
- Export MOLT_SESSION_ID="{record["session_id"]}"
- Resume from the next command recorded above.
"""


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(tmp, path)


def resolve_canonical_artifact_path(repo_root: Path, path: Path) -> Path:
    repo_root = repo_root.resolve()
    resolved = path if path.is_absolute() else repo_root / path
    resolved = resolved.resolve()
    try:
        resolved.relative_to(repo_root)
    except ValueError as exc:
        raise ValueError(f"artifact path must stay under repo root: {path}") from exc

    roots = tuple((repo_root / root).resolve() for root in CANONICAL_ARTIFACT_ROOTS)
    if not any(resolved == root or root in resolved.parents for root in roots):
        allowed = ", ".join(root.as_posix() for root in CANONICAL_ARTIFACT_ROOTS)
        raise ValueError(f"artifact path must stay under canonical roots: {allowed}")
    return resolved


def default_codex_stall_report_path(repo_root: Path) -> Path:
    return repo_root / CODEX_STALL_ROOT / f"stall_{artifact_stamp()}_{os.getpid()}.json"


def command_descriptor(
    command: Sequence[str], *, record_command: bool
) -> dict[str, Any]:
    joined = "\0".join(command).encode("utf-8", errors="surrogateescape")
    descriptor: dict[str, Any] = {
        "argv_count": len(command),
        "argv_sha256": hashlib.sha256(joined).hexdigest(),
        "executable_name": Path(command[0]).name if command else "",
        "argv_recorded": record_command,
    }
    if record_command:
        descriptor["argv"] = list(command)
    return descriptor


def codex_stall_launch_command(
    args: argparse.Namespace, command: Sequence[str]
) -> list[str]:
    if args.no_memory_guard:
        return list(command)
    memory_guard = args.repo_root.resolve() / "tools" / "memory_guard.py"
    wrapped = [sys.executable, str(memory_guard)]
    if args.memory_guard_max_rss_gb is not None:
        wrapped.extend(["--max-rss-gb", str(args.memory_guard_max_rss_gb)])
    if args.memory_guard_max_total_rss_gb is not None:
        wrapped.extend(["--max-total-rss-gb", str(args.memory_guard_max_total_rss_gb)])
    if args.memory_guard_child_rlimit_gb is not None:
        wrapped.extend(["--child-rlimit-gb", str(args.memory_guard_child_rlimit_gb)])
    if args.memory_guard_timeout_sec is not None:
        wrapped.extend(["--timeout", str(args.memory_guard_timeout_sec)])
    wrapped.extend(["--", *command])
    return wrapped


def _write_stream_chunk(target: TextIO, chunk: bytes) -> None:
    buffer = getattr(target, "buffer", None)
    try:
        if buffer is not None:
            buffer.write(chunk)
            buffer.flush()
        else:
            target.write(chunk.decode("utf-8", errors="replace"))
            target.flush()
    except BrokenPipeError:
        return


def _pipe_reader(
    pipe: BinaryIO,
    *,
    stream_name: str,
    target: TextIO,
    telemetry: CodexStallTelemetry,
) -> None:
    try:
        while True:
            chunk = pipe.read(8192)
            if not chunk:
                break
            telemetry.observe(stream_name, len(chunk))
            _write_stream_chunk(target, chunk)
    finally:
        pipe.close()


def run_codex_stall_diagnostic(args: argparse.Namespace) -> int:
    repo_root = args.repo_root.resolve()
    command = list(args.child_command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("agent_coordination: codex-stall command is required", file=sys.stderr)
        return 2
    if args.idle_threshold_sec <= 0:
        print("agent_coordination: --idle-threshold-sec must be > 0", file=sys.stderr)
        return 2
    if args.poll_sec <= 0:
        print("agent_coordination: --poll-sec must be > 0", file=sys.stderr)
        return 2
    if args.max_spans < 0:
        print("agent_coordination: --max-spans must be >= 0", file=sys.stderr)
        return 2
    try:
        report_path = resolve_canonical_artifact_path(
            repo_root,
            args.out or default_codex_stall_report_path(repo_root),
        )
    except ValueError as exc:
        print(f"agent_coordination: {exc}", file=sys.stderr)
        return 2

    launched_command = codex_stall_launch_command(args, command)
    started_at = utc_now()
    started_monotonic = time.monotonic()
    telemetry = CodexStallTelemetry(
        idle_threshold_sec=args.idle_threshold_sec,
        max_spans=args.max_spans,
        started_monotonic=started_monotonic,
    )
    base_payload: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "kind": "codex_stall_diagnostic",
        "status": "running",
        "started_at_utc": started_at,
        "completed_at_utc": None,
        "repo_root": str(repo_root),
        "report_path": repo_relative(report_path, repo_root),
        "privacy": {
            "records_child_output_text": False,
            "records_codex_state": False,
            "records_argv_by_default": False,
            "recorded_fields": [
                "timing",
                "byte_counts",
                "chunk_counts",
                "return_code",
                "command_hash",
            ],
        },
        "diagnostic": {
            "idle_threshold_sec": args.idle_threshold_sec,
            "poll_sec": args.poll_sec,
            "max_spans_per_stream": args.max_spans,
            "live_notices": not args.no_live_notices,
        },
        "memory_guard": {
            "enabled": not args.no_memory_guard,
            "wrapper": "tools/memory_guard.py" if not args.no_memory_guard else None,
            "timeout_sec": args.memory_guard_timeout_sec,
            "max_rss_gb": args.memory_guard_max_rss_gb,
            "max_total_rss_gb": args.memory_guard_max_total_rss_gb,
            "child_rlimit_gb": args.memory_guard_child_rlimit_gb,
        },
        "command": command_descriptor(command, record_command=args.record_command),
        "launched_command": command_descriptor(
            launched_command,
            record_command=False,
        ),
        "environment": environment_snapshot(repo_root),
        "streams": {},
    }
    write_json(report_path, base_payload)

    print(
        "codex-stall: timing child output; report={path}; privacy=no child output text".format(
            path=repo_relative(report_path, repo_root)
        ),
        file=sys.stderr,
    )
    proc: subprocess.Popen[bytes] | None = None
    interrupted = False
    try:
        proc = subprocess.Popen(
            launched_command,
            cwd=str(repo_root),
            stdin=None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )
        assert proc.stdout is not None
        assert proc.stderr is not None
        readers = [
            threading.Thread(
                target=_pipe_reader,
                kwargs={
                    "pipe": proc.stdout,
                    "stream_name": "stdout",
                    "target": sys.stdout,
                    "telemetry": telemetry,
                },
                daemon=True,
            ),
            threading.Thread(
                target=_pipe_reader,
                kwargs={
                    "pipe": proc.stderr,
                    "stream_name": "stderr",
                    "target": sys.stderr,
                    "telemetry": telemetry,
                },
                daemon=True,
            ),
        ]
        for reader in readers:
            reader.start()

        next_notice_sec = args.idle_threshold_sec
        while proc.poll() is None:
            time.sleep(args.poll_sec)
            if args.no_live_notices:
                continue
            idle_sec, awaiting_first = telemetry.combined_idle_sec()
            elapsed_sec = time.monotonic() - started_monotonic
            if idle_sec >= next_notice_sec:
                phase = "awaiting first child output" if awaiting_first else "idle"
                print(
                    "codex-stall: {phase} for {idle:.1f}s (elapsed {elapsed:.1f}s)".format(
                        phase=phase,
                        idle=idle_sec,
                        elapsed=elapsed_sec,
                    ),
                    file=sys.stderr,
                )
                next_notice_sec = idle_sec + args.idle_threshold_sec
            elif idle_sec < args.idle_threshold_sec:
                next_notice_sec = args.idle_threshold_sec

        return_code = proc.wait()
        for reader in readers:
            reader.join(timeout=5.0)
    except KeyboardInterrupt:
        interrupted = True
        return_code = 130
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                return_code = proc.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                proc.kill()
                return_code = proc.wait()
    except OSError as exc:
        completed_at = utc_now()
        elapsed_sec = time.monotonic() - started_monotonic
        payload = base_payload | {
            "status": "spawn_failed",
            "completed_at_utc": completed_at,
            "elapsed_sec": round(elapsed_sec, 6),
            "error": str(exc),
            "streams": telemetry.finish(elapsed_sec),
        }
        write_json(report_path, payload)
        print(f"agent_coordination: codex-stall spawn failed: {exc}", file=sys.stderr)
        return 2

    completed_at = utc_now()
    elapsed_sec = time.monotonic() - started_monotonic
    streams = telemetry.finish(elapsed_sec)
    status = "interrupted" if interrupted else "completed"
    payload = base_payload | {
        "status": status,
        "completed_at_utc": completed_at,
        "elapsed_sec": round(elapsed_sec, 6),
        "return_code": return_code,
        "streams": streams,
    }
    write_json(report_path, payload)
    combined = streams["combined"]
    print(
        "codex-stall: rc={rc} elapsed={elapsed:.1f}s first_output_gap={first:.1f}s "
        "max_idle={idle:.1f}s report={path}".format(
            rc=return_code,
            elapsed=elapsed_sec,
            first=combined["first_output_gap_sec"],
            idle=combined["max_idle_gap_sec"],
            path=repo_relative(report_path, repo_root),
        ),
        file=sys.stderr,
    )
    return int(return_code)


def init_task(args: argparse.Namespace) -> dict[str, Any]:
    repo_root = args.repo_root.resolve()
    task = validate_task_name(args.task)
    base = task_dir(repo_root, task)
    artifacts = base / "artifacts"
    artifacts.mkdir(parents=True, exist_ok=True)
    (base / "progress.log").touch()

    created_at = utc_now()
    stamp = created_at.replace("-", "").replace(":", "").removesuffix("Z")
    report_path = base / f"report_{stamp}.md"
    record = build_record(
        repo_root=repo_root,
        task=task,
        report_path=report_path,
        role=args.role,
        lane=args.lane,
        status=args.status,
        target_root=args.target_root,
        owned_paths=args.owned,
        agent=args.agent,
        session=args.session,
        created_at=created_at,
    )
    report_path.write_text(render_report(record), encoding="utf-8")
    write_json(base / "coordination.json", record)
    return record


def load_records(repo_root: Path) -> list[CoordinationRecord]:
    root = repo_root / LOG_ROOT
    if not root.is_dir():
        return []
    records: list[CoordinationRecord] = []
    for path in sorted(root.glob("**/coordination.json")):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            payload = {
                "schema_version": SCHEMA_VERSION,
                "task": path.parent.name,
                "status": "invalid",
                "error": str(exc),
            }
        records.append(
            CoordinationRecord(
                task=str(payload.get("task") or path.parent.name),
                path=path,
                payload=payload,
            )
        )
    return records


def broad_lane_collisions(
    records: Sequence[CoordinationRecord],
    repo_root: Path,
) -> list[dict[str, Any]]:
    groups: dict[tuple[str, str], list[CoordinationRecord]] = {}
    for record in records:
        if not (record.active and record.broad_coordinator):
            continue
        key = (record.shared_target_root, record.planned_proof_lane)
        groups.setdefault(key, []).append(record)

    collisions: list[dict[str, Any]] = []
    for (target_root, lane), group in sorted(groups.items()):
        if len(group) < 2:
            continue
        collisions.append(
            {
                "kind": "broad_lane_collision",
                "shared_target_root": target_root,
                "planned_proof_lane": lane,
                "tasks": [record.task for record in group],
                "paths": [repo_relative(record.path, repo_root) for record in group],
            }
        )
    return collisions


def summary_payload(repo_root: Path) -> dict[str, Any]:
    records = load_records(repo_root)
    return {
        "schema_version": SCHEMA_VERSION,
        "repo_root": str(repo_root),
        "records": [
            record.payload | {"coordination_path": str(record.path)}
            for record in records
        ],
        "collisions": broad_lane_collisions(records, repo_root),
    }


def print_text_summary(payload: dict[str, Any]) -> None:
    records = payload["records"]
    collisions = payload["collisions"]
    print(f"agent coordination: {len(records)} task record(s)")
    for record in records:
        print(
            "- {task}: status={status} role={role} lane={lane} target={target}".format(
                task=record.get("task", "unknown"),
                status=record.get("status", "unknown"),
                role=record.get("proof_role", "unknown"),
                lane=record.get("planned_proof_lane") or "TBD",
                target=record.get("shared_target_root") or "TBD",
            )
        )
    if collisions:
        print("collisions:")
        for collision in collisions:
            print(
                "- {kind}: target={target} lane={lane} tasks={tasks}".format(
                    kind=collision["kind"],
                    target=collision["shared_target_root"],
                    lane=collision["planned_proof_lane"],
                    tasks=", ".join(collision["tasks"]),
                )
            )


def print_text_environment(payload: dict[str, Any]) -> None:
    print(
        "environment: {system} {release} {machine}".format(
            system=payload["platform_system"],
            release=payload["platform_release"],
            machine=payload["platform_machine"],
        )
    )
    print(f"- sys.platform={payload['sys_platform']} os.name={payload['os_name']}")
    print(f"- python={payload['python_executable']}")
    print(f"- recommended_python_command={payload['recommended_python_command']}")
    print(f"- uv={payload['uv'] or 'not found'}")
    print(f"- posix_shell={payload['usable_bash'] or 'not found'}")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Create and inspect Molt multi-agent coordination records."
    )
    parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    sub = parser.add_subparsers(dest="command", required=True)

    init = sub.add_parser("init", help="create a task log and coordination.json")
    init.add_argument("task")
    init.add_argument("--agent")
    init.add_argument("--session")
    init.add_argument("--role", choices=VALID_ROLES, default="implementer")
    init.add_argument("--lane", default="")
    init.add_argument("--status", default="running")
    init.add_argument("--target-root", default="target")
    init.add_argument("--owned", action="append", default=[])
    init.add_argument("--json", action="store_true")

    scan = sub.add_parser("scan", help="list active coordination records")
    scan.add_argument("--json", action="store_true")

    check = sub.add_parser("check", help="fail on broad-lane coordination collisions")
    check.add_argument("--json", action="store_true")

    proof_plan = sub.add_parser(
        "proof-plan",
        help="recommend focused proof lanes for explicit paths or current git changes",
    )
    proof_plan.add_argument(
        "paths",
        nargs="*",
        help="repo-relative paths; defaults to current git status when omitted",
    )
    proof_plan.add_argument("--json", action="store_true")

    env = sub.add_parser("env", help="print local agent environment facts")
    env.add_argument("--json", action="store_true")

    stall = sub.add_parser(
        "codex-stall",
        help=(
            "run a command and write privacy-preserving first-output/idle timing "
            "diagnostics under canonical artifact roots"
        ),
    )
    stall.add_argument(
        "--out",
        type=Path,
        help=(
            "JSON report path; must stay under logs/, tmp/, bench/results/, or "
            "target/ (default: logs/agents/codex_stall/stall_<timestamp>.json)"
        ),
    )
    stall.add_argument(
        "--idle-threshold-sec",
        type=float,
        default=30.0,
        help="minimum silent span recorded as an idle gap (default: 30)",
    )
    stall.add_argument(
        "--poll-sec",
        type=float,
        default=1.0,
        help="live-notice polling interval while the child is running (default: 1)",
    )
    stall.add_argument(
        "--max-spans",
        type=int,
        default=200,
        help="maximum idle spans retained per stream before truncation (default: 200)",
    )
    stall.add_argument(
        "--record-command",
        action="store_true",
        help="include the raw child argv in the report; default stores only a hash",
    )
    stall.add_argument(
        "--no-live-notices",
        action="store_true",
        help="suppress stderr notices while combined child output stays idle",
    )
    stall.add_argument(
        "--no-memory-guard",
        action="store_true",
        help=(
            "launch the command directly instead of through tools/memory_guard.py; "
            "use only for non-proof probes or an already guarded direct child"
        ),
    )
    stall.add_argument("--memory-guard-timeout-sec", type=float)
    stall.add_argument("--memory-guard-max-rss-gb", type=float)
    stall.add_argument("--memory-guard-max-total-rss-gb", type=float)
    stall.add_argument("--memory-guard-child-rlimit-gb", type=float)
    stall.add_argument("child_command", nargs=argparse.REMAINDER)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = args.repo_root.resolve()
    if args.command == "init":
        record = init_task(args)
        if args.json:
            print(json.dumps(record, indent=2, sort_keys=True))
        else:
            print(f"Created task scaffold at {LOG_ROOT / record['task']}")
            print("Read docs/ops/MULTI_AGENT_COORDINATION.md before long proof lanes.")
        return 0

    if args.command == "env":
        payload = environment_snapshot(repo_root)
        if args.json:
            print(json.dumps(payload, indent=2, sort_keys=True))
        else:
            print_text_environment(payload)
        return 0

    if args.command == "codex-stall":
        return run_codex_stall_diagnostic(args)

    if args.command == "proof-plan":
        payload = proof_plan_payload(args)
        if args.json:
            print(json.dumps(payload, indent=2, sort_keys=True))
        else:
            print_text_proof_plan(payload)
        return 0

    payload = summary_payload(repo_root)
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print_text_summary(payload)
    if args.command == "check" and payload["collisions"]:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
