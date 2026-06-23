from __future__ import annotations

import json
import os
from pathlib import Path
import sys

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


def test_proof_plan_recommends_focused_lanes_for_explicit_paths(tmp_path: Path) -> None:
    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            [
                "--repo-root",
                str(tmp_path),
                "proof-plan",
                "tools/agent_coordination.py",
                "tests/differential/basic/imported_generator_lowering.py",
                "runtime/molt-backend/src/tir/type_refine.rs",
            ]
        )
    )

    lanes = {item["lane"]: item for item in payload["recommendations"]}
    assert payload["source"] == "explicit"
    assert lanes["focused_differential"]["priority"] == "P0"
    assert (
        "tests/molt_diff.py tests/differential/basic/imported_generator_lowering.py"
        in lanes["focused_differential"]["commands"][0]
    )
    assert lanes["agent_coordination"]["proof_role"] == "implementer"
    assert lanes["tir_type_refine"]["commands"] == [
        "cargo test -p molt-backend type_refine -- --nocapture"
    ]


def test_proof_plan_file_rules_do_not_match_same_prefix_siblings(
    tmp_path: Path,
) -> None:
    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            [
                "--repo-root",
                str(tmp_path),
                "proof-plan",
                "tools/agent_coordination.py.bak",
                "src/molt/frontend/visitors/calls.py",
            ]
        )
    )

    lanes = {item["lane"]: item for item in payload["recommendations"]}
    assert "agent_coordination" not in lanes
    assert lanes["frontend_targeted"]["covered_paths"] == [
        "src/molt/frontend/visitors/calls.py"
    ]


def test_proof_plan_normalize_preserves_dot_directories(tmp_path: Path) -> None:
    assert (
        agent_coordination.normalize_repo_path(
            "./.github/workflows/ci.yml",
            tmp_path,
        )
        == ".github/workflows/ci.yml"
    )


def test_proof_plan_uses_git_status_when_paths_are_omitted(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        agent_coordination,
        "git_status_paths",
        lambda repo_root: [
            "tools/check_subprocess_guard_coverage.py",
            "tests/differential/basic/example.py",
        ],
    )

    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            ["--repo-root", str(tmp_path), "proof-plan", "--json"]
        )
    )

    assert payload["source"] == "git-status"
    assert payload["input_paths"] == [
        "tools/check_subprocess_guard_coverage.py",
        "tests/differential/basic/example.py",
    ]
    assert [item["lane"] for item in payload["recommendations"]] == [
        "focused_differential",
        "subprocess_guard_coverage",
    ]


def test_proof_plan_clean_status_does_not_invent_broad_work(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(agent_coordination, "git_status_paths", lambda repo_root: [])

    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(["--repo-root", str(tmp_path), "proof-plan"])
    )

    assert payload["source"] == "git-status"
    assert payload["input_paths"] == []
    assert payload["recommendations"] == []


def test_codex_stall_launch_uses_memory_guard_by_default(tmp_path: Path) -> None:
    args = agent_coordination.parse_args(
        [
            "--repo-root",
            str(tmp_path),
            "codex-stall",
            "--",
            "python",
            "-c",
            "pass",
        ]
    )

    launch = agent_coordination.codex_stall_launch_command(
        args,
        ["python", "-c", "pass"],
    )

    assert launch[:2] == [sys.executable, str(tmp_path / "tools" / "memory_guard.py")]
    assert launch[-4:] == ["--", "python", "-c", "pass"]


def test_codex_stall_telemetry_records_first_output_and_idle_spans(
    monkeypatch,
) -> None:
    monotonic_values = iter([1.25, 1.60])
    monkeypatch.setattr(
        agent_coordination.time,
        "monotonic",
        lambda: next(monotonic_values),
    )
    telemetry = agent_coordination.CodexStallTelemetry(
        idle_threshold_sec=0.1,
        max_spans=10,
        started_monotonic=1.0,
    )

    telemetry.observe("stdout", 5)
    telemetry.observe("stdout", 3)

    streams = telemetry.finish(0.75)
    stdout = streams["stdout"]
    assert stdout["byte_count"] == 8
    assert stdout["first_output_gap_sec"] == 0.25
    assert stdout["max_idle_gap_sec"] == 0.35
    assert [span["kind"] for span in stdout["idle_spans"]] == [
        "first_output_gap",
        "between_outputs",
        "terminal_idle",
    ]


def test_codex_stall_report_omits_child_output_and_argv_by_default(
    tmp_path: Path,
) -> None:
    report = tmp_path / "logs" / "agents" / "codex_stall" / "privacy.json"

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "codex-stall",
            "--no-memory-guard",
            "--no-live-notices",
            "--idle-threshold-sec",
            "0.001",
            "--poll-sec",
            "0.001",
            "--out",
            str(report),
            "--",
            sys.executable,
            "-c",
            "print('codex-secret-output')",
        ]
    )

    assert rc == 0
    report_text = report.read_text(encoding="utf-8")
    assert "codex-secret-output" not in report_text
    payload = json.loads(report_text)
    assert payload["privacy"]["records_child_output_text"] is False
    assert payload["privacy"]["records_codex_state"] is False
    assert payload["command"]["argv_recorded"] is False
    assert "argv" not in payload["command"]
    assert payload["streams"]["combined"]["byte_count"] > 0
