from __future__ import annotations

import json
from pathlib import Path
import shlex

import pytest

from molt import dx
from tools import hosted_ci_env

ROOT = Path(__file__).resolve().parents[2]


def _parse_posix(text: str) -> dict[str, str]:
    parsed: dict[str, str] = {}
    for line in text.splitlines():
        words = shlex.split(line)
        assert words[0] == "export", line
        key, value = words[1].split("=", 1)
        parsed[key] = value
    return parsed


def test_emitted_contract_is_hosted_custody_for_this_checkout(tmp_path: Path) -> None:
    runner_temp = tmp_path / "runner"
    env = hosted_ci_env.hosted_job_env(
        ROOT, runner_temp, sha=dx.git_checkout_head(ROOT) or ""
    )

    custody = dx.checkout_custody(ROOT, env)

    assert custody.kind == "github-actions-ephemeral"
    assert custody.source_root == ROOT.resolve()
    assert custody.custody_root == (runner_temp / "molt-custody").resolve()


def test_cli_prints_posix_exports_that_a_shell_reads_back(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    runner_temp = tmp_path / "runner with space"

    assert (
        hosted_ci_env.main(["--runner-temp", str(runner_temp), "--no-resource-plan"])
        == 0
    )

    exported = _parse_posix(capsys.readouterr().out)
    assert exported["GITHUB_WORKSPACE"] == str(ROOT.resolve())
    assert exported["RUNNER_TEMP"] == str(runner_temp.resolve())
    assert dx.checkout_custody(ROOT, exported).kind == "github-actions-ephemeral"
    assert "MOLT_MAX_PROCESS_RSS_GB" not in exported


def test_cli_adds_the_setup_action_resource_plan(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    assert hosted_ci_env.main(["--runner-temp", str(tmp_path / "runner")]) == 0

    exported = _parse_posix(capsys.readouterr().out)
    assert int(exported["CARGO_BUILD_JOBS"]) >= 1
    assert int(exported["PYTEST_XDIST_AUTO_NUM_WORKERS"]) >= 1
    assert isinstance(json.loads(exported["MOLT_CI_RESOURCE_PLAN_JSON"]), dict)


def test_cli_refuses_a_checkout_that_molt_dx_rejects(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # No .github/workflows/ci.yml: the hosted contract cannot name the
    # workflow, so the emulation must fail instead of printing it.
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    monkeypatch.setattr(dx, "git_checkout_head", lambda _root: "c" * 40)

    status = hosted_ci_env.main(
        ["--checkout", str(checkout), "--runner-temp", str(tmp_path / "runner")]
    )

    captured = capsys.readouterr()
    assert status == 2
    assert captured.out == ""
    assert "molt.dx rejects the emulated job" in captured.err


def test_powershell_quoting_doubles_single_quotes() -> None:
    rendered = hosted_ci_env.render({"RUNNER_TEMP": "C:\\it's"}, "powershell")

    assert rendered == "$env:RUNNER_TEMP = 'C:\\it''s'\n"
