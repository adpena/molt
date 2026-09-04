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
    assert payload["session_id"].startswith("agent-stdlib-lane-")
    assert payload["dx_env"]["MOLT_SESSION_ID"] == payload["session_id"]
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


def test_load_records_tolerates_utf16_and_bom_records(tmp_path: Path) -> None:
    """A record written through a Windows shell redirect (PowerShell `>` emits
    UTF-16-LE+BOM) must not crash load_records or be silently dropped as invalid.
    Regression: a single stray-encoded record previously raised UnicodeDecodeError
    out of scan/check (UTF-16) or parsed as invalid (UTF-8-BOM), which silently
    defeated cross-agent coordination on Windows."""
    agents = tmp_path / "logs" / "agents"

    def _record(task: str, status: str, role: str, lane: str) -> dict:
        return {
            "schema_version": 1,
            "task": task,
            "status": status,
            "proof_role": role,
            "planned_proof_lane": lane,
        }

    utf8_dir = agents / "utf8-lane"
    utf8_dir.mkdir(parents=True)
    (utf8_dir / "coordination.json").write_text(
        json.dumps(_record("utf8-lane", "running", "implementer", "lane/utf8")),
        encoding="utf-8",
    )

    utf16_dir = agents / "utf16-lane"
    utf16_dir.mkdir(parents=True)
    (utf16_dir / "coordination.json").write_bytes(
        json.dumps(_record("utf16-lane", "running", "integrator", "lane/utf16")).encode(
            "utf-16"
        )
    )

    bom_dir = agents / "bom-lane"
    bom_dir.mkdir(parents=True)
    (bom_dir / "coordination.json").write_bytes(
        b"\xef\xbb\xbf"
        + json.dumps(_record("bom-lane", "blocked", "reviewer", "lane/bom")).encode(
            "utf-8"
        )
    )

    records = agent_coordination.load_records(tmp_path)  # must not raise
    by_task = {record.task: record.payload for record in records}

    assert by_task["utf8-lane"]["status"] == "running"
    assert by_task["utf16-lane"]["status"] == "running"
    assert by_task["utf16-lane"]["planned_proof_lane"] == "lane/utf16"
    assert by_task["bom-lane"]["status"] == "blocked"
    # None of the three were dropped as invalid.
    assert all(payload.get("status") != "invalid" for payload in by_task.values()), (
        by_task
    )


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


def test_agent_context_git_facts_cover_origin_drift_and_dirty_worktrees(
    monkeypatch,
    tmp_path: Path,
) -> None:
    canonical = tmp_path / "canonical"
    current = tmp_path / "worktree"
    canonical.mkdir()
    current.mkdir()
    responses = {
        "git.root": (0, str(current)),
        "git.head": (0, "a" * 40),
        "git.branch": (0, "feature/context"),
        "git.origin_main": (0, "b" * 40),
        "git.origin_drift": (0, "2 3"),
        "git.worktrees": (
            0,
            f"worktree {canonical.as_posix()}\nHEAD {'b' * 40}\n"
            "branch refs/heads/main\n\n"
            f"worktree {current.as_posix()}\nHEAD {'a' * 40}\n"
            "branch refs/heads/feature/context\n",
        ),
        f"git.worktree_status:{canonical}": (0, ""),
        f"git.worktree_status:{current}": (0, " M tools/agent_coordination.py\0"),
    }

    def fake_run(source, command, *, cwd, timeout=30.0):
        return_code, stdout = responses[source]
        return agent_coordination.ContextCommandResult(
            source=source,
            command=tuple(command),
            return_code=return_code,
            stdout=stdout,
        )

    monkeypatch.setattr(agent_coordination, "_run_context_command", fake_run)
    errors = []

    payload = agent_coordination._git_agent_context(current, errors)

    assert errors == []
    assert payload["canonical_root"] == str(canonical.resolve())
    assert payload["queried_root"] == str(current.resolve())
    assert payload["queried_root_is_canonical"] is False
    assert payload["ahead"] == 2
    assert payload["behind"] == 3
    assert payload["dirty_worktree_count"] == 1
    assert payload["worktrees"][1]["dirty_path_count"] == 1


def test_agent_context_proof_audit_preserves_dead_custody_and_nonzero_status(
    monkeypatch,
    tmp_path: Path,
) -> None:
    audit_payload = {
        "scanned_runs": 4,
        "active_runs": 1,
        "classified_failed_runs": 1,
        "issue_counts": {"error": 1, "warning": 1},
        "issues": [
            {
                "signal_id": "audit-dead-running-guard",
                "severity": "error",
                "run_id": "dead-run",
                "summary": "guard is dead",
            },
            {
                "signal_id": "audit-weak-proof-metadata",
                "severity": "warning",
                "run_id": "old-run",
                "summary": "metadata is weak",
            },
        ],
        "frontier_failures": [{"run_id": "frontier-run"}],
    }
    monkeypatch.setattr(
        agent_coordination,
        "_run_context_command",
        lambda source, command, *, cwd, timeout=30.0: (
            agent_coordination.ContextCommandResult(
                source=source,
                command=tuple(command),
                return_code=1,
                stdout=json.dumps(audit_payload),
            )
        ),
    )
    errors = []

    payload = agent_coordination._proof_audit_context(tmp_path, errors)

    assert payload["available"] is True
    assert payload["custody_issue_count"] == 1
    assert payload["custody_issues"][0]["run_id"] == "dead-run"
    assert payload["issue_signal_counts"] == {
        "audit-dead-running-guard": 1,
        "audit-weak-proof-metadata": 1,
    }
    assert errors[0]["kind"] == "health_check_failed"
    assert errors[0]["return_code"] == 1


def test_agent_context_command_failure_is_structured_and_cli_returns_nonzero(
    monkeypatch,
    capsys,
    tmp_path: Path,
) -> None:
    failure = {
        "source": "git.head",
        "kind": "timeout",
        "command": ["git", "rev-parse", "HEAD"],
        "return_code": None,
        "message": "command exceeded 30s timeout",
    }
    model = agent_coordination.AgentContext(
        generated_at_utc="2026-09-04T00:00:00Z",
        live_facts={"git": {}},
        file_records={},
        documentation={},
        errors=(failure,),
    )
    monkeypatch.setattr(agent_coordination, "agent_context", lambda _root: model)

    rc = agent_coordination.main(["--repo-root", str(tmp_path), "context", "--json"])
    payload = json.loads(capsys.readouterr().out)

    assert rc == 2
    assert payload["schema"] == agent_coordination.AGENT_CONTEXT_SCHEMA
    assert payload["ok"] is False
    assert payload["errors"] == [failure]
    assert set(payload) == {
        "schema",
        "generated_at_utc",
        "ok",
        "live_facts",
        "file_records",
        "documentation",
        "errors",
    }


def test_agent_context_documentation_reuses_instruction_authority(
    tmp_path: Path,
) -> None:
    for _role, relative in agent_coordination.AGENT_CONTEXT_DOCUMENTS:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("# Authority\n", encoding="utf-8")
    (tmp_path / "AGENTS.md").write_text(
        "# Agent contract\n\nSee `docs/INDEX.md`.\n",
        encoding="utf-8",
    )
    (tmp_path / "CLAUDE.md").write_text("@AGENTS.md\n", encoding="utf-8")
    for relative in agent_coordination.check_instruction_hierarchy.ARCHIVES:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            agent_coordination.check_instruction_hierarchy.ARCHIVE_MARKER + "\n",
            encoding="utf-8",
        )
    errors = []

    payload = agent_coordination._documentation_context(tmp_path, errors)

    assert errors == []
    assert payload["instruction_authority"]["canonical"] == "AGENTS.md"
    assert payload["instruction_authority"]["claude_adapter"] == "CLAUDE.md"
    assert payload["instruction_authority"]["audit"] == {
        "ok": True,
        "failures": [],
    }

    (tmp_path / "CLAUDE.md").write_text("@AGENTS.md\nextra\n", encoding="utf-8")
    errors = []
    payload = agent_coordination._documentation_context(tmp_path, errors)
    assert payload["instruction_authority"]["audit"]["ok"] is False
    assert errors[0]["kind"] == "authority_drift"


def test_proof_plan_recommends_focused_lanes_for_explicit_paths(tmp_path: Path) -> None:
    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            [
                "--repo-root",
                str(tmp_path),
                "proof-plan",
                "tools/agent_coordination.py",
                "tests/differential/basic/imported_generator_lowering.py",
                "runtime/molt-passes/src/tir/type_refine.rs",
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


def test_proof_plan_recommends_gpu_crate_lane(tmp_path: Path) -> None:
    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            [
                "--repo-root",
                str(tmp_path),
                "proof-plan",
                "runtime/molt-gpu/src/dtype.rs",
            ]
        )
    )

    assert payload["recommendations"] == [
        {
            "lane": "molt_gpu_targeted",
            "proof_role": "implementer",
            "shared_target_root": "target",
            "priority": "P1",
            "reason": "GPU compute/render primitive changes need focused crate-level Rust validation",
            "covered_paths": ["runtime/molt-gpu/src/dtype.rs"],
            "commands": ["cargo test -p molt-gpu"],
        }
    ]


def test_proof_plan_recommends_gpu_runtime_crate_lane(tmp_path: Path) -> None:
    payload = agent_coordination.proof_plan_payload(
        agent_coordination.parse_args(
            [
                "--repo-root",
                str(tmp_path),
                "proof-plan",
                "runtime/molt-gpu-runtime/src/bridge.rs",
            ]
        )
    )

    assert payload["recommendations"] == [
        {
            "lane": "molt_gpu_runtime_targeted",
            "proof_role": "implementer",
            "shared_target_root": "target",
            "priority": "P1",
            "reason": "GPU object-runtime integration changes need focused crate-level Rust validation",
            "covered_paths": ["runtime/molt-gpu-runtime/src/bridge.rs"],
            "commands": ["cargo test -p molt-gpu-runtime"],
        }
    ]


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


def test_codex_crash_classifies_responses_retry_control_c(
    tmp_path: Path,
) -> None:
    report = tmp_path / "logs" / "agents" / "codex_crash" / "retry.json"
    crash_text = (
        "An error has occurred\n"
        "Codex crashed with the following error:\n\n"
        "  (code=3221225786, signal=null).\n"
        'Most recent error: {"timestamp":"2026-06-30T16:56:31.824250Z",'
        '"level":"WARN","fields":{"message":"stream disconnected - retrying '
        'sampling request (1/5 in 206ms)..."},"target":'
        '"codex_core::responses_retry"}'
    )

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "codex-crash",
            "--out",
            str(report),
            "--codex-home",
            str(tmp_path / "codex-home"),
            "--runtime-cache-root",
            str(tmp_path / "runtime-cache"),
            "--crash-text",
            crash_text,
        ]
    )

    assert rc == 0
    report_text = report.read_text(encoding="utf-8")
    assert "An error has occurred" not in report_text
    payload = json.loads(report_text)
    assert payload["privacy"]["records_raw_crash_text"] is False
    assert "raw_crash_text" not in payload
    assert payload["parsed"]["code"] == 3221225786
    assert payload["parsed"]["signal"] == "null"
    assert payload["parsed"]["most_recent_error"]["target"] == (
        "codex_core::responses_retry"
    )
    assert {item["id"] for item in payload["classification"]} == {
        "windows_status_control_c_exit",
        "responses_retry_stream_disconnected",
    }
    assert payload["plugin_manifests"]["default_prompt_violation_count"] == 0


def test_codex_crash_reports_default_prompt_manifest_pressure(
    tmp_path: Path,
) -> None:
    codex_home = tmp_path / "codex-home"
    runtime_cache = tmp_path / "runtime-cache"
    manifest = (
        codex_home
        / "plugins"
        / "cache"
        / "bad-plugin"
        / ".codex-plugin"
        / "plugin.json"
    )
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        json.dumps(
            {
                "name": "bad-plugin",
                "interface": {
                    "defaultPrompt": [
                        "one",
                        "two",
                        "three",
                        "four",
                    ]
                },
            }
        ),
        encoding="utf-8",
    )
    report = tmp_path / "logs" / "agents" / "codex_crash" / "manifest.json"
    crash_error = {
        "timestamp": "2026-06-30T02:51:17.910404Z",
        "level": "WARN",
        "fields": {
            "message": "ignoring interface.defaultPrompt: maximum of 3 prompts is supported",
            "path": str(manifest),
        },
        "target": "codex_core_plugins::manifest",
    }
    crash_text = (
        "Codex crashed with the following error:\n"
        "  (code=3221225786, signal=null).\n"
        f"Most recent error: {json.dumps(crash_error)}"
    )

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "codex-crash",
            "--out",
            str(report),
            "--codex-home",
            str(codex_home),
            "--runtime-cache-root",
            str(runtime_cache),
            "--crash-text",
            crash_text,
        ]
    )

    assert rc == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert {item["id"] for item in payload["classification"]} == {
        "windows_status_control_c_exit",
        "plugin_default_prompt_manifest_warning",
    }
    assert payload["parsed"]["most_recent_error"]["path"] == str(manifest)
    assert payload["plugin_manifests"]["manifest_count_scanned"] == 1
    assert payload["plugin_manifests"]["default_prompt_violation_count"] == 1
    assert payload["plugin_manifests"]["default_prompt_violations"] == [
        {
            "limit": 3,
            "path": str(manifest),
            "prompt_count": 4,
        }
    ]


def test_codex_crash_classifies_project_doc_budget_pressure(
    tmp_path: Path,
) -> None:
    report = tmp_path / "logs" / "agents" / "codex_crash" / "projectdoc.json"

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "codex-crash",
            "--out",
            str(report),
            "--codex-home",
            str(tmp_path / "codex-home"),
            "--runtime-cache-root",
            str(tmp_path / "runtime-cache"),
            "--crash-text",
            "projectdoc exceeds remaining budget",
        ]
    )

    assert rc == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["privacy"]["records_raw_crash_text"] is False
    assert payload["parsed"]["markers"] == ["projectdoc_exceeds_remaining_budget"]
    assert {item["id"] for item in payload["classification"]} == {
        "projectdoc_remaining_budget_exhausted"
    }
    assert any(
        "tests/test_agent_contract_budget.py" in action
        for action in payload["next_actions"]
    )


def test_codex_crash_classifies_unsupported_exec_interrupt(
    tmp_path: Path,
) -> None:
    report = tmp_path / "logs" / "agents" / "codex_crash" / "interrupt.json"
    crash_error = {
        "timestamp": "2026-07-01T17:04:26.840097Z",
        "level": "ERROR",
        "fields": {
            "error": (
                "write_stdin failed: Unified exec process failed: "
                "process interrupt is not supported by this process backend"
            )
        },
        "target": "codex_core::tools::router",
    }

    rc = agent_coordination.main(
        [
            "--repo-root",
            str(tmp_path),
            "codex-crash",
            "--out",
            str(report),
            "--codex-home",
            str(tmp_path / "codex-home"),
            "--runtime-cache-root",
            str(tmp_path / "runtime-cache"),
            "--crash-text",
            (
                "Codex crashed with the following error:\n"
                "  (code=3221225786, signal=null).\n"
                f"Most recent error: {json.dumps(crash_error)}"
            ),
        ]
    )

    assert rc == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert (
        payload["parsed"]["most_recent_error"]["error"]
        == (crash_error["fields"]["error"])
    )
    assert "exec_backend_interrupt_unsupported" in payload["parsed"]["markers"]
    assert {item["id"] for item in payload["classification"]} == {
        "windows_status_control_c_exit",
        "exec_backend_interrupt_unsupported",
    }
    assert any(
        "proof_queue prune-stale" in action for action in payload["next_actions"]
    )
