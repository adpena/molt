from __future__ import annotations

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
