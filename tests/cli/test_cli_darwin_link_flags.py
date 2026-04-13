from __future__ import annotations

from pathlib import Path

import pytest

import molt.cli as cli


def test_append_darwin_runtime_frameworks_for_host_darwin(
    monkeypatch,
) -> None:
    monkeypatch.setattr(cli.sys, "platform", "darwin")
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    args = ["clang", "-lc++"]
    cli._append_darwin_runtime_frameworks(args, target_triple=None)
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


def test_append_darwin_runtime_frameworks_for_cross_target() -> None:
    import os
    os.environ.pop("MOLT_RUNTIME_GPU_METAL", None)
    args = ["zig", "cc", "-target", "aarch64-macos"]
    cli._append_darwin_runtime_frameworks(args, target_triple="aarch64-apple-darwin")
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


def test_append_darwin_runtime_frameworks_adds_metal_when_enabled(
    monkeypatch,
) -> None:
    monkeypatch.setattr(cli.sys, "platform", "darwin")
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    args = ["clang", "-lc++"]
    cli._append_darwin_runtime_frameworks(args, target_triple=None)
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
    monkeypatch.setattr(cli.sys, "platform", "darwin")
    monkeypatch.setenv("MOLT_RUNTIME_GPU_WEBGPU", "1")
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    args = ["clang", "-lc++"]
    cli._append_darwin_runtime_frameworks(args, target_triple=None)
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
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    runtime_lib.write_bytes(b"")
    build_output = runtime_lib.parent / "build" / "wgpu-sys" / "output"
    build_output.parent.mkdir(parents=True)
    build_output.write_text(
        "\n".join(
            [
                "cargo:rustc-link-lib=framework=Metal",
                "cargo:rustc-link-lib=framework=Foundation",
                "cargo:rustc-link-lib=dylib=c++",
                "cargo:rustc-link-search=framework=/tmp/fwk",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    search_paths, link_libs = cli._collect_cargo_native_link_deps(runtime_lib)

    assert "-L/tmp/fwk" in search_paths
    assert "-framework" in link_libs
    assert "Metal" in link_libs
    assert "Foundation" in link_libs
    assert "-lc++" in link_libs


def test_build_native_link_command_includes_metal_frameworks_when_runtime_gpu_metal_enabled(
    monkeypatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(cli.sys, "platform", "darwin")
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    runtime_lib.write_bytes(b"!<arch>\nfake-staticlib")
    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    stub_path = tmp_path / "main_stub.c"
    stub_path.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    output_binary = tmp_path / "app"

    cmd, _, _ = cli._build_native_link_command(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=output_binary,
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        stdlib_obj_path=None,
    )

    assert "-framework" in cmd
    assert "Metal" in cmd
    assert "-lobjc" in cmd


def test_append_darwin_runtime_frameworks_skips_non_darwin_target() -> None:
    args = ["zig", "cc", "-target", "x86_64-unknown-linux-gnu"]
    cli._append_darwin_runtime_frameworks(
        args, target_triple="x86_64-unknown-linux-gnu"
    )
    assert "-framework" not in args


def test_detect_macos_deployment_target_prefers_molt_env(monkeypatch) -> None:
    monkeypatch.setenv("MOLT_MACOSX_DEPLOYMENT_TARGET", "13.3")
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    assert cli._detect_macos_deployment_target("arm64") == "13.3"


def test_detect_macos_deployment_target_prefers_standard_env(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.setenv("MACOSX_DEPLOYMENT_TARGET", "12.7")
    assert cli._detect_macos_deployment_target("arm64") == "12.7"


@pytest.mark.parametrize(
    ("arch", "expected"),
    [
        ("x86_64", "10.13"),
        ("amd64", "10.13"),
    ],
)
def test_detect_macos_deployment_target_uses_stable_arch_baseline(
    monkeypatch, arch: str, expected: str
) -> None:
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    assert cli._detect_macos_deployment_target(arch) == expected


def test_detect_macos_deployment_target_arm64_uses_sdk_version(
    monkeypatch,
) -> None:
    """arm64/aarch64/unknown arches use the SDK version (xcrun --show-sdk-version)."""
    import subprocess

    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    try:
        expected = subprocess.check_output(
            ["xcrun", "--show-sdk-version"], text=True, timeout=5
        ).strip()
    except (subprocess.SubprocessError, FileNotFoundError):
        import platform
        ver = platform.mac_ver()[0]
        parts = ver.split(".")
        expected = ".".join(parts[:2]) if len(parts) >= 2 else ver
    for arch in ("arm64", "aarch64", "mystery"):
        result = cli._detect_macos_deployment_target(arch)
        assert result == expected, f"arch={arch}: got {result}, expected {expected}"
