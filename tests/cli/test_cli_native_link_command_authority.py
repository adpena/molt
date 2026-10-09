from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import native_link_command
from molt.cli.native_link_plan import resolve_native_target_spec
from molt import toolchain_identity

_NATIVE_LINK_COMMAND_NAMES = (
    "_build_native_link_plan",
    "_build_native_link_driver_command",
    "_resolve_available_fast_linker",
    "_resolve_dev_linker",
    "_resolve_native_linker_hint",
)

_NATIVE_LINK_COMMAND_DEFINITIONS = (
    "def _build_native_link_plan(",
    "def _build_native_link_driver_command(",
    "def _resolve_available_fast_linker(",
    "def _resolve_dev_linker(",
    "def _resolve_native_linker_hint(",
)


def test_native_link_resolver_uses_shared_toolchain_authority() -> None:
    from molt.cli import llvm_wasi_tools

    assert (
        native_link_command.resolve_explicit_tool_command
        is toolchain_identity.resolve_explicit_tool_command
    )
    assert not hasattr(llvm_wasi_tools, "resolve_explicit_tool_command")


def test_cross_compiler_environment_preserves_native_quoted_command(
    monkeypatch, tmp_path
):
    tool = tmp_path / "selected tools" / "clang.exe"
    tool.parent.mkdir()
    tool.write_bytes(b"selected compiler fixture")
    monkeypatch.setenv("CC", str(tool))
    monkeypatch.setenv("MOLT_CROSS_CC", f' \t"{tool}" --sysroot="~/selected SDK"')
    monkeypatch.setenv("CFLAGS", "")
    target = resolve_native_target_spec("aarch64-unknown-linux-gnu")
    observed = []
    monkeypatch.setattr(
        native_link_command,
        "validate_compiler_target",
        lambda command, triple: observed.append((list(command), triple)),
    )
    monkeypatch.setattr(
        native_link_command, "_resolve_native_linker_hint", lambda **_: None
    )
    command, linker, triple = native_link_command._build_native_link_driver_command(
        output_obj=None, target=target, sysroot_path=None, profile="release-output"
    )
    expected = [str(tool), "--sysroot=~/selected SDK", "-target", target.triple]
    assert command == expected
    assert observed == [(expected, target.triple)]
    assert linker is None
    assert triple == target.triple


def test_cli_native_link_command_authority_is_single_home() -> None:
    for name in _NATIVE_LINK_COMMAND_NAMES:
        assert getattr(cli, name) is getattr(native_link_command, name)

    cli_source = inspect.getsource(cli)
    for marker in _NATIVE_LINK_COMMAND_DEFINITIONS:
        assert marker not in cli_source
    assert not hasattr(native_link_command, "_windows_coff_library_command")

    command_source = inspect.getsource(native_link_command)
    assert "shutil.which" not in command_source
    assert "llvm_tool_candidates" in command_source
    assert "llvm_named_tool_candidates" in command_source
    assert "resolve_explicit_tool_command" in command_source
