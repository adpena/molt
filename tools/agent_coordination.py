#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
from dataclasses import dataclass
from datetime import UTC, datetime
import json
import os
import platform
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
from typing import Any, BinaryIO, Sequence, TextIO


REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt import process_guard  # noqa: E402
from molt.dx import DX_ENV_KEYS, RunContext, render_env  # noqa: E402
from tools import check_instruction_hierarchy, claims_status  # noqa: E402

LOG_ROOT = Path("logs/agents")
CODEX_STALL_ROOT = LOG_ROOT / "codex_stall"
CODEX_CRASH_ROOT = LOG_ROOT / "codex_crash"
CANONICAL_ARTIFACT_ROOTS = (
    Path("logs"),
    Path("tmp"),
    Path("bench/results"),
    Path("target"),
)
SCHEMA_VERSION = 1
AGENT_CONTEXT_SCHEMA = "molt.agent-context.v1"
AGENT_CONTEXT_COMMAND_TIMEOUT_SEC = 30.0
AGENT_CONTEXT_DOCUMENTS = (
    ("agent contract", "AGENTS.md"),
    ("agent contract adapter", "CLAUDE.md"),
    ("live orchestration", "docs/agent/ORCHESTRATION.md"),
    ("claims ledger", "docs/agent/CLAIMS.md"),
    ("proof custody", "docs/agent/PROOF_QUEUE.md"),
    ("coordination protocol", "docs/ops/MULTI_AGENT_COORDINATION.md"),
    ("canonical documentation map", "docs/CANONICALS.md"),
    ("documentation index", "docs/INDEX.md"),
    ("specification index", "docs/spec/README.md"),
)
CODEX_WINDOWS_CONTROL_C_EXIT = 3221225786
CODEX_DEFAULT_PROMPT_LIMIT = 3
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


@dataclass(frozen=True)
class ContextCommandResult:
    source: str
    command: tuple[str, ...]
    return_code: int | None
    stdout: str = ""
    stderr: str = ""
    failure_kind: str | None = None
    failure_message: str | None = None

    @property
    def ok(self) -> bool:
        return self.failure_kind is None and self.return_code == 0

    def error_payload(self) -> dict[str, Any]:
        message = self.failure_message or self.stderr.strip() or "command failed"
        return _context_error(
            self.source,
            self.failure_kind or "nonzero_exit",
            message,
            command=self.command,
            return_code=self.return_code,
        )


@dataclass(frozen=True)
class AgentContext:
    generated_at_utc: str
    live_facts: dict[str, Any]
    file_records: dict[str, Any]
    documentation: dict[str, Any]
    errors: tuple[dict[str, Any], ...]

    @property
    def ok(self) -> bool:
        return not self.errors

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema": AGENT_CONTEXT_SCHEMA,
            "generated_at_utc": self.generated_at_utc,
            "ok": self.ok,
            "live_facts": self.live_facts,
            "file_records": self.file_records,
            "documentation": self.documentation,
            "errors": list(self.errors),
        }


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
        path_prefixes=("runtime/molt-passes/src/tir/type_refine.rs",),
        commands=("cargo test -p molt-backend type_refine -- --nocapture",),
        reason="TIR type facts require direct solver regressions before broader differential proof",
    ),
    ProofLaneRule(
        lane="luau_backend",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=(
            "runtime/molt-backend-luau/src/luau.rs",
            "runtime/molt-backend-luau/src/luau/",
            "runtime/molt-backend-luau/src/luau_backend/",
            "tools/gen_luau_support_matrix.py",
            "tests/tools/test_gen_luau_support_matrix.py",
            "docs/spec/areas/compiler/luau_support_matrix.generated.md",
        ),
        commands=(
            "uv run --python 3.12 python -m pytest -q tests/tools/test_gen_luau_support_matrix.py -p no:cacheprovider",
            "cargo test -p molt-backend-luau --features luau-backend test_compile_checked_lowers_call_async_poll_target_directly -- --nocapture",
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
        reason="GPU compute/render primitive changes need focused crate-level Rust validation",
    ),
    ProofLaneRule(
        lane="molt_gpu_runtime_targeted",
        proof_role="implementer",
        shared_target_root="target",
        priority="P1",
        path_prefixes=("runtime/molt-gpu-runtime/src/",),
        commands=("cargo test -p molt-gpu-runtime",),
        reason="GPU object-runtime integration changes need focused crate-level Rust validation",
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


def _bounded_context_text(value: object, limit: int = 500) -> str:
    text = " ".join(str(value).split())
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def _context_error(
    source: str,
    kind: str,
    message: object,
    *,
    command: Sequence[str] = (),
    return_code: int | None = None,
) -> dict[str, Any]:
    return {
        "source": source,
        "kind": kind,
        "command": [str(item) for item in command],
        "return_code": return_code,
        "message": _bounded_context_text(message),
    }


def _run_context_command(
    source: str,
    command: Sequence[str],
    *,
    cwd: Path,
    timeout: float = AGENT_CONTEXT_COMMAND_TIMEOUT_SEC,
) -> ContextCommandResult:
    argv = tuple(str(item) for item in command)
    try:
        completed = process_guard.run_completed_command(
            argv,
            cwd=str(cwd),
            capture_output=True,
            check=False,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            memory_guard_prefix=None,
        )
    except subprocess.TimeoutExpired as exc:
        return ContextCommandResult(
            source=source,
            command=argv,
            return_code=None,
            stdout=str(exc.stdout or ""),
            stderr=str(exc.stderr or ""),
            failure_kind="timeout",
            failure_message=f"command exceeded {timeout:g}s timeout",
        )
    except OSError as exc:
        return ContextCommandResult(
            source=source,
            command=argv,
            return_code=None,
            failure_kind="spawn_error",
            failure_message=str(exc),
        )
    return ContextCommandResult(
        source=source,
        command=argv,
        return_code=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )


def _required_command_text(
    result: ContextCommandResult,
    errors: list[dict[str, Any]],
) -> str | None:
    if not result.ok:
        errors.append(result.error_payload())
        return None
    return result.stdout.strip()


def _parse_worktree_porcelain(text: str) -> list[dict[str, Any]]:
    worktrees: list[dict[str, Any]] = []
    for block in re.split(r"\r?\n\r?\n", text.strip()):
        if not block.strip():
            continue
        record: dict[str, Any] = {
            "path": "",
            "head": "unknown",
            "branch": "detached",
            "detached": False,
            "locked": False,
            "prunable": False,
        }
        for line in block.splitlines():
            key, _, value = line.partition(" ")
            if key == "worktree":
                record["path"] = value
            elif key == "HEAD":
                record["head"] = value
            elif key == "branch":
                record["branch"] = value.removeprefix("refs/heads/")
            elif key == "detached":
                record["detached"] = True
            elif key == "locked":
                record["locked"] = True
                if value:
                    record["locked_reason"] = value
            elif key == "prunable":
                record["prunable"] = True
                if value:
                    record["prunable_reason"] = value
        if record["path"]:
            worktrees.append(record)
    return worktrees


def _porcelain_status_entry_count(text: str) -> int:
    entries = text.split("\0")
    count = 0
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if not entry:
            continue
        count += 1
        status = entry[:2]
        if (
            status.startswith(("R", "C")) or status.endswith(("R", "C"))
        ) and index < len(entries):
            index += 1
    return count


def _git_agent_context(
    repo_root: Path,
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    def git(source: str, *arguments: str, cwd: Path = repo_root) -> str | None:
        result = _run_context_command(
            f"git.{source}",
            ("git", *arguments),
            cwd=cwd,
        )
        return _required_command_text(result, errors)

    current_root = git("root", "rev-parse", "--show-toplevel")
    head = git("head", "rev-parse", "HEAD")
    branch = git("branch", "branch", "--show-current")
    origin_main = git("origin_main", "rev-parse", "origin/main")
    drift_text = git(
        "origin_drift",
        "rev-list",
        "--left-right",
        "--count",
        "HEAD...origin/main",
    )
    worktree_text = git("worktrees", "worktree", "list", "--porcelain")

    ahead: int | None = None
    behind: int | None = None
    if drift_text is not None:
        try:
            ahead_text, behind_text = drift_text.split()
            ahead, behind = int(ahead_text), int(behind_text)
        except (ValueError, TypeError):
            errors.append(
                _context_error(
                    "git.origin_drift",
                    "invalid_output",
                    f"unexpected drift output: {drift_text!r}",
                    command=(
                        "git",
                        "rev-list",
                        "--left-right",
                        "--count",
                        "HEAD...origin/main",
                    ),
                    return_code=0,
                )
            )

    worktrees = _parse_worktree_porcelain(worktree_text or "")

    def inspect_worktree(
        record: dict[str, Any],
    ) -> tuple[dict[str, Any], ContextCommandResult]:
        path = Path(str(record["path"]))
        result = _run_context_command(
            f"git.worktree_status:{path}",
            (
                "git",
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ),
            cwd=path,
        )
        enriched = dict(record)
        if result.ok:
            dirty_count = _porcelain_status_entry_count(result.stdout)
            enriched["dirty"] = dirty_count > 0
            enriched["dirty_path_count"] = dirty_count
        else:
            enriched["dirty"] = None
            enriched["dirty_path_count"] = None
        return enriched, result

    if worktrees:
        with ThreadPoolExecutor(
            max_workers=min(8, len(worktrees)),
            thread_name_prefix="molt-agent-context",
        ) as executor:
            inspected = list(executor.map(inspect_worktree, worktrees))
        worktrees = []
        for record, result in inspected:
            worktrees.append(record)
            if not result.ok:
                errors.append(result.error_payload())

    canonical_root = worktrees[0]["path"] if worktrees else None
    normalized_current = (
        str(Path(current_root).resolve()) if current_root is not None else None
    )
    normalized_canonical = (
        str(Path(str(canonical_root)).resolve()) if canonical_root is not None else None
    )
    dirty_worktrees = [record for record in worktrees if record.get("dirty") is True]
    return {
        "source": "git commands observed at query time",
        "queried_root": normalized_current,
        "canonical_root": normalized_canonical,
        "queried_root_is_canonical": (
            normalized_current == normalized_canonical
            if normalized_current is not None and normalized_canonical is not None
            else None
        ),
        "branch": branch or "detached",
        "head": head,
        "origin_main": origin_main,
        "ahead": ahead,
        "behind": behind,
        "worktree_count": len(worktrees),
        "dirty_worktree_count": len(dirty_worktrees),
        "worktrees": worktrees,
    }


def _coordination_record_context(
    repo_root: Path,
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    try:
        records = load_records(repo_root)
    except OSError as exc:
        errors.append(_context_error("coordination.records", "read_error", exc))
        return {
            "source": "logs/agents/**/coordination.json",
            "record_count": None,
            "active_count": None,
            "invalid_count": None,
            "collision_count": None,
            "active": [],
            "invalid": [],
            "collisions": [],
        }
    collisions = broad_lane_collisions(records, repo_root)
    active = [
        {
            "task": record.task,
            "status": record.status,
            "proof_role": record.proof_role or "unknown",
            "planned_proof_lane": record.planned_proof_lane or None,
            "shared_target_root": record.shared_target_root or None,
            "path": repo_relative(record.path, repo_root),
        }
        for record in records
        if record.active
    ]
    invalid = [
        {
            "task": record.task,
            "path": repo_relative(record.path, repo_root),
            "error": _bounded_context_text(
                record.payload.get("error", "invalid record")
            ),
        }
        for record in records
        if record.status == "invalid"
    ]
    for record in invalid:
        errors.append(
            _context_error(
                "coordination.records",
                "invalid_record",
                f"{record['path']}: {record['error']}",
            )
        )
    if collisions:
        errors.append(
            _context_error(
                "coordination.records",
                "broad_lane_collision",
                f"{len(collisions)} active broad proof-lane collision(s)",
            )
        )
    return {
        "source": "logs/agents/**/coordination.json",
        "record_count": len(records),
        "active_count": len(active),
        "invalid_count": len(invalid),
        "collision_count": len(collisions),
        "active": active,
        "invalid": invalid,
        "collisions": collisions,
    }


def _claims_context(
    repo_root: Path,
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    relative = Path(claims_status.CLAIMS_REL)
    path = repo_root / relative
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        errors.append(
            _context_error(
                "claims.records",
                "read_error",
                f"{relative.as_posix()}: {exc}",
            )
        )
        return {
            "source": relative.as_posix(),
            "counts": None,
            "live": [],
            "stale": [],
        }
    summary = claims_status.summarize(
        claims_status.parse_rows(text),
        datetime.now(UTC),
    ).as_dict()

    def active_claim(record: object) -> dict[str, object]:
        if not isinstance(record, dict):
            return {}
        return {
            key: record.get(key)
            for key in ("lane", "agent", "utc", "status", "class", "age_hours")
        }

    return {
        "source": relative.as_posix(),
        "counts": summary["counts"],
        "live": [active_claim(record) for record in summary["live"]],
        "stale": [active_claim(record) for record in summary["stale"]],
        "full_detail_command": (
            "uv run --python 3.12 python tools/claims_status.py --json"
        ),
    }


def _proof_audit_context(
    repo_root: Path,
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    command = (
        sys.executable,
        str(repo_root / "tools" / "proof_queue.py"),
        "--repo-root",
        str(repo_root),
        "audit",
        "--json",
        "--max-issues",
        "0",
    )
    result = _run_context_command("proof.audit", command, cwd=repo_root)
    if result.failure_kind is not None:
        errors.append(result.error_payload())
        return {"source_command": list(command), "available": False}
    try:
        payload = json.loads(result.stdout)
        if not isinstance(payload, dict):
            raise TypeError("audit JSON root is not an object")
    except (json.JSONDecodeError, TypeError) as exc:
        errors.append(
            _context_error(
                "proof.audit",
                "invalid_json",
                exc,
                command=command,
                return_code=result.return_code,
            )
        )
        return {"source_command": list(command), "available": False}

    issues = payload.get("issues")
    if not isinstance(issues, list):
        issues = []
    signals = Counter(
        str(issue.get("signal_id", "unknown"))
        for issue in issues
        if isinstance(issue, dict)
    )
    custody_signal_ids = {
        "audit-active-log-missing",
        "audit-active-log-stale",
        "audit-dead-running-guard",
        "audit-memory-guard-summary-incomplete",
        "audit-memory-guard-timeout",
        "audit-native-call-lane-memory-guard-timeout",
    }
    custody_issues = [
        issue
        for issue in issues
        if isinstance(issue, dict) and issue.get("signal_id") in custody_signal_ids
    ]

    def project_issue(issue: object) -> dict[str, object]:
        if not isinstance(issue, dict):
            return {}
        return {
            key: issue.get(key)
            for key in (
                "signal_id",
                "severity",
                "run_id",
                "summary",
                "next_action",
                "artifacts",
            )
        }

    def project_frontier(item: object) -> dict[str, object]:
        if not isinstance(item, dict):
            return {}
        return {
            key: item.get(key)
            for key in (
                "run_id",
                "logical_id",
                "diagnostic",
                "summary",
                "next_action",
                "log_path",
            )
        }

    if result.return_code != 0:
        errors.append(
            _context_error(
                "proof.audit",
                "health_check_failed",
                (
                    "proof queue audit reported unhealthy recorded custody: "
                    f"{payload.get('issue_counts', {})}"
                ),
                command=command,
                return_code=result.return_code,
            )
        )
    frontier = payload.get("frontier_failures")
    if not isinstance(frontier, list):
        frontier = []
    return {
        "source_command": list(command),
        "available": True,
        "return_code": result.return_code,
        "scanned_runs": payload.get("scanned_runs"),
        "active_runs": payload.get("active_runs"),
        "classified_failed_runs": payload.get("classified_failed_runs"),
        "issue_counts": payload.get("issue_counts", {}),
        "issue_signal_counts": dict(sorted(signals.items())),
        "custody_issue_count": len(custody_issues),
        "custody_issues": [project_issue(issue) for issue in custody_issues],
        "frontier_failure_count": len(frontier),
        "frontier_failures": [project_frontier(item) for item in frontier[:3]],
        "frontier_failures_omitted": max(0, len(frontier) - 3),
        "full_detail_command": (
            "uv run --python 3.12 python tools/proof_queue.py audit --json "
            "--max-issues 0"
        ),
    }


def _documentation_context(
    repo_root: Path,
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    pointers = []
    for role, relative in AGENT_CONTEXT_DOCUMENTS:
        inspection_failed = False
        try:
            exists = (repo_root / relative).is_file()
        except OSError as exc:
            exists = False
            inspection_failed = True
            errors.append(
                _context_error(
                    "documentation",
                    "read_error",
                    f"cannot inspect {relative}: {exc}",
                )
            )
        pointers.append({"role": role, "path": relative, "exists": exists})
        if not exists and not inspection_failed:
            errors.append(
                _context_error(
                    "documentation",
                    "missing_canonical_document",
                    f"missing {role}: {relative}",
                )
            )
    try:
        instruction_audit = check_instruction_hierarchy.audit(repo_root).as_dict()
    except (OSError, UnicodeDecodeError) as exc:
        instruction_audit = {"ok": False, "failures": [str(exc)]}
    if not instruction_audit["ok"]:
        errors.append(
            _context_error(
                "documentation.instruction_authority",
                "authority_drift",
                "; ".join(str(item) for item in instruction_audit["failures"]),
            )
        )
    return {
        "pointers": pointers,
        "instruction_authority": {
            "canonical": "AGENTS.md",
            "claude_adapter": "CLAUDE.md",
            "claude_expected_content": check_instruction_hierarchy.CLAUDE_IMPORT.rstrip(
                "\n"
            ),
            "audit": instruction_audit,
        },
    }


def agent_context(repo_root: Path) -> AgentContext:
    root = repo_root.resolve()
    errors: list[dict[str, Any]] = []
    live_facts = {"git": _git_agent_context(root, errors)}
    file_records = {
        "coordination": _coordination_record_context(root, errors),
        "claims": _claims_context(root, errors),
        "proof_audit": _proof_audit_context(root, errors),
    }
    documentation = _documentation_context(root, errors)
    return AgentContext(
        generated_at_utc=utc_now(),
        live_facts=live_facts,
        file_records=file_records,
        documentation=documentation,
        errors=tuple(errors),
    )


def print_text_agent_context(payload: dict[str, Any]) -> None:
    git = payload["live_facts"]["git"]
    head = str(git.get("head") or "unknown")[:12]
    origin = str(git.get("origin_main") or "unknown")[:12]
    print(
        f"agent context: {'ok' if payload['ok'] else 'attention'} "
        f"schema={payload['schema']}"
    )
    print(
        "git: root={root} branch={branch} head={head} origin/main={origin} "
        "ahead={ahead} behind={behind}".format(
            root=git.get("canonical_root") or "unknown",
            branch=git.get("branch") or "detached",
            head=head,
            origin=origin,
            ahead=git.get("ahead"),
            behind=git.get("behind"),
        )
    )
    dirty = [item for item in git["worktrees"] if item.get("dirty") is True]
    print(
        f"worktrees: total={git['worktree_count']} dirty={len(dirty)} "
        f"queried_is_canonical={git['queried_root_is_canonical']}"
    )
    for item in dirty:
        print(
            f"- dirty {item['path']} branch={item['branch']} "
            f"paths={item['dirty_path_count']}"
        )
    coordination = payload["file_records"]["coordination"]
    print(
        "coordination records: total={record_count} active={active_count} "
        "invalid={invalid_count} collisions={collision_count}".format(**coordination)
    )
    claims = payload["file_records"]["claims"]
    counts = claims.get("counts") or {"live": "?", "stale": "?", "retired": "?"}
    print(
        f"claims: live={counts['live']} stale={counts['stale']} "
        f"retired={counts['retired']}"
    )
    proof = payload["file_records"]["proof_audit"]
    issue_counts = proof.get("issue_counts", {})
    print(
        "proof audit: active={active} errors={errors} warnings={warnings} "
        "custody={custody} frontier={frontier}".format(
            active=proof.get("active_runs", "?"),
            errors=issue_counts.get("error", 0),
            warnings=issue_counts.get("warning", 0),
            custody=proof.get("custody_issue_count", "?"),
            frontier=proof.get("frontier_failure_count", "?"),
        )
    )
    print(
        "docs: AGENTS.md; docs/agent/ORCHESTRATION.md; docs/CANONICALS.md; "
        "docs/INDEX.md; docs/spec/README.md"
    )
    for error in payload["errors"]:
        return_code = error.get("return_code")
        rc = "?" if return_code is None else return_code
        print(
            f"- error source={error['source']} kind={error['kind']} rc={rc}: "
            f"{error['message']}"
        )


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
    session_id = (
        session or os.environ.get("MOLT_SESSION_ID") or f"agent-{task}-{os.getpid()}"
    )
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
        "env_sh": str(base / "env.sh"),
        "env_ps1": str(base / "env.ps1"),
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
- Env: {record["env_sh"]}
- Env PowerShell: {record["env_ps1"]}
- MOLT_SESSION_ID: {record["session_id"]}
- CARGO_TARGET_DIR: {record.get("dx_env", {}).get("CARGO_TARGET_DIR", "see env file")}

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
4. Run commands with `molt dx run -- <command>` or source the env file first.

## Resume Instructions
- Export MOLT_SESSION_ID="{record["session_id"]}"
- POSIX: source "{record["env_sh"]}"
- PowerShell: . "{record["env_ps1"]}"
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


def default_codex_crash_report_path(repo_root: Path) -> Path:
    return repo_root / CODEX_CRASH_ROOT / f"crash_{artifact_stamp()}_{os.getpid()}.json"


def _default_codex_home() -> Path:
    raw = os.environ.get("CODEX_HOME")
    if raw:
        return Path(raw)
    return Path.home() / ".codex"


def _default_codex_runtime_cache_root() -> Path:
    if os.name == "nt":
        return Path.home() / ".cache" / "codex-runtimes"
    return Path.home() / ".cache" / "codex-runtimes"


def _extract_json_object_after_marker(text: str, marker: str) -> dict[str, Any] | None:
    marker_index = text.find(marker)
    if marker_index < 0:
        return None
    start = text.find("{", marker_index + len(marker))
    if start < 0:
        return None

    depth = 0
    in_string = False
    escaped = False
    for index in range(start, len(text)):
        char = text[index]
        if escaped:
            escaped = False
            continue
        if char == "\\":
            escaped = in_string
            continue
        if char == '"':
            in_string = not in_string
            continue
        if in_string:
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                try:
                    parsed = json.loads(text[start : index + 1])
                except json.JSONDecodeError:
                    return None
                return parsed if isinstance(parsed, dict) else None
    return None


def parse_codex_crash_text(text: str) -> dict[str, Any]:
    code_match = re.search(r"\(code=(-?\d+),\s*signal=([^)]+)\)", text)
    code: int | None = None
    signal: str | None = None
    if code_match:
        code = int(code_match.group(1))
        signal = code_match.group(2).strip()

    lowercase_text = text.lower()
    markers: list[str] = []
    if "projectdoc exceeds remaining budget" in lowercase_text:
        markers.append("projectdoc_exceeds_remaining_budget")
    if "process interrupt is not supported by this process backend" in lowercase_text:
        markers.append("exec_backend_interrupt_unsupported")

    most_recent_error = _extract_json_object_after_marker(text, "Most recent error:")
    fields = most_recent_error.get("fields", {}) if most_recent_error else {}
    if not isinstance(fields, dict):
        fields = {}

    warning_paths = sorted(
        dict.fromkeys(
            match.group(1) for match in re.finditer(r'"path"\s*:\s*"([^"]+)"', text)
        )
    )
    return {
        "crash_text_sha256": hashlib.sha256(
            text.encode("utf-8", errors="surrogateescape")
        ).hexdigest(),
        "crash_text_bytes": len(text.encode("utf-8", errors="surrogateescape")),
        "code": code,
        "signal": signal,
        "most_recent_error": {
            "parsed": most_recent_error is not None,
            "timestamp": most_recent_error.get("timestamp")
            if most_recent_error
            else None,
            "level": most_recent_error.get("level") if most_recent_error else None,
            "target": most_recent_error.get("target") if most_recent_error else None,
            "message": fields.get("message"),
            "error": fields.get("error"),
            "path": fields.get("path"),
        },
        "warning_paths": warning_paths,
        "markers": markers,
    }


def classify_codex_crash(parsed: dict[str, Any]) -> list[dict[str, str]]:
    classifications: list[dict[str, str]] = []
    code = parsed.get("code")
    most_recent = parsed.get("most_recent_error") or {}
    target = str(most_recent.get("target") or "")
    message = str(most_recent.get("message") or "")
    error = str(most_recent.get("error") or "")
    diagnostic_text = f"{message}\n{error}"
    markers = set(parsed.get("markers") or [])

    if code == CODEX_WINDOWS_CONTROL_C_EXIT:
        classifications.append(
            {
                "id": "windows_status_control_c_exit",
                "severity": "control_plane_interruption",
                "summary": (
                    "Windows crash code 3221225786 is 0xC000013A "
                    "STATUS_CONTROL_C_EXIT; treat the dialog as an interrupted "
                    "or torn-down control-plane process until correlated with "
                    "logs and live commands."
                ),
            }
        )
    if (
        target == "codex_core::responses_retry"
        or "stream disconnected" in diagnostic_text
    ):
        classifications.append(
            {
                "id": "responses_retry_stream_disconnected",
                "severity": "crash_adjacent_retry_pressure",
                "summary": (
                    "The most recent warning is a response-stream retry; reduce "
                    "orchestration/proof fanout and preserve evidence instead "
                    "of retrying tool discovery or long commands in a storm."
                ),
            }
        )
    if (
        "interface.defaultPrompt" in diagnostic_text
        or "maximum of 3 prompts" in diagnostic_text
    ):
        classifications.append(
            {
                "id": "plugin_default_prompt_manifest_warning",
                "severity": "plugin_manifest_pressure",
                "summary": (
                    "A plugin manifest defaultPrompt warning is crash-adjacent "
                    "control-plane evidence, not authority to hand-edit cached "
                    "plugin manifests during active Molt work."
                ),
            }
        )
    if (
        "state db discrepancy" in diagnostic_text
        or "read_repair_rollout_path" in diagnostic_text
    ):
        classifications.append(
            {
                "id": "rollout_state_repair_symptom",
                "severity": "resume_state_pressure",
                "summary": (
                    "State DB repair text near a crash is a resume symptom; "
                    "verify repo state and active commands before touching "
                    "Codex SQLite or rollout files."
                ),
            }
        )
    if "exec_backend_interrupt_unsupported" in markers:
        classifications.append(
            {
                "id": "exec_backend_interrupt_unsupported",
                "severity": "control_plane_interruption",
                "summary": (
                    "Codex attempted to interrupt a unified exec process, but "
                    "this backend does not support process interrupts. Avoid "
                    "Ctrl-C/write_stdin interruption as a recovery mechanism; "
                    "use bounded commands, proof-queue stale cleanup, and "
                    "Molt-owned process custody instead."
                ),
            }
        )
    if "projectdoc_exceeds_remaining_budget" in markers:
        classifications.append(
            {
                "id": "projectdoc_remaining_budget_exhausted",
                "severity": "instruction_budget_pressure",
                "summary": (
                    "Project instructions exceeded the remaining context budget; "
                    "keep root/global agent guidance compact, preserve full "
                    "guides outside the auto-loaded path, and verify the "
                    "project-doc budget guard before resuming a large thread."
                ),
            }
        )
    if not classifications:
        classifications.append(
            {
                "id": "unclassified_codex_control_plane_crash",
                "severity": "unknown",
                "summary": (
                    "No known crash-adjacent warning class matched; preserve "
                    "the capsule and correlate with Codex logs, active command "
                    "sessions, and repo state."
                ),
            }
        )
    return classifications


def _plugin_manifest_roots(
    *, codex_home: Path, runtime_cache_root: Path
) -> tuple[Path, ...]:
    return (
        codex_home / "plugins",
        runtime_cache_root,
    )


def _plugin_manifest_summary(
    *,
    roots: Sequence[Path],
    max_manifests: int,
) -> dict[str, Any]:
    scanned = 0
    unreadable = 0
    default_prompt_violations: list[dict[str, Any]] = []
    roots_payload: list[str] = []

    for root in roots:
        resolved_root = root.expanduser()
        roots_payload.append(str(resolved_root))
        if not resolved_root.exists():
            continue
        try:
            iterator = resolved_root.rglob("plugin.json")
            for manifest in iterator:
                if scanned >= max_manifests:
                    break
                scanned += 1
                try:
                    payload = json.loads(
                        manifest.read_text(encoding="utf-8", errors="replace")
                    )
                except (OSError, json.JSONDecodeError):
                    unreadable += 1
                    continue
                interface = payload.get("interface")
                if not isinstance(interface, dict):
                    continue
                prompts = interface.get("defaultPrompt")
                if not isinstance(prompts, list):
                    continue
                if len(prompts) <= CODEX_DEFAULT_PROMPT_LIMIT:
                    continue
                default_prompt_violations.append(
                    {
                        "path": str(manifest),
                        "prompt_count": len(prompts),
                        "limit": CODEX_DEFAULT_PROMPT_LIMIT,
                    }
                )
        except OSError:
            unreadable += 1
        if scanned >= max_manifests:
            break

    return {
        "roots": roots_payload,
        "manifest_count_scanned": scanned,
        "scan_truncated": scanned >= max_manifests,
        "unreadable_manifest_count": unreadable,
        "default_prompt_limit": CODEX_DEFAULT_PROMPT_LIMIT,
        "default_prompt_violation_count": len(default_prompt_violations),
        "default_prompt_violations": default_prompt_violations,
    }


def codex_crash_next_actions(classifications: Sequence[dict[str, str]]) -> list[str]:
    action_ids = {item["id"] for item in classifications}
    actions = [
        "Run git status --short --branch and inspect any active command/session evidence before assuming repo state changed.",
        "Keep one active structural arc and one bounded proof lane until the control plane is stable.",
        "Do not delete or hand-edit Codex state databases, plugin caches, cached plugin manifests, or rollout summaries as a first response.",
    ]
    if "responses_retry_stream_disconnected" in action_ids:
        actions.append(
            "Reduce optional MCP/plugin/tool-discovery load and avoid retry storms; continue from local terminal evidence where possible."
        )
    if "plugin_default_prompt_manifest_warning" in action_ids:
        actions.append(
            "If defaultPrompt violations persist after active work is quiescent, disable or update optional plugins through normal operator-controlled config rather than editing cache files."
        )
    if "rollout_state_repair_symptom" in action_ids:
        actions.append(
            "Copy bounded nearby Codex logs into logs/ or tmp/ if state-repair evidence is needed; do not mutate SQLite state in-place."
        )
    if "projectdoc_remaining_budget_exhausted" in action_ids:
        actions.append(
            "Verify root/global instruction files stay compact and run tests/test_agent_contract_budget.py before resuming a large thread; move long guidance into referenced docs, not auto-loaded project instructions."
        )
    if "exec_backend_interrupt_unsupported" in action_ids:
        actions.append(
            "Do not retry Ctrl-C/write_stdin interruption. Let the command finish or inspect queue/guard logs, then use proof_queue prune-stale or custody-aware Molt cleanup from a fresh command only for live-proved Molt-owned children."
        )
    return actions


def codex_crash_payload(
    *,
    repo_root: Path,
    crash_text: str,
    report_path: Path,
    codex_home: Path,
    runtime_cache_root: Path,
    max_plugin_manifests: int,
    record_crash_text: bool,
) -> dict[str, Any]:
    parsed = parse_codex_crash_text(crash_text)
    classifications = classify_codex_crash(parsed)
    payload: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "kind": "codex_crash_diagnostic",
        "status": "completed",
        "created_at_utc": utc_now(),
        "repo_root": str(repo_root),
        "report_path": repo_relative(report_path, repo_root),
        "privacy": {
            "records_raw_crash_text": record_crash_text,
            "records_codex_state": False,
            "records_plugin_manifest_text": False,
            "recorded_fields": [
                "crash_text_hash",
                "parsed_error_fields",
                "classification",
                "plugin_manifest_counts",
                "next_actions",
            ],
        },
        "environment": environment_snapshot(repo_root),
        "parsed": parsed,
        "classification": classifications,
        "plugin_manifests": _plugin_manifest_summary(
            roots=_plugin_manifest_roots(
                codex_home=codex_home,
                runtime_cache_root=runtime_cache_root,
            ),
            max_manifests=max_plugin_manifests,
        ),
        "next_actions": codex_crash_next_actions(classifications),
    }
    if record_crash_text:
        payload["raw_crash_text"] = crash_text
    return payload


def run_codex_crash_diagnostic(args: argparse.Namespace) -> int:
    repo_root = args.repo_root.resolve()
    if args.crash_text:
        crash_text = "\n".join(args.crash_text)
    elif args.from_file is not None:
        try:
            crash_text = args.from_file.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            print(f"agent_coordination: cannot read crash text: {exc}", file=sys.stderr)
            return 2
    else:
        crash_text = sys.stdin.read()
    if not crash_text.strip():
        print("agent_coordination: codex-crash requires crash text", file=sys.stderr)
        return 2
    if args.max_plugin_manifests < 0:
        print(
            "agent_coordination: --max-plugin-manifests must be >= 0", file=sys.stderr
        )
        return 2

    try:
        report_path = resolve_canonical_artifact_path(
            repo_root,
            args.out or default_codex_crash_report_path(repo_root),
        )
    except ValueError as exc:
        print(f"agent_coordination: {exc}", file=sys.stderr)
        return 2

    payload = codex_crash_payload(
        repo_root=repo_root,
        crash_text=crash_text,
        report_path=report_path,
        codex_home=args.codex_home,
        runtime_cache_root=args.runtime_cache_root,
        max_plugin_manifests=args.max_plugin_manifests,
        record_crash_text=args.record_crash_text,
    )
    write_json(report_path, payload)
    primary = payload["classification"][0]
    print(
        "codex-crash: {kind}; report={path}; raw_text_recorded={raw}".format(
            kind=primary["id"],
            path=repo_relative(report_path, repo_root),
            raw=payload["privacy"]["records_raw_crash_text"],
        ),
        file=sys.stderr,
    )
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    return 0


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
    dx_env = RunContext(
        repo_root,
        session_prefix=f"agent-{task}",
        prefer_external_artifacts=True,
    ).dx_env(
        os.environ | {"MOLT_SESSION_ID": record["session_id"]},
        create_dirs=False,
    )
    record["dx_env"] = {key: dx_env[key] for key in DX_ENV_KEYS if key in dx_env}
    (base / "env.sh").write_text(
        render_env(dx_env, DX_ENV_KEYS, "posix") + "\n",
        encoding="utf-8",
    )
    (base / "env.ps1").write_text(
        render_env(dx_env, DX_ENV_KEYS, "powershell") + "\n",
        encoding="utf-8",
    )
    report_path.write_text(render_report(record), encoding="utf-8")
    write_json(base / "coordination.json", record)
    with (base / "progress.log").open("a", encoding="utf-8") as progress:
        progress.write(
            f"{created_at} initialized task={task} session={record['session_id']}\n"
        )
    return record


def _decode_record_bytes(data: bytes) -> str:
    """Decode a coordination record tolerant of BOM / encoding variance.

    Records are canonically UTF-8 (see ``write_json``), but an agent that writes
    a record through a Windows shell redirect (PowerShell defaults to UTF-16-LE
    with a BOM) emits UTF-16 or UTF-8-BOM bytes. The read boundary must tolerate
    all of these: a single stray-encoded record previously raised
    ``UnicodeDecodeError`` out of ``load_records`` (the UTF-16 case) or was
    silently dropped as invalid (the UTF-8-BOM case), which made ``scan`` /
    ``check`` unusable and silently defeated cross-agent coordination on Windows.
    """
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16")
    if data[:3] == b"\xef\xbb\xbf":
        return data.decode("utf-8-sig")
    return data.decode("utf-8")


def load_records(repo_root: Path) -> list[CoordinationRecord]:
    root = repo_root / LOG_ROOT
    if not root.is_dir():
        return []
    records: list[CoordinationRecord] = []
    for path in sorted(root.glob("**/coordination.json")):
        try:
            payload = json.loads(_decode_record_bytes(path.read_bytes()))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
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

    context = sub.add_parser(
        "context",
        help="summarize live repository facts and recorded coordination health",
    )
    context.add_argument("--json", action="store_true")

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

    crash = sub.add_parser(
        "codex-crash",
        help=(
            "classify a Codex crash dialog and write a privacy-bounded recovery "
            "capsule under canonical artifact roots"
        ),
    )
    crash.add_argument(
        "--out",
        type=Path,
        help=(
            "JSON report path; must stay under logs/, tmp/, bench/results/, or "
            "target/ (default: logs/agents/codex_crash/crash_<timestamp>.json)"
        ),
    )
    crash.add_argument(
        "--crash-text",
        action="append",
        help="crash dialog text; may be repeated, otherwise stdin is read",
    )
    crash.add_argument(
        "--from-file",
        type=Path,
        help="read crash dialog text from a file instead of stdin",
    )
    crash.add_argument(
        "--codex-home",
        type=Path,
        default=_default_codex_home(),
        help="Codex home used for bounded plugin-manifest counting",
    )
    crash.add_argument(
        "--runtime-cache-root",
        type=Path,
        default=_default_codex_runtime_cache_root(),
        help="Codex runtime cache root used for bounded plugin-manifest counting",
    )
    crash.add_argument(
        "--max-plugin-manifests",
        type=int,
        default=2000,
        help="maximum plugin.json files inspected for manifest pressure (default: 2000)",
    )
    crash.add_argument(
        "--record-crash-text",
        action="store_true",
        help="include raw crash text in the report; default records only parsed fields and a hash",
    )
    crash.add_argument("--json", action="store_true")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = args.repo_root.resolve()
    if args.command == "init":
        record = init_task(args)
        if args.json:
            print(json.dumps(record, indent=2, sort_keys=True))
        else:
            print(f"Created task scaffold at {task_dir(repo_root, record['task'])}")
            print("Read docs/ops/MULTI_AGENT_COORDINATION.md before long proof lanes.")
        return 0

    if args.command == "env":
        payload = environment_snapshot(repo_root)
        if args.json:
            print(json.dumps(payload, indent=2, sort_keys=True))
        else:
            print_text_environment(payload)
        return 0

    if args.command == "context":
        payload = agent_context(repo_root).as_dict()
        if args.json:
            print(json.dumps(payload, sort_keys=True, separators=(",", ":")))
        else:
            print_text_agent_context(payload)
        return 0 if payload["ok"] else 2

    if args.command == "codex-stall":
        return run_codex_stall_diagnostic(args)

    if args.command == "codex-crash":
        return run_codex_crash_diagnostic(args)

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
