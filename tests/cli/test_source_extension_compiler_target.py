from __future__ import annotations

from pathlib import Path
import shlex

import pytest

from molt.cli import (
    llvm_wasi_tools,
    source_extension_target,
    source_extension_toolchain,
)
from molt.cli.compiler_target import compiler_target_triple, validate_compiler_target


@pytest.mark.parametrize(
    "selectors",
    [
        ("-target", "x86_64-unknown-linux-gnu"),
        ("--target", "x86_64-unknown-linux-gnu"),
        ("-target=x86_64-unknown-linux-gnu",),
        ("--target=x86_64-unknown-linux-gnu",),
        (
            "-target",
            "x86_64-unknown-linux-gnu",
            "--target=x86_64-unknown-linux-gnu",
        ),
    ],
)
def test_compiler_target_preserves_matching_selectors(
    selectors: tuple[str, ...],
) -> None:
    command = ("clang", *selectors)
    target = "x86_64-unknown-linux-gnu"
    assert validate_compiler_target(command, target)
    assert (
        source_extension_toolchain._compiler_command_with_target(command, target)
        == command
    )


@pytest.mark.parametrize(
    "selectors",
    [
        ("-target",),
        ("--target",),
        ("-target=",),
        ("--target=",),
        ("-target", "-O2"),
        ("--target", ""),
        ("--target=aarch64-unknown-linux-gnu",),
        ("-target", "x86_64-pc-windows-msvc"),
        ("--target=x86_64-unknown-linux-musl",),
        ("--target=x86_64-unknown-linux-gnu", "-target", "aarch64-unknown-linux-gnu"),
    ],
)
def test_compiler_target_rejects_malformed_or_conflicting_selectors(
    selectors: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError, match="target"):
        validate_compiler_target(["clang", *selectors], "x86_64-unknown-linux-gnu")


@pytest.mark.parametrize("explicit_target", [False, True])
def test_compiler_target_injection_is_separate_from_validation(
    explicit_target: bool,
) -> None:
    target = "x86_64-unknown-linux-gnu"
    command = ("clang", "-O2")
    assert not validate_compiler_target(command, target)
    assert source_extension_toolchain._compiler_command_with_target(
        command, target, explicit_target=explicit_target
    ) == ((*command, "-target", target) if explicit_target else command)
    with pytest.raises(ValueError, match="target conflicts"):
        source_extension_toolchain._compiler_command_with_target(
            ("clang", "--target=aarch64-unknown-linux-gnu"),
            target,
            explicit_target=explicit_target,
        )


@pytest.mark.parametrize(
    "triple, flags",
    [
        ("x86_64-unknown-linux-gnu", ("-m64",)),
        ("i686-unknown-linux-gnu", ("-m32",)),
        ("x86_64-unknown-linux-gnux32", ("-mx32",)),
        ("aarch64-apple-darwin", ("-arch", "arm64")),
        ("x86_64-apple-darwin", ("-arch=x86_64",)),
    ],
)
def test_matching_machine_overrides_do_not_replace_triple_custody(
    triple: str,
    flags: tuple[str, ...],
) -> None:
    assert not validate_compiler_target(("clang", *flags), triple)


@pytest.mark.parametrize(
    "triple, flags",
    [
        ("x86_64-unknown-linux-gnu", ("-m32",)),
        ("x86_64-unknown-linux-gnu", ("-mx32",)),
        ("i686-unknown-linux-gnu", ("-m64",)),
        ("x86_64-unknown-linux-gnux32", ("-m64",)),
        ("aarch64-unknown-linux-gnu", ("-m64",)),
        ("aarch64-apple-darwin", ("-arch", "x86_64")),
        ("aarch64-apple-darwin", ("-arch", "arm64", "-arch", "x86_64")),
        ("x86_64-unknown-linux-gnu", ("-arch",)),
        ("x86_64-unknown-linux-gnu", ("-arch=",)),
    ],
)
def test_foreign_or_unverified_machine_overrides_fail_closed(
    triple: str,
    flags: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError, match="arch|target"):
        validate_compiler_target(("clang", *flags), triple)


def _mock_native_tools(
    monkeypatch: pytest.MonkeyPatch,
    *,
    discovered_cpp: tuple[str, ...] = ("clang++",),
) -> None:
    for name in ("CC", "CXX", "MOLT_CROSS_CC", "MOLT_CROSS_CXX"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setattr(
        source_extension_toolchain,
        "resolve_explicit_tool_command",
        lambda command, **_kwargs: tuple(shlex.split(command)),
    )
    monkeypatch.setattr(
        source_extension_toolchain,
        "llvm_tool_candidates",
        lambda role, **_kwargs: (Path("clang"),) if role == "cc" else (),
    )

    def tool(
        role: llvm_wasi_tools.LlvmToolRole, command: tuple[str, ...]
    ) -> llvm_wasi_tools.ResolvedLlvmTool:
        return llvm_wasi_tools.ResolvedLlvmTool(
            role=role,
            command=command,
            path=Path(command[0]),
            version="22.1.8",
            sha256="a" * 64,
        )

    def family(
        *,
        target_family: llvm_wasi_tools.LlvmTargetFamily,
        explicit_commands: dict[llvm_wasi_tools.LlvmToolRole, tuple[str, ...]],
        sibling_directories: tuple[Path, ...],
        environment: object,
    ) -> llvm_wasi_tools.LlvmWasiToolFamily:
        del sibling_directories, environment
        assert target_family == "native"
        return llvm_wasi_tools.LlvmWasiToolFamily(
            cc=tool("cc", explicit_commands["cc"]),
            cxx=tool("cxx", explicit_commands.get("cxx", discovered_cpp)),
            wasm_ld=None,
            ar=tool("ar", ("llvm-ar",)),
            ranlib=None,
            nm=tool("nm", ("llvm-nm",)),
            strip=None,
        )

    monkeypatch.setattr(
        source_extension_toolchain, "resolve_llvm_wasi_tool_family", family
    )


@pytest.mark.parametrize("requested", ["native", "x86_64-unknown-linux-gnu"])
@pytest.mark.parametrize("role", ["c", "cpp", "discovered_cpp"])
def test_native_toolchain_rejects_each_foreign_compiler_target(
    monkeypatch: pytest.MonkeyPatch,
    requested: str,
    role: str,
) -> None:
    foreign = ("clang++", "--target=aarch64-unknown-linux-gnu")
    _mock_native_tools(
        monkeypatch,
        discovered_cpp=foreign if role == "discovered_cpp" else ("clang++",),
    )
    cross = requested != "native"
    cc_name, cpp_name = ("MOLT_CROSS_CC", "MOLT_CROSS_CXX") if cross else ("CC", "CXX")
    monkeypatch.setenv(cc_name, "clang")
    if role != "discovered_cpp":
        monkeypatch.setenv(cc_name if role == "c" else cpp_name, " ".join(foreign))
    plan = source_extension_target.resolve_source_extension_target_plan(
        requested, host_platform="linux", host_arch="x86_64"
    )
    with pytest.raises(ValueError, match="target conflicts"):
        source_extension_toolchain._resolve_source_extension_native_toolchain(plan)


@pytest.mark.parametrize(
    "host_platform, host_arch, triple",
    [
        ("linux", "x86_64", "x86_64-unknown-linux-gnu"),
        ("win32", "AMD64", "x86_64-pc-windows-msvc"),
        ("darwin", "arm64", "aarch64-apple-darwin"),
    ],
)
@pytest.mark.parametrize("explicit_request", [False, True])
@pytest.mark.parametrize("configured_selector", [False, True])
def test_native_toolchain_preserves_each_matching_compiler_projection(
    monkeypatch: pytest.MonkeyPatch,
    host_platform: str,
    host_arch: str,
    triple: str,
    explicit_request: bool,
    configured_selector: bool,
) -> None:
    _mock_native_tools(monkeypatch)
    cc_name, cpp_name = (
        ("MOLT_CROSS_CC", "MOLT_CROSS_CXX") if explicit_request else ("CC", "CXX")
    )
    selector = (f"--target={triple}",) if configured_selector else ()
    c, cpp = (
        ("clang-cl", "clang-cl") if host_platform == "win32" else ("clang", "clang++")
    )
    for name, compiler in ((cc_name, c), (cpp_name, cpp)):
        monkeypatch.setenv(name, " ".join((compiler, *selector)))
    plan = source_extension_target.resolve_source_extension_target_plan(
        triple if explicit_request else "native",
        host_platform=host_platform,
        host_arch=host_arch,
    )
    resolved = source_extension_toolchain._resolve_source_extension_native_toolchain(
        plan
    )
    injected = (
        (f"--target={triple}",) if host_platform == "win32" else ("-target", triple)
    )
    expected_args = selector or (injected if explicit_request else ())
    assert resolved.commands["c"] == (c, *expected_args)
    assert resolved.commands["cpp"] == (cpp, *expected_args)


@pytest.mark.parametrize(
    "command, canonical, expected",
    [
        (("zig", "cc"), "wasm32-wasip1", "wasm32-wasi"),
        ((r"C:\tools\zig.exe", "c++"), "wasm32-wasip1", "wasm32-wasi"),
        (("/tools/zig", "cc"), "aarch64-apple-darwin", "aarch64-macos"),
        (("zig", "cc"), "x86_64-unknown-linux-gnu", "x86_64-linux-gnu"),
        (("clang",), "wasm32-wasip1", "wasm32-wasip1"),
        (("clang", "-Dzig=1"), "wasm32-wasip1", "wasm32-wasip1"),
        (("not-zig", "cc"), "wasm32-wasip1", "wasm32-wasip1"),
    ],
)
def test_compiler_target_spelling_follows_actual_driver(
    command: tuple[str, ...],
    canonical: str,
    expected: str,
) -> None:
    assert compiler_target_triple(command, canonical) == expected
    assert source_extension_toolchain._compiler_command_with_target(
        command, canonical
    ) == (*command, "-target", expected)


@pytest.mark.parametrize("configured_selector", [False, True])
def test_wasm_probe_and_materialization_share_zig_target_spelling(
    configured_selector: bool,
) -> None:
    command = ("zig", "cc")
    if configured_selector:
        command += ("-target", "wasm32-wasi")
    probe_args = source_extension_toolchain._compiler_probe_target_args(
        command, "wasm32-wasip1"
    )
    assert (*command, *probe_args) == ("zig", "cc", "-target", "wasm32-wasi")
    assert source_extension_toolchain._compiler_command_with_target(
        command, "wasm32-wasip1"
    ) == (*command, *probe_args)
    with pytest.raises(ValueError, match="target conflicts"):
        source_extension_toolchain._compiler_probe_target_args(
            ("zig", "cc", "-target", "wasm32-freestanding"), "wasm32-wasip1"
        )


@pytest.mark.parametrize("requested", ["native", "x86_64-unknown-linux-gnu"])
@pytest.mark.parametrize("cpp_driver", ["implicit", "zig", "clang"])
def test_configured_zig_native_compilers_use_per_driver_target_spelling(
    monkeypatch: pytest.MonkeyPatch,
    requested: str,
    cpp_driver: str,
) -> None:
    _mock_native_tools(monkeypatch)
    cross = requested != "native"
    cc_name, cpp_name = ("MOLT_CROSS_CC", "MOLT_CROSS_CXX") if cross else ("CC", "CXX")
    monkeypatch.setenv(cc_name, "zig cc --target=x86_64-linux-gnu")
    if cpp_driver == "zig":
        monkeypatch.setenv(cpp_name, "zig c++ --target=x86_64-linux-gnu")
    elif cpp_driver == "clang":
        monkeypatch.setenv(cpp_name, "clang++ --target=x86_64-unknown-linux-gnu")
    plan = source_extension_target.resolve_source_extension_target_plan(
        requested, host_platform="linux", host_arch="x86_64"
    )
    resolved = source_extension_toolchain._resolve_source_extension_native_toolchain(
        plan
    )
    assert resolved.commands["c"] == ("zig", "cc", "--target=x86_64-linux-gnu")
    assert resolved.commands["cpp"] == (
        ("clang++", "--target=x86_64-unknown-linux-gnu")
        if cpp_driver == "clang"
        else ("zig", "c++", "--target=x86_64-linux-gnu")
    )


def test_windows_default_uses_one_clang_cl_driver_for_both_languages(monkeypatch):
    _mock_native_tools(monkeypatch)
    observed = []

    def candidates(role, **kwargs):
        observed.append((role, kwargs["target_triple"]))
        return (Path("clang-cl"),)

    monkeypatch.setattr(source_extension_toolchain, "llvm_tool_candidates", candidates)
    plan = source_extension_target.resolve_source_extension_target_plan(
        "native", host_platform="win32", host_arch="AMD64"
    )
    resolved = source_extension_toolchain._resolve_source_extension_native_toolchain(
        plan
    )
    assert observed == [("cc", "x86_64-pc-windows-msvc")]
    assert resolved.commands["c"] == resolved.commands["cpp"] == ("clang-cl",)
    assert resolved.tools.cc.command == resolved.tools.cxx.command


@pytest.mark.parametrize(
    "compiler, target",
    [
        (("clang",), "x86_64-pc-windows-msvc"),
        (("clang-cl", "--driver-mode=gcc"), "x86_64-pc-windows-msvc"),
        (("clang-cl",), "wasm32-wasip1"),
        (("clang-cl",), "x86_64-unknown-linux-gnu"),
    ],
)
def test_source_compiler_admission_rejects_incompatible_dialects(compiler, target):
    from molt.cli.source_extension_compiler_inputs import (
        validate_source_extension_compiler_command,
    )

    with pytest.raises(ValueError, match="incompatible"):
        validate_source_extension_compiler_command(
            compiler, role="c", target_triple=target
        )


def test_clang_cl_target_passthrough_is_admitted_and_cannot_override_target():
    from molt.cli.source_extension_compiler_inputs import (
        validate_source_extension_compiler_command,
    )

    target = "x86_64-pc-windows-msvc"
    command = ("clang-cl", "/clang:-target", "/clang:" + target)
    assert (
        validate_source_extension_compiler_command(
            command, role="cpp", target_triple=target
        ).argv
        == command
    )
    with pytest.raises(ValueError, match="target conflicts"):
        validate_source_extension_compiler_command(
            command, role="c", target_triple="aarch64-pc-windows-msvc"
        )


def test_clang_cl_replay_removes_all_owned_selectors_and_forwards_semantics():
    from molt.cli.source_extensions import _source_extension_replay_compile_args

    args = _source_extension_replay_compile_args(
        (
            "/clang:--target=x86_64-pc-windows-msvc",
            "/clang:--sysroot",
            "/clang:C:/sdk",
            "--driver-mode=cl",
            "-m64",
            "-ffile-prefix-map=C:/old=.",
            "-fno-exceptions",
            "-include",
            "C:/owned/config.h",
            "/O2",
        ),
        compiler_target="x86_64-pc-windows-msvc",
        compiler_command=("clang-cl",),
    )
    assert args == [
        "/clang:-fno-exceptions",
        "/clang:-include",
        "/clang:C:/owned/config.h",
        "/O2",
    ]


@pytest.mark.parametrize(
    "selectors",
    [
        ("/winsysroot", "C:/old"),
        ("/winsysroot:C:/old",),
        ("/winsysroot=C:/old",),
        ("-Xclang", "-isysroot", "-Xclang", "C:/old"),
        ("/clang:-Xclang", "/clang:-isysroot", "/clang:-Xclang", "/clang:C:/old"),
    ],
)
def test_replay_owns_all_driver_and_frontend_sysroot_spellings(selectors):
    from molt.cli.source_extensions import _source_extension_replay_compile_args

    assert _source_extension_replay_compile_args(
        (*selectors, "/MD", "/FI", "C:/owned/config.h"),
        compiler_target="x86_64-pc-windows-msvc",
        compiler_command=("clang-cl",),
    ) == ["/MD", "/FI", "C:/owned/config.h"]


def test_forwarded_frontend_target_cannot_bypass_replay_target_validation():
    from molt.cli.source_extensions import _source_extension_replay_compile_args

    with pytest.raises(ValueError, match="target conflicts"):
        _source_extension_replay_compile_args(
            ("-Xclang", "-triple", "-Xclang", "aarch64-pc-windows-msvc"),
            compiler_target="x86_64-pc-windows-msvc",
            compiler_command=("clang-cl",),
        )
