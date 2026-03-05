from __future__ import annotations

from pathlib import Path

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


def test_detect_missing_java_runtime_signature() -> None:
    output = "The operation couldn't be completed. Unable to locate a Java Runtime."
    assert check_formal_methods._detect_missing_java_runtime(output) is True


def test_resolve_java_home_prefers_java_home_env(monkeypatch, tmp_path: Path) -> None:
    java_home = tmp_path / "jdk"
    java_bin = java_home / "bin"
    java_bin.mkdir(parents=True)
    (java_bin / "java").write_text("", encoding="utf-8")

    monkeypatch.setenv("JAVA_HOME", str(java_home))
    monkeypatch.setenv("MOLT_JAVA_HOME", "")

    assert check_formal_methods._resolve_java_home() == str(java_home)


def test_resolve_apalache_work_dir_prefers_env(monkeypatch, tmp_path: Path) -> None:
    work_dir = tmp_path / "apalache-work"
    monkeypatch.setenv("MOLT_APALACHE_WORK_DIR", str(work_dir))
    resolved = check_formal_methods._resolve_apalache_work_dir()
    assert resolved == work_dir.resolve()
    assert resolved.exists()


def test_resolve_quint_fallback_prefix_prefers_env(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_QUINT_NODE_FALLBACK", "node22-wrapper --flag")
    assert check_formal_methods._resolve_quint_fallback_prefix() == [
        "node22-wrapper",
        "--flag",
    ]
