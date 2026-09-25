from __future__ import annotations

import pytest

from molt.cli.source_extension_compiler_inputs import (
    compiler_sysroot_arg_value,
    compiler_sysroot_arguments,
    validate_source_extension_compiler_command,
    validate_source_extension_tool_command,
)


def test_compiler_grammar_admits_matching_target_sysroot_and_codegen_flags() -> None:
    command = (
        "/tools/clang",
        "-target",
        "wasm32-wasip1",
        "--sysroot=/sdk/wasi",
        "--no-default-config",
        "-O2",
        "--driver-mode=gcc",
        "-fPIC",
        "-ffunction-sections",
        "-D_FORTIFY_SOURCE=2",
        "-fvisibility=hidden",
        "-Wno-unused-command-line-argument",
        "-mcpu=generic",
    )

    admitted = validate_source_extension_compiler_command(
        command,
        role="c",
        target_triple="wasm32-wasip1",
        require_explicit_target=True,
        sysroot_policy="required",
        expected_sysroot="/sdk/wasi",
    )

    assert admitted.argv == command
    assert admitted.target_is_explicit
    assert admitted.sysroot == "/sdk/wasi"


@pytest.mark.parametrize(
    "option",
    [
        "@args.rsp",
        "/clang:@args.rsp",
        "-B/toolchain/bin",
        "-fuse-ld=/tools/ld",
        "--gcc-toolchain=/toolchain",
        "-resource-dir=/resources",
        "-Xclang",
        "-Xassembler",
        "-Xlinker",
        "-Wl,@link.rsp",
        "-fplugin=/tools/plugin.so",
        "-I/includes",
        "-isystem/includes",
        "-includeheader.h",
        "-ivfsoverlay=overlay.yaml",
    ],
)
def test_compiler_grammar_rejects_external_selectors(option: str) -> None:
    with pytest.raises(ValueError, match="external input or helper custody"):
        validate_source_extension_compiler_command(
            ("/tools/clang", option), role="c", target_triple="wasm32-wasip1"
        )


@pytest.mark.parametrize("option", ["-D=1", "-E", "-MFdeps.d", "-unknown-flag"])
def test_compiler_grammar_rejects_unknown_or_non_codegen_operands(option: str) -> None:
    with pytest.raises(ValueError, match="positive grammar"):
        validate_source_extension_compiler_command(
            ("/tools/clang", option), role="cpp", target_triple="wasm32-wasip1"
        )


def test_compiler_grammar_rejects_relative_or_mismatched_sysroot() -> None:
    with pytest.raises(ValueError, match="absolute"):
        validate_source_extension_compiler_command(
            ("/tools/clang", "--sysroot", "relative"),
            role="c",
            target_triple="wasm32-wasip1",
            sysroot_policy="required",
        )
    with pytest.raises(ValueError, match="differs from captured"):
        validate_source_extension_compiler_command(
            ("/tools/clang", "--sysroot=/other"),
            role="c",
            target_triple="wasm32-wasip1",
            sysroot_policy="required",
            expected_sysroot="/sdk/wasi",
        )


def test_compiler_grammar_preserves_selected_home_sysroot_for_later_materialization() -> (
    None
):
    admitted = validate_source_extension_compiler_command(
        ("/tools/clang", "--sysroot=~/wasi-sysroot"),
        role="c",
        target_triple="wasm32-wasip1",
        sysroot_policy="required",
    )

    assert admitted.sysroot == "~/wasi-sysroot"


def test_shared_sysroot_parser_preserves_materialization_indices() -> None:
    command = ("/tools/clang", "-O2", "--sysroot=/sdk/wasi")

    assert compiler_sysroot_arguments(command) == ((2, "--sysroot=", "/sdk/wasi"),)
    assert compiler_sysroot_arg_value(command) == "/sdk/wasi"


@pytest.mark.parametrize("command", [("clang", "--sysroot"), ("clang", "--sysroot=")])
def test_shared_sysroot_parser_rejects_missing_values(command: tuple[str, ...]) -> None:
    with pytest.raises(ValueError, match="missing value"):
        compiler_sysroot_arg_value(command)


def test_shared_sysroot_parser_rejects_conflicting_values() -> None:
    with pytest.raises(ValueError, match="conflicting sysroot"):
        compiler_sysroot_arg_value(("clang", "--sysroot=/one", "-isysroot", "/two"))


def test_auxiliary_tool_grammar_has_no_hidden_argv() -> None:
    assert validate_source_extension_tool_command(("/tools/llvm-ar",), role="ar") == (
        "/tools/llvm-ar",
    )
    with pytest.raises(ValueError, match="argument-free"):
        validate_source_extension_tool_command(("/tools/llvm-ar", "rcs"), role="ar")
    assert validate_source_extension_tool_command(("/tools/zig", "ar"), role="ar") == (
        "/tools/zig",
        "ar",
    )
