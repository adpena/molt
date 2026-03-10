from __future__ import annotations

import pytest

import molt.cli as cli


def test_append_darwin_runtime_frameworks_for_host_darwin(
    monkeypatch,
) -> None:
    monkeypatch.setattr(cli.sys, "platform", "darwin")
    args = ["clang", "-lc++"]
    cli._append_darwin_runtime_frameworks(args, target_triple=None)
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


def test_append_darwin_runtime_frameworks_for_cross_target() -> None:
    args = ["zig", "cc", "-target", "aarch64-macos"]
    cli._append_darwin_runtime_frameworks(args, target_triple="aarch64-apple-darwin")
    assert args[-4:] == ["-framework", "Security", "-framework", "CoreFoundation"]


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
        ("arm64", "11.0"),
        ("aarch64", "11.0"),
        ("x86_64", "10.13"),
        ("amd64", "10.13"),
        ("mystery", "11.0"),
    ],
)
def test_detect_macos_deployment_target_uses_stable_arch_baseline(
    monkeypatch, arch: str, expected: str
) -> None:
    monkeypatch.delenv("MOLT_MACOSX_DEPLOYMENT_TARGET", raising=False)
    monkeypatch.delenv("MACOSX_DEPLOYMENT_TARGET", raising=False)
    assert cli._detect_macos_deployment_target(arch) == expected
