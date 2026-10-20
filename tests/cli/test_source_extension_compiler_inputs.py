from __future__ import annotations


import pytest

from molt.process_environment import NameRule, ProcessEnvironment


from molt.cli.source_extension_compiler_inputs import (
    compiler_sysroot_arg_value,
    compiler_sysroot_arguments,
    validate_source_extension_compiler_command,
    validate_source_extension_tool_command,
    source_extension_compiler_environment,
)
from tests.cli.native_link_test_support import write_test_darwin_sdk


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_compiler_environment_removes_hidden_command_overrides_not_sdk_inputs(rule):
    # An explicit SDKROOT is an SDK input the projection keeps as it is.
    sdk = {
        "INCLUDE": "sdk/include",
        "LIB": "sdk/lib",
        "Path": "first/bin",
        "PATH": "owned/bin",
        "SDKROOT": "sdk/macos",
    }
    overrides = {
        "CL": "/DUNRECORDED=1",
        "_cl_": "/Foother.obj",
        "CCC_OVERRIDE_OPTIONS": "+-target;wasm32-wasip1",
        "CC_LD": "unowned-linker",
        "cxx_ld": "another-linker",
        "CC_LD_FOR_BUILD": "build-linker",
        "CXX_LD_FOR_BUILD": "build-cpp-linker",
    }
    captured = ProcessEnvironment({**sdk, **overrides}, rule=rule)
    before = dict(captured)
    observed = source_extension_compiler_environment(captured)
    expected = dict(sdk)
    if rule is NameRule.WINDOWS:
        expected = {
            "INCLUDE": "sdk/include",
            "LIB": "sdk/lib",
            "Path": "owned/bin",
            "SDKROOT": "sdk/macos",
        }
    assert isinstance(observed, ProcessEnvironment) and observed.rule is rule
    assert dict(observed) == expected
    assert dict(captured) == before


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
    diagnostic = (
        "has no .*operand"
        if option in {"-Xclang", "-Xassembler", "-Xlinker"}
        else "external input or helper custody"
    )
    with pytest.raises(ValueError, match=diagnostic):
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


@pytest.mark.parametrize(
    ("command", "expected"),
    [
        (
            ("clang", "-mllvm", "--sysroot=/opaque", "--sysroot", "/sdk"),
            (4, "", "/sdk"),
        ),
        (
            (
                "clang-cl",
                "/clang:-mllvm",
                "/clang:--sysroot=/opaque",
                "/clang:--sysroot",
                "/clang:/sdk",
            ),
            (4, "/clang:", "/sdk"),
        ),
        (("clang", "-Xclang", "-isysroot", "-Xclang", "/sdk"), (4, "", "/sdk")),
        (("clang", "-Xclang", "-isysroot/sdk"), (2, "-isysroot", "/sdk")),
    ],
)
def test_sysroot_materialization_respects_forwarded_operand_context(command, expected):
    assert compiler_sysroot_arguments(command) == (expected,)
    assert compiler_sysroot_arg_value(command) == "/sdk"


@pytest.mark.parametrize("command", [("clang", "--sysroot"), ("clang", "--sysroot=")])
def test_shared_sysroot_parser_rejects_missing_values(command: tuple[str, ...]) -> None:
    with pytest.raises(ValueError, match="missing value|has no operand"):
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


@pytest.mark.parametrize(
    ("entrypoint", "mode", "role"),
    [
        ("clang", (), "c"),
        ("clang++", (), "cpp"),
        ("clang++.exe", (), "cpp"),
        ("clang", ("--driver-mode=g++",), "cpp"),
        ("clang++", ("--driver-mode=gcc",), "c"),
    ],
)
@pytest.mark.parametrize(
    "target", ["wasm32-wasip1", "wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"]
)
def test_compiler_role_uses_lexical_entrypoint_and_explicit_mode(
    entrypoint, mode, role, target
):
    command = (entrypoint, *mode)
    assert (
        validate_source_extension_compiler_command(
            command, role=role, target_triple=target
        ).argv
        == command
    )


@pytest.mark.parametrize(
    ("command", "role"),
    [
        (("clang",), "cpp"),
        (("clang++",), "c"),
        (("clang++", "--driver-mode=gcc"), "cpp"),
        (("clang", "--driver-mode=g++", "--driver-mode=gcc"), "c"),
    ],
)
@pytest.mark.parametrize(
    "target", ["wasm32-wasip1", "wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"]
)
def test_compiler_refuses_wrong_or_conflicting_role(command, role, target):
    with pytest.raises(ValueError, match="driver mode"):
        validate_source_extension_compiler_command(
            command, role=role, target_triple=target
        )


@pytest.mark.parametrize(
    "target", ["wasm32-wasip1", "wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"]
)
@pytest.mark.parametrize(
    "options", [("--driver-mode", "g++"), ("-x", "c++"), ("-xc++",), ("-ansi",)]
)
def test_base_compiler_rejects_unowned_language_spelling(target, options):
    with pytest.raises(ValueError, match="positive grammar"):
        validate_source_extension_compiler_command(
            ("clang++", *options), role="cpp", target_triple=target
        )


@pytest.mark.parametrize(
    ("command", "role"),
    [
        (("clang++", "-std=c17"), "cpp"),
        (("clang", "-std=c++20"), "c"),
        (("clang-cl", "/clang:-std=c11"), "cpp"),
    ],
)
def test_base_compiler_language_standard_matches_declared_role(command, role):
    target = (
        "x86_64-pc-windows-msvc"
        if command[0] == "clang-cl"
        else "x86_64-unknown-linux-gnu"
    )
    with pytest.raises(ValueError, match="standard for a different language"):
        validate_source_extension_compiler_command(
            command, role=role, target_triple=target
        )


def test_compiler_environment_never_discovers_an_implicit_sdk(monkeypatch):
    from molt import platform_toolchain

    monkeypatch.setattr(
        platform_toolchain,
        "select_darwin_sdk",
        lambda *_: pytest.fail("environment filtering selected an Apple SDK"),
    )
    assert dict(source_extension_compiler_environment({"PATH": "selected"})) == {
        "PATH": "selected"
    }


@pytest.mark.parametrize(
    "forwarded",
    [
        ("-Wl,-syslibroot,/other",),
        ("-Wl,--sysroot=/other",),
        ("-Xlinker", "-syslibroot", "-Xlinker", "/other"),
        ("-Xlinker", "--sysroot=/other"),
        ("-Wl,-platform_version,macos,12.0,14.0",),
    ],
)
def test_darwin_compiler_projection_rejects_independent_linker_sdk_selection(forwarded):
    from molt.cli.source_extension_compiler_inputs import darwin_compiler_commands

    with pytest.raises(ValueError, match="not linker overrides"):
        darwin_compiler_commands(
            {"c": ("clang", *forwarded)}, environment={}, arch="arm64"
        )


def test_darwin_compiler_projection_preserves_opaque_backend_arguments(tmp_path):
    from molt.cli.source_extension_compiler_inputs import darwin_compiler_commands

    sdk = write_test_darwin_sdk(tmp_path / "SDK")
    command = ("clang", "-mllvm", "-mmacosx-version-min=opaque")
    result = darwin_compiler_commands(
        {"c": command}, environment={"SDKROOT": str(sdk)}, arch="arm64"
    )["c"]
    assert result[:3] == command
    assert result[3:] == ("-isysroot", str(sdk), "-mmacosx-version-min=15.4")


@pytest.mark.parametrize("transport", ["separate", "joined", "cc1", "cc1_joined"])
def test_darwin_compiler_projection_canonicalizes_sdk_transport(tmp_path, transport):
    from molt.cli.source_extension_compiler_inputs import darwin_compiler_commands

    sdk = write_test_darwin_sdk(tmp_path / "SDK")
    roots = {
        "separate": ("-isysroot", str(sdk)),
        "joined": ("-isysroot" + str(sdk),),
        "cc1": ("-Xclang", "-isysroot", "-Xclang", str(sdk)),
        "cc1_joined": ("-Xclang", "-isysroot" + str(sdk)),
    }
    result = darwin_compiler_commands(
        {"c": ("clang", *roots[transport])}, environment={}, arch="arm64"
    )["c"]
    assert result == ("clang", "-isysroot", str(sdk), "-mmacosx-version-min=15.4")


@pytest.mark.parametrize(
    "command",
    [
        ("clang", "-isysroot/sdk"),
        ("clang", "-Xclang", "-isysroot/sdk"),
        ("clang-cl", "/clang:-isysroot/sdk"),
    ],
)
def test_joined_sysroot_matches_clang_operand_grammar(command):
    assert compiler_sysroot_arg_value(command) == "/sdk"


def test_driver_isysroot_equals_is_a_literal_relative_operand():
    assert compiler_sysroot_arg_value(("clang", "-isysroot=/sdk")) == "=/sdk"
    with pytest.raises(ValueError, match="absolute"):
        validate_source_extension_compiler_command(
            ("clang", "-isysroot=/sdk"),
            role="c",
            target_triple="aarch64-apple-darwin",
            sysroot_policy="optional",
        )


@pytest.mark.parametrize(
    "command",
    [
        ("clang", "@hidden.rsp"),
        ("clang", "-Wl,@hidden.rsp"),
        ("clang", "-Xlinker", "@hidden.rsp"),
    ],
)
def test_darwin_response_arguments_cannot_hide_sdk_selection(command):
    from molt.cli.source_extension_compiler_inputs import darwin_compiler_commands

    with pytest.raises(ValueError, match="response|linker overrides"):
        darwin_compiler_commands({"c": command}, environment={}, arch="arm64")


@pytest.mark.parametrize(
    "defect",
    [
        None,
        "missing_sdk",
        "missing_deployment",
        "different_sdk",
        "different_deployment",
        "noncanonical_version",
        "hidden_linker_sdk",
        "frontend_only_sdk",
    ],
)
def test_published_darwin_command_custody_needs_no_producer_filesystem(
    monkeypatch, defect
):
    from molt import platform_toolchain
    from molt.cli.source_extension_compiler_inputs import (
        validate_darwin_compiler_commands,
    )

    monkeypatch.setattr(
        platform_toolchain,
        "select_darwin_sdk",
        lambda *_: pytest.fail("published metadata probed a producer SDK"),
    )
    commands = {
        role: (name, "-isysroot", "@llvm-sdk", "-mmacosx-version-min=15.4")
        for role, name in (("c", "@llvm-bin/clang"), ("cpp", "@llvm-bin/clang++"))
    }
    if defect == "missing_sdk":
        commands["cpp"] = ("@llvm-bin/clang++", "-mmacosx-version-min=15.4")
    elif defect == "missing_deployment":
        commands["cpp"] = ("@llvm-bin/clang++", "-isysroot", "@llvm-sdk")
    elif defect == "different_sdk":
        commands["cpp"] = (
            "@llvm-bin/clang++",
            "-isysroot",
            "@other-sdk",
            "-mmacosx-version-min=15.4",
        )
    elif defect == "different_deployment":
        commands["cpp"] = (
            "@llvm-bin/clang++",
            "-isysroot",
            "@llvm-sdk",
            "-mmacosx-version-min=14.0",
        )
    elif defect == "noncanonical_version":
        commands["cpp"] = (
            "@llvm-bin/clang++",
            "-isysroot",
            "@llvm-sdk",
            "-mmacosx-version-min=15.4.0",
        )
    elif defect == "hidden_linker_sdk":
        commands["cpp"] += ("-Wl,-syslibroot,@other-sdk",)
    elif defect == "frontend_only_sdk":
        commands["cpp"] = (
            "@llvm-bin/clang++",
            "-Xclang",
            "-isysroot",
            "-Xclang",
            "@llvm-sdk",
            "-mmacosx-version-min=15.4",
        )
    if defect is None:
        validate_darwin_compiler_commands(commands)
    else:
        with pytest.raises(ValueError):
            validate_darwin_compiler_commands(commands)
