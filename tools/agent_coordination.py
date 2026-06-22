#!/usr/bin/env python3
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import UTC, datetime
import json
import os
import platform
from pathlib import Path
import sys
from typing import Any, Sequence


REPO_ROOT = Path(__file__).resolve().parents[1]
LOG_ROOT = Path("logs/agents")
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


def utc_now() -> str:
    return datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


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

    env = sub.add_parser("env", help="print local agent environment facts")
    env.add_argument("--json", action="store_true")
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
