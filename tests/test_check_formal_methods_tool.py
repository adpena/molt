from __future__ import annotations

import tools.check_formal_methods as check_formal_methods


def test_parse_node_major() -> None:
    assert check_formal_methods._parse_node_major("v25.8.0") == 25
    assert check_formal_methods._parse_node_major("invalid") is None


def test_detect_runtime_mismatch_signature() -> None:
    output = (
        "ReferenceError: require is not defined in ES module scope\n"
        "Error [ERR_REQUIRE_ESM]\n"
        "Node.js v25.8.0\n"
    )
    assert check_formal_methods._detect_runtime_mismatch(output) is True


def test_resolve_quint_fallback_prefix_prefers_env(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_QUINT_NODE_FALLBACK", "node22-wrapper --flag")
    assert check_formal_methods._resolve_quint_fallback_prefix() == [
        "node22-wrapper",
        "--flag",
    ]
