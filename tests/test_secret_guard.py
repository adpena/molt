from __future__ import annotations

import tools.secret_guard as secret_guard


def test_iter_added_lines_tracks_paths_and_line_numbers() -> None:
    diff = """diff --git a/foo.txt b/foo.txt
index 1111111..2222222 100644
--- a/foo.txt
+++ b/foo.txt
@@ -1,0 +1,2 @@
+first
+second
"""
    added = secret_guard.iter_added_lines(diff)
    assert [item.path for item in added] == ["foo.txt", "foo.txt"]
    assert [item.line_no for item in added] == [1, 2]
    assert [item.text for item in added] == ["first", "second"]


def test_scan_diff_detects_linear_api_key() -> None:
    token = "lin_api_" + "GQHZxAFeCj9ZfBCeYAZzGBxn3YM6kliCLx6HuGwA"
    diff = """diff --git a/ops/linear/runtime/symphony.env b/ops/linear/runtime/symphony.env
index 1111111..2222222 100644
--- a/ops/linear/runtime/symphony.env
+++ b/ops/linear/runtime/symphony.env
@@ -1,0 +1 @@
+LINEAR_API_KEY={token}
""".format(token=token)
    findings = secret_guard.scan_diff_text(diff)
    assert findings
    assert findings[0].reason in {"Linear API key", "Sensitive assignment value"}


def test_scan_diff_ignores_placeholder_values_and_allow_marker() -> None:
    diff = """diff --git a/ops/linear/runtime/symphony.env.example b/ops/linear/runtime/symphony.env.example
index 1111111..2222222 100644
--- a/ops/linear/runtime/symphony.env.example
+++ b/ops/linear/runtime/symphony.env.example
@@ -1,0 +1,2 @@
+LINEAR_API_KEY=<your-token>
+AUTH_TOKEN=not-real-fixture # secret-guard: allow
"""
    assert secret_guard.scan_diff_text(diff) == []


def test_scan_diff_detects_private_key_block() -> None:
    key_header = "-----BEGIN " + "PRIVATE KEY-----"
    diff = """diff --git a/certs/key.pem b/certs/key.pem
index 1111111..2222222 100644
--- a/certs/key.pem
+++ b/certs/key.pem
@@ -1,0 +1 @@
+{key_header}
""".format(key_header=key_header)
    findings = secret_guard.scan_diff_text(diff)
    assert any(item.reason == "Private key material" for item in findings)
