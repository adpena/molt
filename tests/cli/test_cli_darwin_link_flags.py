from __future__ import annotations

from molt.cli.native_link_plan import resolve_native_target_spec

import json
import os
import shutil
import sys
from pathlib import Path

import pytest

import molt.cli.native_toolchain as NATIVE_TOOLCHAIN
from molt.cli import native_link_command
from molt.cli.native_link_manifest import write_native_link_dependency_manifest
from tests.cli.native_link_test_support import (
    write_test_darwin_sdk,
    write_test_static_archive,
)
from tests.cli.process_guard import run_cli_test_process
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity
from molt.cli import native_link_deps as cli_native_link_deps


def _cargo_output(message: str, native_arguments: str = "") -> str:
    return "\n".join(
        (
            message,
            json.dumps(
                {
                    "reason": "compiler-message",
                    "message": {
                        "message": f"native-static-libs: {native_arguments}",
                        "level": "note",
                    },
                }
            ),
        )
    )


def test_append_darwin_runtime_frameworks_for_host_darwin(
    monkeypatch,
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    args = ["clang", "-lc++"]
    NATIVE_TOOLCHAIN._append_darwin_runtime_frameworks(
        args,
        target=resolve_native_target_spec(
            None, host_platform="darwin", host_arch="arm64"
        ),
    )
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


def test_append_darwin_runtime_frameworks_for_cross_target() -> None:
    import os

    os.environ.pop("MOLT_RUNTIME_GPU_METAL", None)
    args = ["zig", "cc", "-target", "aarch64-macos"]
    NATIVE_TOOLCHAIN._append_darwin_runtime_frameworks(
        args, target=resolve_native_target_spec("aarch64-apple-darwin")
    )
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


def test_append_darwin_runtime_frameworks_adds_metal_when_enabled(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    args = ["clang", "-lc++"]
    NATIVE_TOOLCHAIN._append_darwin_runtime_frameworks(
        args, target=resolve_native_target_spec("aarch64-apple-darwin")
    )
    assert args[-7:] == [
        "-framework",
        "Security",
        "-framework",
        "CoreFoundation",
        "-framework",
        "Metal",
        "-lobjc",
    ]


def test_append_darwin_runtime_frameworks_adds_webgpu_when_enabled(
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_GPU_WEBGPU", "1")
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    args = ["clang", "-lc++"]
    NATIVE_TOOLCHAIN._append_darwin_runtime_frameworks(
        args,
        target=resolve_native_target_spec(
            None, host_platform="darwin", host_arch="arm64"
        ),
    )
    assert args[-13:] == [
        "-framework",
        "Security",
        "-framework",
        "CoreFoundation",
        "-framework",
        "Metal",
        "-framework",
        "Foundation",
        "-framework",
        "QuartzCore",
        "-framework",
        "AppKit",
        "-lobjc",
    ]


def test_collect_cargo_native_link_deps_preserves_framework_link_kinds(
    tmp_path: Path,
) -> None:
    target_triple = "aarch64-apple-darwin"
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="darwin-framework-link-kinds",
    )
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    runtime_lib.write_bytes(b"!<arch>\n")
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    write_native_link_dependency_manifest(
        _cargo_output(
            json.dumps(
                {
                    "reason": "build-script-executed",
                    "package_id": "registry#mdk-sys@0.1.0",
                    "out_dir": str(out_dir),
                    "linked_paths": [],
                    "linked_libs": [
                        "framework=Metal",
                        "framework=Foundation",
                        "dylib=c++",
                    ],
                }
            ),
            "-framework Metal -framework Foundation -lc++",
        ),
        runtime_lib=runtime_lib,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )

    link_flags = list(
        cli_native_link_deps._collect_cargo_native_link_deps(
            runtime_lib,
            target_triple=target_triple,
            object_format="macho",
            runtime_build_identity=build_identity,
        ).flags
    )

    assert link_flags == [
        "-framework",
        "Metal",
        "-framework",
        "Foundation",
        "-lc++",
    ]


def test_collect_cargo_native_link_deps_ignores_stale_inactive_build_outputs(
    tmp_path: Path,
) -> None:
    target_triple = "aarch64-apple-darwin"
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="release-output",
        target_triple=target_triple,
        family_seed="darwin-ignore-stale-build-outputs",
    )
    runtime_lib = tmp_path / "target" / "release-output" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    write_test_static_archive(runtime_lib)
    exact_out = tmp_path / "exact-out"
    exact_out.mkdir()

    build_dir = runtime_lib.parent / "build"
    active = build_dir / "getrandom-aaaaaaaaaaaaaaaa" / "output"
    stale = build_dir / "lzma-sys-bbbbbbbbbbbbbbbb" / "output"
    active.parent.mkdir(parents=True)
    stale.parent.mkdir(parents=True)
    active.write_text(
        "cargo:rustc-link-lib=framework=Security\n",
        encoding="utf-8",
    )
    stale.write_text(
        "\n".join(
            [
                "cargo:rustc-link-search=native=/opt/homebrew/Cellar/xz/5.8.3/lib",
                "cargo:rustc-link-lib=lzma",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    write_native_link_dependency_manifest(
        _cargo_output(
            json.dumps(
                {
                    "reason": "build-script-executed",
                    "package_id": "registry#getrandom@0.3.0",
                    "out_dir": str(exact_out),
                    "linked_paths": [],
                    "linked_libs": ["framework=Security"],
                }
            ),
            "-framework Security",
        ),
        runtime_lib=runtime_lib,
        cargo_profile="release-output",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )

    link_flags = list(
        cli_native_link_deps._collect_cargo_native_link_deps(
            runtime_lib,
            target_triple=target_triple,
            object_format="macho",
            runtime_build_identity=build_identity,
        ).flags
    )

    assert link_flags == ["-framework", "Security"]
    assert "-llzma" not in link_flags


def _darwin_native_link_plan(
    monkeypatch, tmp_path: Path, *, driver: tuple[str, ...] = ("clang",)
):
    target_triple = "aarch64-apple-darwin"
    # The plan's driver comes from host compiler discovery (zig or
    # MOLT_CROSS_CC off macOS); these cases check only the planned flags.
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: ([*driver, "-target", target_triple], None, target_triple),
    )
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="darwin-metal-runtime-plan",
    )
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    write_test_static_archive(runtime_lib)
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path = tmp_path / "main_stub.c"
    stub_path.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    output_binary = tmp_path / "app"
    write_native_link_dependency_manifest(
        _cargo_output(""),
        runtime_lib=runtime_lib,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )

    plan = native_link_command._build_native_link_plan(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target=resolve_native_target_spec(target_triple),
        sysroot_path=None,
        profile="dev",
        runtime_build_identity=build_identity,
        stdlib_obj_path=None,
    )
    return plan, output_binary


def test_build_native_link_plan_includes_metal_frameworks_when_runtime_gpu_metal_enabled(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    plan, _output = _darwin_native_link_plan(monkeypatch, tmp_path)

    assert "-framework" in plan.command
    assert "Metal" in plan.command
    assert "-lobjc" in plan.command


@pytest.mark.parametrize(
    ("driver", "signs"),
    [(("clang",), True), (("clang", "-fuse-ld=lld"), False), (("zig", "cc"), False)],
)
def test_only_apple_ld_is_told_the_published_name_of_its_stage(
    monkeypatch, tmp_path: Path, driver: tuple[str, ...], signs: bool
) -> None:
    from molt.cli.native_link_plan import native_link_execution_command

    plan, output = _darwin_native_link_plan(monkeypatch, tmp_path, driver=driver)
    stage = output.with_name(".app.stage")
    with native_link_execution_command(
        plan, planned_output=output, execution_output=stage
    ) as command:
        # The stage replaces the -o operand; the signature still names the
        # published binary.
        assert command[command.index("-o") + 1] == str(stage)
        if signs:
            assert "-Wl,-adhoc_codesign" in command
            final = command.index("-final_output")
            assert command[final - 1] == command[final + 1] == "-Xlinker"
            assert command[final + 2] == str(output)
        else:
            assert "-final_output" not in command
            assert "-Wl,-adhoc_codesign" not in command


@pytest.mark.skipif(
    sys.platform != "darwin", reason="actual Apple ld and codesign oracle"
)
@pytest.mark.parametrize("arch", ["arm64", "x86_64"])
def test_apple_ld_signs_a_private_stage_as_the_published_binary(
    monkeypatch, tmp_path: Path, arch: str
) -> None:
    import platform

    from molt import file_publication
    from molt.cli import atomic_io, build_results
    from molt.cli.native_link_plan import (
        native_link_capabilities,
        native_link_output_flags,
    )
    from molt.native_artifact_header import read_macho_code_signature
    from molt.platform_toolchain import select_darwin_toolchain

    # This oracle tests Apple's linker across both Mach-O architectures. A
    # PATH-selected LLVM may intentionally contain only the host code generator.
    toolchain = select_darwin_toolchain(dict(os.environ))
    environment = {**os.environ, **toolchain.environment()}
    clang = str(toolchain.tools["clang"])
    target = resolve_native_target_spec(
        "aarch64-apple-darwin" if arch == "arm64" else "x86_64-apple-darwin"
    )
    flags = native_link_output_flags(
        capabilities=native_link_capabilities(target=target, linker_hint=None),
        output=tmp_path / "out" / "app",
    )
    output = tmp_path / "out" / "app"
    output.parent.mkdir()
    source = tmp_path / "main.c"
    source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    obj = tmp_path / "main.o"
    cold_timeout = 60  # Cold image execution shares its compile/link budget.
    run_cli_test_process(
        [clang, "-arch", arch, "-c", str(source), "-o", str(obj)],
        env=environment,
        check=True,
        timeout=cold_timeout,
    )
    stages = [
        file_publication.staged_file_path(output, purpose="native-link")
        for _ in range(2)
    ]
    for stage in stages:
        run_cli_test_process(
            [clang, "-arch", arch, str(obj), "-o", str(stage), *flags],
            env=environment,
            check=True,
            timeout=cold_timeout,
        )
    # The stage name never reaches the published bytes.
    assert stages[0].read_bytes() == stages[1].read_bytes()
    signature = read_macho_code_signature(stages[0])
    assert signature is not None
    assert signature.identifier == "app"
    assert signature.ad_hoc and signature.covers_image
    # Apple's own decoder and verifier are the oracle for both.
    described = run_cli_test_process(
        ["codesign", "-dv", str(stages[0])], check=True, timeout=30
    ).stderr
    assert "Identifier=app\n" in described
    assert f"flags=0x{signature.flags:x}(adhoc,linker-signed)" in described
    assert f"hashes={signature.code_slots}+" in described
    run_cli_test_process(
        ["codesign", "--verify", "--strict", str(stages[0])], check=True, timeout=30
    )

    def no_codesign(*_args: object) -> None:
        raise AssertionError("the linker signature already names the binary")

    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", no_codesign)
    assert (
        build_results._finalize_native_link_candidate(
            candidate=stages[0],
            output_binary=output,
            target_triple=target.triple,
            strip=False,
        )
        is None
    )
    stages[1].unlink()
    if platform.machine() == arch:
        run_cli_test_process([str(output)], check=True, timeout=cold_timeout)


def test_append_darwin_runtime_frameworks_skips_non_darwin_target() -> None:
    args = ["zig", "cc", "-target", "x86_64-unknown-linux-gnu"]
    NATIVE_TOOLCHAIN._append_darwin_runtime_frameworks(
        args, target=resolve_native_target_spec("x86_64-unknown-linux-gnu")
    )
    assert "-framework" not in args


@pytest.mark.skipif(
    sys.platform != "darwin", reason="actual Darwin compiler/linker oracle"
)
def test_native_compile_and_link_use_the_explicit_sdk(tmp_path, monkeypatch):
    from molt.platform_toolchain import select_darwin_sdk
    from molt.cli.source_extension_compiler_inputs import (
        source_extension_compiler_environment,
        darwin_compiler_commands,
    )
    from molt.cli.source_extension_target import resolve_source_extension_target_plan

    base_env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in {"SDKROOT", "MOLT_MACOSX_DEPLOYMENT_TARGET", "MACOSX_DEPLOYMENT_TARGET"}
    }
    installed = select_darwin_sdk(base_env)
    sdk = tmp_path / "SDK with sentinel header"
    (sdk / "usr/include").mkdir(parents=True)
    shutil.copyfile(installed.sdk_root / "SDKSettings.json", sdk / "SDKSettings.json")
    (sdk / "usr/lib").symlink_to(
        installed.sdk_root / "usr/lib", target_is_directory=True
    )
    (sdk / "System").symlink_to(installed.sdk_root / "System", target_is_directory=True)
    (sdk / "usr/include/molt_sdk_contract.h").write_text(
        "#define MOLT_SDK_CONTRACT 73\n", encoding="utf-8"
    )
    monkeypatch.setenv("SDKROOT", str(sdk))
    monkeypatch.delenv("CFLAGS", raising=False)
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    target = resolve_source_extension_target_plan("native")
    assert target.native_target is not None
    environment = source_extension_compiler_environment(os.environ)
    command, _, _ = native_link_command._build_native_link_driver_command(
        output_obj=None, target=target.native_target, sysroot_path=None, profile="dev"
    )
    assert command[command.index("-isysroot") + 1] == environment["SDKROOT"] == str(sdk)
    source = tmp_path / "sdk_contract.c"
    source.write_text(
        "#include <molt_sdk_contract.h>\nint main(void) { return MOLT_SDK_CONTRACT != 73; }\n",
        encoding="utf-8",
    )
    binary = tmp_path / "sdk_contract"
    # Captured native/Meson compiler commands must retain the SDK even when
    # their subprocess environment carries none of the selection variables.
    compile_command = darwin_compiler_commands(
        {"c": (command[0], "-Xclang", "-isysroot", "-Xclang", str(sdk))},
        environment=environment,
        arch=target.native_target.arch,
    )["c"]
    assert "-Xclang" not in compile_command
    assert compile_command[compile_command.index("-isysroot") + 1] == str(sdk)
    from molt.cli.source_extension_toolchain import _meson_native_text
    import ast

    native_file = _meson_native_text(commands={"c": compile_command})
    captured_command = ast.literal_eval(
        next(
            line.removeprefix("c = ")
            for line in native_file.splitlines()
            if line.startswith("c = ")
        )
    )
    clean_environment = {
        key: value
        for key, value in environment.items()
        if key
        not in {
            "SDKROOT",
            "DEVELOPER_DIR",
            "MACOSX_DEPLOYMENT_TARGET",
            "MOLT_MACOSX_DEPLOYMENT_TARGET",
        }
    }
    # Clang's JoinedOrSeparate -isysroot grammar keeps '=' in the operand;
    # it is not an equals alias. Use actual header lookup as the oracle.
    cold_timeout = 60  # Cold image execution shares its compile/link budget.
    rejected = run_cli_test_process(
        [
            command[0],
            f"-isysroot={sdk}",
            "-c",
            str(source),
            "-o",
            str(tmp_path / "wrong.o"),
        ],
        env=clean_environment,
        check=False,
        text=True,
        timeout=cold_timeout,
    )
    assert rejected.returncode != 0
    assert (
        "no such sysroot directory" in rejected.stderr
        and "=" + str(sdk) in rejected.stderr
    )
    obj = tmp_path / "sdk_contract.o"
    run_cli_test_process(
        [*captured_command, "-c", str(source), "-o", str(obj)],
        env=clean_environment,
        check=True,
        text=True,
        timeout=cold_timeout,
    )
    run_cli_test_process(
        [*command, str(obj), "-o", str(binary)],
        env=clean_environment,
        check=True,
        text=True,
        timeout=cold_timeout,
    )
    run_cli_test_process([str(binary)], check=True, timeout=cold_timeout)


@pytest.mark.parametrize("selector", ["argument", "cflags"])
@pytest.mark.parametrize("symlink", [False, True])
def test_cross_darwin_search_paths_share_the_explicit_sysroot(
    tmp_path, monkeypatch, selector, symlink
):
    sdk = write_test_darwin_sdk(tmp_path / "selected_sdk")
    selected_sdk = sdk
    if symlink:
        selected_sdk = tmp_path / "SDK alias"
        try:
            selected_sdk.symlink_to(sdk, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"symlink capability unavailable: {exc}")
    driver = tmp_path / "cross-clang"
    driver.write_bytes(b"unexecuted command-plan fixture")
    monkeypatch.setenv("MOLT_CROSS_CC", str(driver))
    monkeypatch.setenv("SDKROOT", str(tmp_path / "wrong_ambient_sdk"))
    monkeypatch.delenv("DEVELOPER_DIR", raising=False)
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.setenv(
        "CFLAGS", "" if selector == "argument" else f'-isysroot "{selected_sdk}"'
    )
    monkeypatch.setattr(
        native_link_command, "validate_compiler_target", lambda *_: None
    )
    monkeypatch.setattr(
        native_link_command, "_resolve_native_linker_hint", lambda **_: None
    )
    command, _, _ = native_link_command._build_native_link_driver_command(
        output_obj=None,
        target=resolve_native_target_spec(
            "aarch64-apple-darwin", host_platform="linux", host_arch="x86_64"
        ),
        sysroot_path=selected_sdk if selector == "argument" else None,
        profile="dev",
        host_platform="linux",
        host_arch="x86_64",
    )
    assert command[command.index("-isysroot") + 1] == str(sdk)
    assert command[command.index("-F") + 1] == str(sdk / "System/Library/Frameworks")
    assert command[command.index("-L") + 1] == str(sdk / "usr/lib")
    assert "-mmacosx-version-min=15.4" in command
    assert not any("wrong_ambient_sdk" in arg for arg in command)
