from __future__ import annotations

import hashlib
import json
import os
import textwrap
from pathlib import Path
import subprocess

import pytest

import tools.secret_guard as secret_guard


def test_secret_guard_detects_high_confidence_token() -> None:
    token_line = "+token=" + "verylongsecurevalueabcdefghijklmno"
    diff_text = textwrap.dedent(
        """\
        diff --git a/example.txt b/example.txt
        index 1111111..2222222 100644
        --- a/example.txt
        +++ b/example.txt
        @@ -0,0 +1 @@
        {token_line}
        """
    ).format(token_line=token_line)
    findings = secret_guard.scan_diff_text(diff_text)
    assert findings
    assert any(f.reason == "Sensitive assignment value" for f in findings)


def test_secret_guard_ignores_allow_marker() -> None:
    diff_text = textwrap.dedent(
        """\
        diff --git a/example.txt b/example.txt
        index 1111111..2222222 100644
        --- a/example.txt
        +++ b/example.txt
        @@ -0,0 +1 @@
        +LINEAR_API_KEY=lin_api_ABCDEFGHIJKLMNOPQRSTUVWX  # secret-guard: allow
        """
    )
    findings = secret_guard.scan_diff_text(diff_text)
    assert findings == []


def test_secret_guard_ignores_placeholder_values() -> None:
    diff_text = textwrap.dedent(
        """\
        diff --git a/example.txt b/example.txt
        index 1111111..2222222 100644
        --- a/example.txt
        +++ b/example.txt
        @@ -0,0 +1 @@
        +API_TOKEN=replace-with-token
        """
    )
    findings = secret_guard.scan_diff_text(diff_text)
    assert findings == []


def test_secret_guard_writes_security_event_on_block(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    linear_token = "lin_api_" + "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456"
    events_file = tmp_path / "events.jsonl"
    monkeypatch.setenv("MOLT_SECURITY_EVENTS_FILE", str(events_file))
    diff_file = tmp_path / "diff.patch"
    diff_file.write_text(
        textwrap.dedent(
            """\
            diff --git a/example.txt b/example.txt
            index 1111111..2222222 100644
            --- a/example.txt
            +++ b/example.txt
            @@ -0,0 +1 @@
            +LINEAR_API_KEY={linear_token}
            """
        ).format(linear_token=linear_token),
        encoding="utf-8",
    )
    rc = secret_guard.main(["--diff-file", str(diff_file)])
    assert rc == 1
    assert events_file.exists()
    content = events_file.read_text(encoding="utf-8")
    assert "secret_guard_blocked" in content
    output = capsys.readouterr()
    assert '"example.txt":1 [Linear API key]' in output.err
    assert linear_token not in output.err + output.out + content


def test_staged_producer_fixes_the_diff_protocol(monkeypatch) -> None:
    commands = []

    def git_output(command):
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, stdout="", stderr="")

    monkeypatch.setattr(secret_guard, "_run", git_output)
    assert secret_guard.main(["--staged"]) == 0
    assert commands == [
        [
            "git",
            "diff",
            "--cached",
            "--no-color",
            "--unified=0",
            "--no-ext-diff",
            "--no-textconv",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ]
    ]


@pytest.mark.parametrize("nested", [False, True])
@pytest.mark.parametrize("payload", ["assignment", "cr-assignment", "cr-provider"])
def test_staged_scan_uses_entire_selected_index_from_any_cwd(
    tmp_path: Path, monkeypatch, capsys, nested: bool, payload: str
) -> None:
    for key in tuple(os.environ):
        if key.startswith("GIT_"):
            monkeypatch.delenv(key)
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", os.devnull)
    repo = tmp_path / "repo"
    repo.mkdir()

    def git(*args: str) -> bytes:
        return subprocess.run(
            ["git", *args], cwd=repo, check=True, capture_output=True, timeout=10
        ).stdout

    git("init", "--quiet")
    git("config", "--local", "diff.relative", "true")
    fixture = "Q7m9R2v5K8n4J6p3" * 2
    contents, reason = {
        "assignment": ("token=" + fixture, "Sensitive assignment value"),
        "cr-assignment": ("SAFE=1\rpassword=" + fixture, "Sensitive assignment value"),
        "cr-provider": ("SAFE=1\r" + "ghp_" + fixture, "GitHub token"),
    }[payload]
    staged = (contents + "\n").encode()
    target = repo / "outside.env"
    target.write_bytes(staged)
    git("add", "--", "outside.env")
    assert git("show", ":outside.env") == staged
    target.write_text("SAFE=1\n", encoding="utf-8")
    index = repo / ".git" / "index"
    before = hashlib.sha256(index.read_bytes()).digest()
    events = tmp_path / "events.jsonl"
    monkeypatch.setenv("MOLT_SECURITY_EVENTS_FILE", str(events))
    cwd = repo / "nested" if nested else repo
    cwd.mkdir(exist_ok=True)
    monkeypatch.chdir(cwd)

    assert secret_guard.main(["--staged"]) == 1
    captured = capsys.readouterr()
    assert fixture not in captured.out + captured.err + events.read_text(
        encoding="utf-8"
    )
    assert f'"outside.env":1 [{reason}]' in captured.err
    event = json.loads(events.read_text(encoding="utf-8"))
    assert event["kind"] == "secret_guard_blocked"
    assert event["finding_count"] == 1
    assert event["paths"] == ["outside.env"]
    assert hashlib.sha256(index.read_bytes()).digest() == before
    assert target.read_text(encoding="utf-8") == "SAFE=1\n"


@pytest.mark.parametrize("provider", [False, True])
def test_diff_file_preserves_carriage_return_inside_added_line(
    tmp_path: Path, monkeypatch, capsys, provider: bool
) -> None:
    fixture = "Q7m9R2v5K8n4J6p3" * 2
    payload = ("ghp_" if provider else "password=") + fixture
    patch = tmp_path / "input.patch"
    patch.write_bytes(
        (
            "--- /dev/null\n+++ b/credentials.env\n@@ -0,0 +1 @@\n"
            "+SAFE=1\r" + payload + "\n"
        ).encode()
    )
    events = tmp_path / "events.jsonl"
    monkeypatch.setenv("MOLT_SECURITY_EVENTS_FILE", str(events))
    assert secret_guard.main(["--diff-file", str(patch)]) == 1
    captured = capsys.readouterr()
    assert fixture not in captured.out + captured.err + events.read_text(
        encoding="utf-8"
    )
    reason = "GitHub token" if provider else "Sensitive assignment value"
    assert f'"credentials.env":1 [{reason}]' in captured.err
    assert json.loads(events.read_text(encoding="utf-8"))["finding_count"] == 1


@pytest.mark.parametrize(
    "suffix",
    [
        "+extra\n",
        "-extra\n",
        " extra\n",
        "+++ b/extra.env\n",
        "--- a/extra.env\n",
        "@@broken\n",
        "@@\t-0,0 +2 @@\n",
        "@@ -broken +2 @@\n",
    ],
)
def test_diff_file_rejects_payload_outside_declared_hunk(
    tmp_path: Path, suffix: str
) -> None:
    patch = tmp_path / "input.patch"
    patch.write_text(
        "--- /dev/null\n+++ b/credentials.env\n@@ -0,0 +1 @@\n+SAFE=1\n" + suffix,
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="unified diff"):
        secret_guard.main(["--diff-file", str(patch)])
