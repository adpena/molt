from __future__ import annotations

import json

import pytest

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
    token = "lin_api_" + "ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890ABCD"
    diff = """diff --git a/ops/linear/runtime/local.env b/ops/linear/runtime/local.env
index 1111111..2222222 100644
--- a/ops/linear/runtime/local.env
+++ b/ops/linear/runtime/local.env
@@ -1,0 +1 @@
+LINEAR_API_KEY={token}
""".format(token=token)
    findings = secret_guard.scan_diff_text(diff)
    assert findings
    assert findings[0].reason in {"Linear API key", "Sensitive assignment value"}


def test_scan_diff_ignores_placeholder_values_and_allow_marker() -> None:
    diff = """diff --git a/ops/linear/runtime/local.env.example b/ops/linear/runtime/local.env.example
index 1111111..2222222 100644
--- a/ops/linear/runtime/local.env.example
+++ b/ops/linear/runtime/local.env.example
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


def test_scan_diff_skips_allowlisted_vendor_prefix() -> None:
    diff = """diff --git a/vendor/rustpython-parser/src/python.lalrpop b/vendor/rustpython-parser/src/python.lalrpop
index 1111111..2222222 100644
--- a/vendor/rustpython-parser/src/python.lalrpop
+++ b/vendor/rustpython-parser/src/python.lalrpop
@@ -1,0 +1,1 @@
+StartInteractive => token::Tok::StartInteractive, # secret-guard: allow
"""
    assert secret_guard.scan_diff_text(diff) == []


def _added(path: str, *lines: str) -> str:
    return (
        f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
        f"@@ -0,0 +1,{len(lines)} @@\n" + "".join("+" + line + "\n" for line in lines)
    )


def _credential() -> str:
    # Synthetic fixed material. Never fetch credentials or query a provider.
    return "Q7m9R2v5K8n4J6p3" * 2


@pytest.mark.parametrize(
    ("path", "expression"),
    [
        ("policy.py", "token = normalized[span.start]"),
        ("policy.py", "secret = make_credential_value()"),
        ("policy.py", "password = credentials.password_for(account)"),
        ("policy.py", "token = config.AUTH_TOKEN_FROM_RUNTIME"),
        ("policy.py", "token = f'{runtime_only_value}'"),
        ("policy.py", "token == some_runtime_variable"),
        ("policy.rs", "let token = selected.argument_value();"),
        (
            "policy.rs",
            "let token = context::set_variable(&py, key, value).unwrap();",
        ),
        (
            "policy.rs",
            "let token = molt_cpython_abi::api::object::PyObject_CallNoArgs(shells[2]);",
        ),
        ("policy.ts", "const token = process.env.AUTH_TOKEN;"),
    ],
)
def test_assignments_distinguish_source_expressions_from_literal_data(
    path: str, expression: str
) -> None:
    assert secret_guard.scan_diff_text(_added(path, expression)) == []
    literal = "token = " + json.dumps(_credential())
    findings = secret_guard.scan_diff_text(_added(path, literal))
    assert [(item.path, item.line_no, item.reason) for item in findings] == [
        (path, 1, "Sensitive assignment value")
    ]


def test_split_rust_call_does_not_become_credential_data() -> None:
    diff = _added(
        "context.rs",
        "let token =",
        "    molt_cpython_abi::api::object::PyObject_CallNoArgs(shells[2]);",
    )
    assert secret_guard.scan_diff_text(diff) == []


@pytest.mark.parametrize(
    "path",
    [
        "src/molt/_intrinsic_symbols.py",
        "src/molt/_wasm_abi_generated.py",
        "wasm/wasm_abi_generated.json",
    ],
)
@pytest.mark.parametrize("direction", ["identity", "forward", "inverse"])
def test_public_symbol_projection_preserves_real_credential_detection(
    path: str, direction: str
) -> None:
    name = "contextvars_token_new"
    symbol = "molt_" + name
    key, value = {
        "identity": (symbol, symbol),
        "forward": (name, symbol),
        "inverse": (symbol, name),
    }[direction]
    entry = "    " + json.dumps(key) + ": " + json.dumps(value) + ","
    assert secret_guard.scan_diff_text(_added(path, entry)) == []
    # The same spelling in ordinary configuration is not a public projection.
    assert secret_guard.scan_diff_text(_added("settings.json", entry))
    for credential in (
        _credential(),
        "molt_" + _credential(),
        value + "_" + _credential(),
        _credential() + "_" + value,
        _credential()[:16] + "(::)" + _credential()[16:],
    ):
        unrelated = "    " + json.dumps(key) + ": " + json.dumps(credential) + ","
        findings = secret_guard.scan_diff_text(_added(path, unrelated))
        assert [item.reason for item in findings] == ["Sensitive assignment value"]
    # A same-spelling assignment or expression is not the complete map entry.
    for expression in (
        json.dumps(key) + " = " + json.dumps(value),
        entry[:-1] + " + " + json.dumps(_credential()),
        entry + ' "password": ' + json.dumps(_credential()),
    ):
        assert secret_guard.scan_diff_text(_added(path, expression))


@pytest.mark.parametrize(
    "path",
    [
        "src/molt/_intrinsic_symbols.py",
        "src/molt/_wasm_abi_generated.py",
        "wasm/wasm_abi_generated.json",
    ],
)
def test_public_symbol_projection_keeps_provider_patterns(path: str) -> None:
    # Even an exact identity row cannot suppress independent provider detection.
    value = "ghp" + "_" + _credential()
    entry = json.dumps(value) + ": " + json.dumps(value) + ","
    findings = secret_guard.scan_diff_text(_added(path, entry))
    assert "GitHub token" in {item.reason for item in findings}


@pytest.mark.parametrize("path", ["settings.rs", ".env", "settings.yaml"])
@pytest.mark.parametrize("quoted", [False, True])
def test_punctuation_remains_credential_data_by_carrier(
    path: str, quoted: bool
) -> None:
    # Punctuation is data in literals/config, not a global expression exemption.
    value = _credential()[:16] + "(::)" + _credential()[16:]
    rhs = json.dumps(value) if quoted else value
    findings = secret_guard.scan_diff_text(_added(path, "password=" + rhs))
    expected = ["Sensitive assignment value"] if quoted or path != "settings.rs" else []
    assert [item.reason for item in findings] == expected


@pytest.mark.parametrize(
    "path",
    [
        ".env",
        ".env.production",
        "runtime.env.example",
        "auth.txt",
        "settings.ini",
        "settings.toml",
        "settings.yaml",
        "deploy.sh",
        "credentials",
        "settings.unrecognized",
    ],
)
def test_bare_data_assignment_remains_detected(path: str) -> None:
    findings = secret_guard.scan_diff_text(_added(path, "AUTH_TOKEN=" + _credential()))
    assert [item.reason for item in findings] == ["Sensitive assignment value"]


@pytest.mark.parametrize(
    "key", ["token", "AUTH_TOKEN", "API_KEY", "client-api-key", "password"]
)
def test_quoted_mapping_keys_and_spaced_values_are_literal_data(key: str) -> None:
    # Expected from the literal JSON payload, independent of scanner grammar.
    value = _credential()[:16] + " " + _credential()[16:]
    line = json.dumps({key: value})
    findings = secret_guard.scan_diff_text(_added("settings.json", line))
    assert [item.reason for item in findings] == ["Sensitive assignment value"]


@pytest.mark.parametrize(
    ("key", "sensitive"),
    [
        ("apikey", True),
        ("API-KEY", True),
        ("client_api_key_value", True),
        ("_service__SECRET___value", True),
        ("account_password_hash", True),
        ("token_", True),
        ("__Token", True),
        ("api-key--value", True),
        ("tokenizer", False),
        ("secretion", False),
        ("mypassword", False),
        ("api__key", False),
        ("api--key", False),
        ("APIKEYS", False),
        ("ordinary_added_value_1000", False),
        ("apitokenvalue", False),
        ("token", True),
        ("SECRET", True),
        ("Password", True),
        ("APIKEY", True),
        ("api_key", True),
        ("_token", True),
        ("token-", True),
        ("x__token--y", True),
        ("_api_key_", True),
        ("x--api-key__", True),
        ("LINEAR_API_KEY", True),
        ("api__key_token", True),
        ("tokenizer_secret", True),
        ("api_key_suffix", True),
        ("tokens", False),
        ("token2", False),
        ("secretive", False),
        ("passwords", False),
        ("myapikey", False),
        ("xapi_key", False),
        ("api_keyx", False),
        ("api_-key", False),
        ("api-_key", False),
        ("_api__key_", False),
        ("secretary_tokenizer", False),
        ("_", False),
        ("__", False),
        ("9_token", False),
        ("-token", False),
    ],
)
@pytest.mark.parametrize("mapping", [False, True])
def test_sensitive_names_use_complete_components(
    key: str, sensitive: bool, mapping: bool
) -> None:
    line = (
        json.dumps({key: _credential()})
        if mapping
        else key + " = " + json.dumps(_credential())
    )
    findings = secret_guard.scan_diff_text(_added("settings.json", line))
    assert [item.reason for item in findings] == (
        ["Sensitive assignment value"] if sensitive else []
    )


@pytest.mark.parametrize(
    ("name", "sensitive"),
    [
        ("αtoken", False),
        ("é_token", False),
        ("ſecret", False),
        ("apıkey", False),
        ("toKen", False),
        ("secret_İ", False),
        ("secret_I", True),
        ("config.token", True),
        ("namespace::token", True),
        ("prefix-tokenizer", False),
    ],
)
def test_assignment_names_preserve_ascii_grammar_and_unicode_outer_boundary(
    name: str, sensitive: bool
) -> None:
    findings = secret_guard.scan_diff_text(
        _added("settings.py", name + " = " + json.dumps(_credential()))
    )
    assert [item.reason for item in findings] == (
        ["Sensitive assignment value"] if sensitive else []
    )


@pytest.mark.parametrize("word", ["test", "secret", "token", "example", "null"])
def test_embedded_placeholder_word_cannot_exempt_a_credential(word: str) -> None:
    value = _credential()[:16] + word + _credential()[16:]
    assignment = "password = " + json.dumps(value)
    findings = secret_guard.scan_diff_text(_added("settings.py", assignment))
    assert [item.reason for item in findings] == ["Sensitive assignment value"]
    bearer = secret_guard.scan_diff_text(
        _added("request.http", "Authorization: Bearer " + value)
    )
    assert [item.reason for item in bearer] == ["Bearer token"]


@pytest.mark.parametrize("prefix", ["$", "<", "{", "["])
def test_punctuation_prefix_is_not_a_placeholder(prefix: str) -> None:
    findings = secret_guard.scan_diff_text(
        _added("settings.py", "password = " + json.dumps(prefix + _credential()))
    )
    assert [item.reason for item in findings] == ["Sensitive assignment value"]


@pytest.mark.parametrize(
    "value",
    ["replace-with-password", "REPLACE_WITH_API_KEY", "<your-token>", "x" * 32],
)
def test_complete_documented_placeholder_forms_are_allowed(value: str) -> None:
    assert (
        secret_guard.scan_diff_text(
            _added("settings.py", "password = " + json.dumps(value))
        )
        == []
    )


@pytest.mark.parametrize(
    "value",
    [
        "$CREDENTIAL_FROM_RUNTIME_ENVIRONMENT",
        "$" + "{CREDENTIAL_FROM_RUNTIME_ENVIRONMENT}",
    ],
)
def test_bare_environment_reference_does_not_exempt_quoted_source_data(
    value: str,
) -> None:
    assert secret_guard.scan_diff_text(_added(".env", "password=" + value)) == []
    findings = secret_guard.scan_diff_text(
        _added("settings.py", "password=" + json.dumps(value))
    )
    assert [item.reason for item in findings] == ["Sensitive assignment value"]


def test_later_bearer_material_is_not_hidden_by_first_placeholder() -> None:
    line = "Bearer " + "x" * 32 + "; Bearer " + _credential()
    findings = secret_guard.scan_diff_text(_added("request.http", line))
    assert [item.reason for item in findings] == ["Bearer token"]


def test_finding_records_do_not_retain_credential_text() -> None:
    value = _credential()
    findings = secret_guard.scan_diff_text(_added(".env", "AUTH_TOKEN=" + value))
    assert [item.reason for item in findings] == ["Sensitive assignment value"]
    assert value not in repr(findings)


@pytest.mark.parametrize("separator", [".", "~", "+", "/"])
def test_bearer_rfc_alphabet_is_not_truncated_at_punctuation(
    separator: str,
) -> None:
    # RFC 6750 permits these characters. Each alphanumeric part is shorter
    # than the guard's 24-character policy, while the complete token is longer.
    value = _credential()[:16] + separator + _credential()[16:] + "=="
    findings = secret_guard.scan_diff_text(
        _added("request.http", "Authorization: Bearer " + value)
    )
    assert [item.reason for item in findings] == ["Bearer token"]


@pytest.mark.parametrize(
    ("prefix", "reason"),
    [
        ("lin_" + "api_", "Linear API key"),
        ("sk" + "-", "OpenAI-style key"),
        ("ghp" + "_", "GitHub token"),
        ("xox" + "b-", "Slack token"),
    ],
)
def test_provider_recognition_is_independent_of_placeholder_vocabulary(
    prefix: str, reason: str
) -> None:
    value = prefix + "test" + _credential()
    findings = secret_guard.scan_diff_text(_added("arbitrary.source", value))
    assert [item.reason for item in findings] == [reason]


def test_added_header_looking_text_does_not_drop_later_findings() -> None:
    diff = _added("settings.env", "++ ordinary payload", "token=" + _credential())
    findings = secret_guard.scan_diff_text(diff)
    assert [(item.path, item.line_no, item.reason) for item in findings] == [
        ("settings.env", 2, "Sensitive assignment value")
    ]


@pytest.mark.parametrize(
    ("header", "path"),
    [
        (r'"b/with\tseparator.env"', "with\tseparator.env"),
        (r'"b/caf\303\251.env"', "café.env"),
        (r'"b/with\"quote.env"', 'with"quote.env'),
    ],
)
def test_git_quoted_paths_are_scanned(header: str, path: str) -> None:
    diff = "--- /dev/null\n+++ " + header + "\n@@ -0,0 +1 @@\n"
    diff += "+token=" + _credential() + "\n"
    findings = secret_guard.scan_diff_text(diff)
    assert [(item.path, item.line_no, item.reason) for item in findings] == [
        (path, 1, "Sensitive assignment value")
    ]


def test_diff_context_deletions_and_multiple_hunks_keep_exact_lines() -> None:
    diff = (
        "--- a/settings.env\n+++ b/settings.env\n"
        "@@ -2,2 +2,2 @@\n unchanged\n-old\n+short\n"
        "@@ -8,0 +9 @@\n+token=" + _credential() + "\n"
    )
    findings = secret_guard.scan_diff_text(diff)
    assert [(item.path, item.line_no) for item in findings] == [("settings.env", 9)]


def test_unicode_line_separator_does_not_split_a_git_payload_line() -> None:
    value = _credential()[:16] + "\u2028" + _credential()[16:]
    line = "password = " + json.dumps(value, ensure_ascii=False)
    findings = secret_guard.scan_diff_text(_added("settings.py", line))
    assert [(item.path, item.line_no, item.reason) for item in findings] == [
        ("settings.py", 1, "Sensitive assignment value")
    ]


def test_incomplete_diff_cannot_succeed_with_partial_scan() -> None:
    diff = "--- /dev/null\n+++ b/settings.env\n@@ -0,0 +1,2 @@\n+short\n"
    with pytest.raises(ValueError, match="incomplete unified diff hunk"):
        secret_guard.scan_diff_text(diff)


def test_complete_hunk_followed_by_new_file_and_no_newline_marker() -> None:
    diff = (
        "diff --git a/old.env b/old.env\n--- a/old.env\n+++ /dev/null\n"
        "@@ -1 +0,0 @@\n-old\n\\ No newline at end of file\n"
        "diff --git a/new.env b/new.env\n--- /dev/null\n+++ b/new.env\n"
        "@@ -0,0 +1 @@\n+token=" + _credential() + "\n"
        "\\ No newline at end of file\n"
    )
    assert [
        (item.path, item.line_no) for item in secret_guard.scan_diff_text(diff)
    ] == [("new.env", 1)]
