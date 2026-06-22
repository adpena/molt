from __future__ import annotations

import json
import os
from pathlib import Path

from tools import agent_coordination


def test_agent_coordination_init_writes_report_and_json(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setenv("MOLT_AGENT_ID", "agent-a")
    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "init",
            "stdlib-lane",
            "--role",
            "reducer",
            "--lane",
            "tests/differential/stdlib/json_basic.py",
            "--owned",
            "src/molt/stdlib/json.py",
            "--json",
        ]
    )

    assert rc == 0
    task_dir = tmp_path / "logs" / "agents" / "stdlib-lane"
    payload = json.loads((task_dir / "coordination.json").read_text(encoding="utf-8"))
    assert payload["schema_version"] == 1
    assert payload["agent"] == "agent-a"
    assert payload["session_id"] == "stdlib-lane"
    assert payload["proof_role"] == "reducer"
    assert payload["planned_proof_lane"] == "tests/differential/stdlib/json_basic.py"
    assert payload["owned_paths"] == ["src/molt/stdlib/json.py"]
    assert payload["progress_log"] == "logs/agents/stdlib-lane/progress.log"
    assert payload["environment"]["recommended_python_command"]
    assert "python_executable" in payload["environment"]
    report = tmp_path / payload["report_path"]
    assert report.exists()
    report_text = report.read_text(encoding="utf-8")
    assert "docs/ops/MULTI_AGENT_COORDINATION.md" in report_text
    assert "## Environment" in report_text


def test_agent_coordination_environment_snapshot_prefers_explicit_python(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        agent_coordination,
        "command_path",
        lambda name, environ=None: None,
    )

    payload = agent_coordination.environment_snapshot(
        tmp_path,
        environ={"PYTHON": "custom-python", "ComSpec": "cmd.exe"},
    )

    assert payload["recommended_python_command"] == "custom-python"
    assert payload["shell"] == "cmd.exe"
    assert payload["repo_root"] == str(tmp_path)


def test_agent_coordination_environment_snapshot_falls_back_to_available_launcher(
    monkeypatch,
    tmp_path: Path,
) -> None:
    available = {"python": "/usr/bin/python"}
    monkeypatch.setattr(
        agent_coordination,
        "command_path",
        lambda name, environ=None: available.get(name),
    )

    payload = agent_coordination.environment_snapshot(tmp_path, environ={})

    assert payload["recommended_python_command"] == "python"
    assert payload["python"] == "/usr/bin/python"
    assert payload["python3"] is None


def test_agent_coordination_environment_snapshot_skips_windowsapps_alias(
    monkeypatch,
    tmp_path: Path,
) -> None:
    def fake_command_path(
        name: str,
        environ: dict[str, str] | None = None,
    ) -> str | None:
        return {
            "python": None,
            "python3": r"C:\Users\name\AppData\Local\Microsoft\WindowsApps\python3.exe",
            "py": r"C:\Windows\py.exe",
        }.get(name)

    monkeypatch.setattr(agent_coordination, "command_path", fake_command_path)

    payload = agent_coordination.environment_snapshot(tmp_path, environ={})

    assert payload["python3_usable"] is False
    assert payload["recommended_python_command"] == "py"


def test_agent_coordination_command_path_uses_supplied_environment(
    tmp_path: Path,
) -> None:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    executable = bin_dir / ("agent-tool.cmd" if os.name == "nt" else "agent-tool")
    executable.write_text("@echo off\n" if os.name == "nt" else "#!/bin/sh\n")
    if os.name != "nt":
        executable.chmod(0o755)

    env = {"PATH": str(bin_dir), "PATHEXT": ".CMD"}

    assert agent_coordination.command_path("agent-tool", env) == str(executable)


def test_agent_coordination_choose_bash_skips_wsl_shims(monkeypatch) -> None:
    def fake_command_paths(
        name: str,
        environ: dict[str, str] | None = None,
    ) -> list[str]:
        assert name == "bash"
        return [
            r"C:\Windows\System32\bash.exe",
            r"C:\Users\name\AppData\Local\Microsoft\WindowsApps\bash.exe",
            r"C:\Program Files\Git\bin\bash.exe",
        ]

    monkeypatch.setattr(agent_coordination, "command_paths", fake_command_paths)

    assert agent_coordination.choose_bash({}) == r"C:\Program Files\Git\bin\bash.exe"


def _write_record(
    root: Path,
    task: str,
    *,
    role: str = agent_coordination.BROAD_ROLE,
    status: str = "running",
    lane: str = "tests/differential/basic",
    target: str = "target",
) -> None:
    task_dir = root / "logs" / "agents" / task
    task_dir.mkdir(parents=True)
    payload = {
        "schema_version": 1,
        "task": task,
        "status": status,
        "proof_role": role,
        "planned_proof_lane": lane,
        "shared_target_root": target,
    }
    (task_dir / "coordination.json").write_text(json.dumps(payload), encoding="utf-8")


def test_agent_coordination_scan_flags_broad_lane_collisions(tmp_path: Path) -> None:
    _write_record(tmp_path, "sweep-a")
    _write_record(tmp_path, "sweep-b")
    _write_record(tmp_path, "targeted", role="implementer")
    _write_record(tmp_path, "done", status="done")

    payload = agent_coordination.summary_payload(tmp_path)

    assert len(payload["records"]) == 4
    assert payload["collisions"] == [
        {
            "kind": "broad_lane_collision",
            "shared_target_root": "target",
            "planned_proof_lane": "tests/differential/basic",
            "tasks": ["sweep-a", "sweep-b"],
            "paths": [
                "logs/agents/sweep-a/coordination.json",
                "logs/agents/sweep-b/coordination.json",
            ],
        }
    ]


def test_agent_coordination_check_returns_nonzero_on_collision(tmp_path: Path) -> None:
    _write_record(tmp_path, "sweep-a")
    _write_record(tmp_path, "sweep-b")

    assert (
        agent_coordination.main(["--repo-root", str(tmp_path), "check", "--json"]) == 2
    )
