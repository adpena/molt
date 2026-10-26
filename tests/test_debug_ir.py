from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[1]


def _base_env() -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _python_executable() -> str:
    exe = sys.executable
    if exe and os.path.exists(exe) and os.access(exe, os.X_OK):
        return exe
    fallback = shutil.which("python3") or shutil.which("python")
    if fallback:
        return fallback
    return exe


def _run_cli(args: list[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return run_cli_test_process(
        [_python_executable(), "-m", "molt.cli", *args],
        cwd=cwd,
        env=_base_env(),
        capture_output=True,
        text=True,
        check=False,
    )


def _write_source(tmp_path: Path) -> Path:
    source_path = tmp_path / "sample_ir_input.py"
    source_path.write_text(
        textwrap.dedent(
            """
            def selected(value):
                99
                value + 1
                return 7

            def other(value):
                return value * 2

            print(selected(other(3)))
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    return source_path


def _load_manifest_from_stdout(stdout: str) -> dict[str, object]:
    payload = json.loads(stdout)
    manifest_path = Path(payload["manifest_path"])
    assert manifest_path.is_file()
    return json.loads(manifest_path.read_text(encoding="utf-8"))


def test_debug_ir_json_supports_pre_post_and_all_stages(tmp_path: Path) -> None:
    source_path = _write_source(tmp_path)

    expected_stage_sets = {
        "pre-midend": ["pre-midend"],
        "post-midend": ["post-midend"],
        "all": ["pre-midend", "post-midend"],
    }

    for stage, expected_stages in expected_stage_sets.items():
        res = _run_cli(
            [
                "debug",
                "ir",
                str(source_path),
                "--stage",
                stage,
                "--format",
                "json",
            ],
            cwd=tmp_path,
        )
        assert res.returncode == 0, res.stderr
        payload = json.loads(res.stdout)
        assert payload["subcommand"] == "ir"
        assert payload["status"] == "ok"
        assert payload["selectors"]["stage"] == stage
        assert [
            entry["stage"] for entry in payload["data"]["snapshots"]
        ] == expected_stages

        for snapshot in payload["data"]["snapshots"]:
            [selected] = [
                function
                for function in snapshot["functions"]
                if function["name"].split("__")[-1] == "selected"
            ]
            ops = selected["ops"]
            # Only the unused literal is removable: value.__add__ may raise.
            assert sum(op["kind"] == "const" and op["value"] == 99 for op in ops) == (
                1 if snapshot["stage"] == "pre-midend" else 0
            )
            [parameter] = [
                op
                for op in ops
                if op["kind"] == "frame_home_store" and op["args"] == ["value"]
            ]
            [one] = [op for op in ops if op["kind"] == "const" and op["value"] == 1]
            [seven] = [op for op in ops if op["kind"] == "const" and op["value"] == 7]
            [addition] = [op for op in ops if op["kind"] == "add"]
            [normal_return] = [op for op in ops if op["kind"] == "ret"]
            assert addition["args"] == [parameter["out"], one["out"]]
            assert normal_return["args"] == [seven["out"]]
            add_index, return_index = ops.index(addition), ops.index(normal_return)
            [exception_check] = [
                op
                for op in ops[add_index + 1 : return_index]
                if op["kind"] == "check_exception"
            ]
            assert (
                add_index < ops.index(exception_check) < ops.index(seven) < return_index
            )
            failure_label = next(
                index
                for index, op in enumerate(ops)
                if op["kind"] == "label" and op["value"] == exception_check["value"]
            )
            assert failure_label > return_index
            assert [
                op["kind"] for op in ops[failure_label + 1 : failure_label + 3]
            ] == [
                "trace_exit",
                "ret_void",
            ]

        manifest_payload = _load_manifest_from_stdout(res.stdout)
        assert manifest_payload["data"]["snapshots"] == payload["data"]["snapshots"]


def test_debug_ir_text_default_and_function_filter_emit_only_selected_function(
    tmp_path: Path,
) -> None:
    source_path = _write_source(tmp_path)

    res = _run_cli(
        [
            "debug",
            "ir",
            str(source_path),
            "--stage",
            "all",
            "--function",
            "selected",
        ],
        cwd=tmp_path,
    )

    assert res.returncode == 0, res.stderr
    assert "--- Stage: pre-midend ---" in res.stdout
    assert "--- Stage: post-midend ---" in res.stdout
    assert "Function: selected" in res.stdout
    assert "Function: other" not in res.stdout
    assert "Function: molt_main" not in res.stdout
