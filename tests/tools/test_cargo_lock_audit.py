"""The Cargo security audit covers every tracked workspace with one shared deny.toml."""

from __future__ import annotations

import json
from pathlib import PurePosixPath
import subprocess

from tools import cargo_lock_audit as audit
from tools.dependency_manifests import cargo_workspace_dirs

ROOT_WS = PurePosixPath(".")
FUZZ_WS = PurePosixPath("fuzz")


def _diagnostic(code: str, span: str, severity: str = "warning") -> str:
    return json.dumps(
        {
            "type": "diagnostic",
            "fields": {
                "code": code,
                "severity": severity,
                "message": f"{code} message",
                "labels": [{"span": span}],
            },
        }
    )


class FakeCargo:
    """Answers each workspace's command with canned diagnostics and records it."""

    def __init__(self, replies: dict[str, tuple[int, list[str]]]) -> None:
        self.replies = replies
        self.commands: list[list[str]] = []

    def __call__(
        self, command: list[str], **_: object
    ) -> subprocess.CompletedProcess[str]:
        self.commands.append(command)
        target = (
            command[command.index("--manifest-path") + 1]
            if "--manifest-path" in command
            else command[command.index("--file") + 1]
        )
        returncode, lines = (
            (0, [])
            if "fetch" in command
            else self.replies[str(PurePosixPath(target).parent)]
        )
        return subprocess.CompletedProcess(
            command, returncode, stdout="", stderr="\n".join(lines)
        )


def test_a_shared_entry_unused_in_every_workspace_fails(capsys) -> None:
    fake = FakeCargo(
        {
            ".": (
                0,
                [
                    _diagnostic("advisory-not-detected", "RUSTSEC-0000-0001"),
                    _diagnostic(
                        "unmatched-source", "https://example.invalid/used-in-fuzz"
                    ),
                ],
            ),
            "fuzz": (
                0,
                [
                    _diagnostic("advisory-not-detected", "RUSTSEC-0000-0001"),
                    _diagnostic("advisory-not-detected", "RUSTSEC-0000-0002"),
                ],
            ),
        }
    )
    assert audit.run_deny([ROOT_WS, FUZZ_WS], run=fake) == 1
    out = capsys.readouterr().out
    assert (
        "matches no tracked workspace (advisory-not-detected): RUSTSEC-0000-0001" in out
    )
    # Each of these is used by one workspace, so it is not stale.
    assert "RUSTSEC-0000-0002" not in out
    assert "used-in-fuzz" not in out


def test_entries_used_somewhere_pass(capsys) -> None:
    fake = FakeCargo(
        {
            ".": (0, [_diagnostic("advisory-not-detected", "RUSTSEC-0000-0002")]),
            "fuzz": (
                0,
                [_diagnostic("unmatched-source", "https://example.invalid/root-only")],
            ),
        }
    )
    assert audit.run_deny([ROOT_WS, FUZZ_WS], run=fake) == 0
    assert "matches no tracked workspace" not in capsys.readouterr().out


def test_an_error_in_any_workspace_fails(capsys) -> None:
    fake = FakeCargo(
        {
            ".": (0, []),
            "fuzz": (
                1,
                [_diagnostic("vulnerability", "rustls 0.23.45", severity="error")],
            ),
        }
    )
    assert audit.run_deny([ROOT_WS, FUZZ_WS], run=fake) == 1
    out = capsys.readouterr().out
    assert "cargo deny fuzz: FAILED (exit 1)" in out
    assert "vulnerability: vulnerability message" in out


def test_each_run_fetches_its_advisory_database_once() -> None:
    deny = FakeCargo({".": (0, []), "fuzz": (0, [])})
    audit.run_deny([ROOT_WS, FUZZ_WS], run=deny)
    assert [command[-2:] for command in deny.commands] == [
        ["fetch", "all"],
        ["fetch", "index"],
        ["--offline", "check"],
        ["--offline", "check"],
    ]
    # Root options must precede the subcommand in cargo-deny 0.20. Every
    # workspace is prepared and checked at its committed lockfile.
    for command in deny.commands:
        boundary = command.index("fetch" if "fetch" in command else "check")
        assert command.index("--config") < boundary
        assert command.index("--locked") < boundary
        assert "--disable-fetch" not in command
    assert [
        command[command.index("--manifest-path") + 1] for command in deny.commands
    ] == ["Cargo.toml", "fuzz/Cargo.toml", "Cargo.toml", "fuzz/Cargo.toml"]

    cargo_audit = FakeCargo({".": (0, []), "fuzz": (0, [])})
    assert audit.run_audit([ROOT_WS, FUZZ_WS], "db", run=cargo_audit) == 0
    assert ["--no-fetch" in command for command in cargo_audit.commands] == [
        False,
        True,
    ]
    assert [
        command[command.index("--file") + 1] for command in cargo_audit.commands
    ] == [
        "Cargo.lock",
        "fuzz/Cargo.lock",
    ]


def test_every_tracked_cargo_lockfile_is_a_workspace() -> None:
    workspaces = cargo_workspace_dirs(audit.ROOT)
    assert ROOT_WS in workspaces
    assert all(
        (audit.ROOT / workspace / "Cargo.lock").is_file() for workspace in workspaces
    )
    assert not any(
        str(workspace).startswith("tests/fixtures") for workspace in workspaces
    )


def test_failed_fetch_reports_plain_cli_error_and_runs_no_checks(capsys) -> None:
    commands: list[list[str]] = []

    def failed_fetch(command: list[str], *, capture: bool):
        assert capture
        commands.append(command)
        return subprocess.CompletedProcess(
            command,
            2,
            stdout="",
            stderr="error: unexpected argument '--config' found\nUsage: cargo deny check\n",
        )

    assert audit.run_deny([ROOT_WS, FUZZ_WS], run=failed_fetch) == 1
    assert len(commands) == 1
    assert commands[0][-2:] == ["fetch", "all"]
    output = capsys.readouterr().out
    assert "unexpected argument '--config'" in output
    assert "Usage: cargo deny check" in output


def test_failed_secondary_fetch_never_starts_offline_checks(capsys) -> None:
    commands: list[list[str]] = []

    def fetch(command: list[str], *, capture: bool):
        assert capture
        commands.append(command)
        failed = len(commands) == 2
        return subprocess.CompletedProcess(
            command,
            int(failed),
            stdout="",
            stderr=json.dumps(
                {
                    "type": "log",
                    "fields": {"level": "ERROR", "message": "crate fetch refused"},
                }
            )
            if failed
            else "",
        )

    assert audit.run_deny([ROOT_WS, FUZZ_WS], run=fetch) == 1
    assert len(commands) == 2
    assert all("check" not in command for command in commands)
    assert "crate fetch refused" in capsys.readouterr().out


def test_error_streams_without_separator_keep_both_failures(capsys) -> None:
    def cargo(command: list[str], *, capture: bool):
        assert capture
        if "fetch" in command:
            return subprocess.CompletedProcess(command, 0, stdout="", stderr="")
        return subprocess.CompletedProcess(
            command,
            1,
            stdout=_diagnostic("vulnerability", "crate", severity="error"),
            stderr=json.dumps(
                {
                    "type": "log",
                    "fields": {"level": "ERROR", "message": "metadata failed"},
                }
            ),
        )

    assert audit.run_deny([ROOT_WS], run=cargo) == 1
    output = capsys.readouterr().out
    assert "vulnerability: vulnerability message" in output
    assert "metadata failed" in output


def test_advisory_identity_and_solution_reach_the_existing_report(capsys) -> None:
    diagnostic = json.loads(
        _diagnostic("vulnerability", "affected-crate", severity="error")
    )
    notes = [
        "ID: RUSTSEC-2099-0001",
        "Advisory: https://rustsec.org/advisories/RUSTSEC-2099-0001",
        "Solution: upgrade to >= 1.2.3",
    ]
    diagnostic["fields"]["notes"] = notes
    fake = FakeCargo({".": (1, [json.dumps(diagnostic)])})

    assert audit.run_deny([ROOT_WS], run=fake) == 1

    output = capsys.readouterr().out
    for note in notes:
        assert note in output
