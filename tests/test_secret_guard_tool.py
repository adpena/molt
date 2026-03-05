from __future__ import annotations

import textwrap

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
    ).format(
        token_line=token_line
    )
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
