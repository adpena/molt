from __future__ import annotations

import argparse
import json
from pathlib import Path

import tools.symphony_recursive_loop as recursive_loop


def test_load_next_tranche_commands(tmp_path: Path) -> None:
    payload = {
        "actions": [
            {"commands": ["echo hello", "  uv run thing  "]},
            {"commands": ["", "echo bye"]},
            {"not_commands": []},
        ]
    }
    path = tmp_path / "next_tranche.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    assert recursive_loop._load_next_tranche_commands(path) == [
        "echo hello",
        "uv run thing",
        "echo bye",
    ]


def test_env_with_external_defaults_respects_existing(tmp_path: Path) -> None:
    ext_root = tmp_path / "ext"
    env = {"CARGO_TARGET_DIR": "/custom/target", "PYTHONPATH": "already-set"}
    out = recursive_loop._env_with_external_defaults(env, ext_root)
    assert out["CARGO_TARGET_DIR"] == "/custom/target"
    assert out["PYTHONPATH"] == "already-set"
    assert out["MOLT_EXT_ROOT"] == str(ext_root)
    assert out["MOLT_DIFF_CARGO_TARGET_DIR"] == "/custom/target"


def test_load_env_file_parses_assignments(tmp_path: Path) -> None:
    env_file = tmp_path / "loop.env"
    env_file.write_text(
        "# comment\nLINEAR_API_KEY=lin_abc\nMOLT_SYMPHONY_DSPY_ENABLE='1'\n",
        encoding="utf-8",
    )
    loaded = recursive_loop._load_env_file(env_file)
    assert loaded["LINEAR_API_KEY"] == "lin_abc"
    assert loaded["MOLT_SYMPHONY_DSPY_ENABLE"] == "1"


def test_render_summary_markdown_includes_next_tranche_section(tmp_path: Path) -> None:
    cycle_dir = tmp_path / "cycle"
    step = recursive_loop.StepResult(
        name="readiness_audit",
        command=["cmd"],
        returncode=0,
        duration_seconds=1.2,
        stdout_path="out.log",
        stderr_path="err.log",
    )
    tranche_step = recursive_loop.StepResult(
        name="next_tranche_01",
        command=["echo x"],
        returncode=1,
        duration_seconds=0.4,
        stdout_path="t-out.log",
        stderr_path="t-err.log",
    )
    md = recursive_loop._render_summary_markdown(
        started_at="2026-03-05T00:00:00Z",
        finished_at="2026-03-05T00:00:02Z",
        status="fail",
        steps=[step],
        executed_commands=[tranche_step],
        cycle_dir=cycle_dir,
    )
    assert "Symphony Recursive Loop" in md
    assert "Executed Next-Tranche Commands" in md
    assert "`next_tranche_01`" in md


def test_build_readiness_command_respects_strict_flag(tmp_path: Path) -> None:
    args = argparse.Namespace(
        team="Moltlang",
        formal_suite="all",
        strict_autonomy=False,
        fail_on="warn",
    )
    command = recursive_loop._build_readiness_command(
        args=args,
        readiness_json=tmp_path / "r.json",
        readiness_md=tmp_path / "r.md",
        next_tranche_json=tmp_path / "n.json",
        next_tranche_md=tmp_path / "n.md",
    )
    assert "--strict-autonomy" not in command
    assert "--fail-on" not in command


def test_failure_codes_from_readiness_extracts_warn_and_fail(tmp_path: Path) -> None:
    report = {
        "findings": [
            {"severity": "info", "code": "human_gate_present"},
            {"severity": "warn", "code": "formal_pass_ratio_low"},
            {"severity": "fail", "code": "symphony_storage_layout_invalid"},
        ]
    }
    path = tmp_path / "readiness.json"
    path.write_text(json.dumps(report), encoding="utf-8")
    assert recursive_loop._failure_codes_from_readiness(path) == [
        "formal_pass_ratio_low",
        "symphony_storage_layout_invalid",
    ]
