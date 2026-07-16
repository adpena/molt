from __future__ import annotations

from pathlib import Path

from molt.cli import native_link_command


def test_bool_singletons_need_no_final_link_aliases() -> None:
    assert (
        "_Py_TrueStruct",
        "Py_True",
    ) not in native_link_command._CPYTHON_SINGLETON_CANONICAL_ALIASES
    assert (
        "_Py_FalseStruct",
        "Py_False",
    ) not in native_link_command._CPYTHON_SINGLETON_CANONICAL_ALIASES
    discovery = (
        Path(__file__).parents[2] / "runtime" / "molt-cext-discovery" / "build.rs"
    ).read_text(encoding="utf-8")
    assert '("Py_True", "_Py_TrueStruct")' not in discovery
    assert '("Py_False", "_Py_FalseStruct")' not in discovery


def _command(monkeypatch, tmp_path: Path, platform: str) -> list[str]:
    monkeypatch.setattr(native_link_command.sys, "platform", platform)
    monkeypatch.setattr(
        native_link_command,
        "_collect_cargo_native_link_deps",
        lambda _runtime_lib, **_kwargs: [],
    )
    monkeypatch.setattr(
        native_link_command,
        "_append_darwin_runtime_frameworks",
        lambda _command, *, target_triple: None,
    )
    output_obj = tmp_path / "app.o"
    stub_path = tmp_path / "main.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    for path in (output_obj, stub_path, runtime_lib):
        path.write_bytes(b"x")
    plan = native_link_command._build_native_link_plan(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=tmp_path / "app",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        source_root=tmp_path,
        source_fingerprint={},
        export_molt_runtime_symbols=True,
    )
    return list(plan.command)


def test_darwin_native_artifact_link_aliases_singletons_to_same_storage(
    monkeypatch, tmp_path: Path
) -> None:
    command = _command(monkeypatch, tmp_path, "darwin")
    for canonical, storage in native_link_command._CPYTHON_SINGLETON_CANONICAL_ALIASES:
        assert f"-Wl,-alias,_{storage},_{canonical}" in command
    exports = (tmp_path / ".molt_exports.exp").read_text(encoding="utf-8")
    assert "__Py_NoneStruct\n" in exports
    assert "_Py_None\n" in exports


def test_linux_native_artifact_link_defines_canonical_singleton_aliases(
    monkeypatch, tmp_path: Path
) -> None:
    command = _command(monkeypatch, tmp_path, "linux")
    for canonical, storage in native_link_command._CPYTHON_SINGLETON_CANONICAL_ALIASES:
        assert f"-Wl,--defsym={canonical}={storage}" in command
    version_script = (tmp_path / ".molt_version.ver").read_text(encoding="utf-8")
    assert "_Py_NoneStruct; Py_None;" in version_script


def test_windows_native_artifact_exports_aliases_not_duplicate_storage(
    monkeypatch, tmp_path: Path
) -> None:
    command = _command(monkeypatch, tmp_path, "win32")
    assert any(arg.startswith("-Wl,/DEF:") for arg in command)
    exports = (tmp_path / ".molt_exports.def").read_text(encoding="utf-8")
    assert "_Py_NoneStruct=Py_None\n" in exports
    assert "_Py_NotImplementedStruct=Py_NotImplementedSentinel\n" in exports
