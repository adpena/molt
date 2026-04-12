from __future__ import annotations

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
