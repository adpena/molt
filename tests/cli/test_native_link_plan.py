from __future__ import annotations

from dataclasses import FrozenInstanceError
from pathlib import Path
import subprocess
import sys

import pytest

import molt.cli as cli
from molt.cli import build_results, native_link_command, source_extensions
from molt.cli.native_link_plan import NativeObjectFormat


def _plan(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    host_platform: str,
    host_arch: str = "x86_64",
    profile: str = "release",
    linker: str | None = None,
    bolt_requested: bool = False,
    cc: str = "clang",
):
    output_obj = tmp_path / "output.o"
    stub_path = tmp_path / "main_stub.c"
    runtime_lib = tmp_path / "libmolt_runtime.a"
    for path in (output_obj, stub_path, runtime_lib):
        path.write_bytes(b"x")

    monkeypatch.setattr(native_link_command.sys, "platform", host_platform)
    monkeypatch.setattr(native_link_command.platform, "machine", lambda: host_arch)
    monkeypatch.setenv("CC", cc)
    monkeypatch.delenv("MOLT_KEEP_SYMBOLS", raising=False)
    monkeypatch.setattr(
        native_link_command,
        "_resolve_native_linker_hint",
        lambda **_kwargs: linker,
    )
    monkeypatch.setattr(
        native_link_command,
        "_collect_cargo_native_link_deps",
        lambda _runtime_lib: ([], []),
    )
    monkeypatch.setattr(
        native_link_command,
        "_native_windows_system_link_libs",
        lambda _target: [],
    )
    monkeypatch.setattr(
        native_link_command,
        "_append_darwin_runtime_frameworks",
        lambda _command, *, target_triple: None,
    )
    return native_link_command._build_native_link_plan(
        output_obj=output_obj,
        stub_path=stub_path,
        runtime_lib=runtime_lib,
        output_binary=tmp_path / "app",
        target_triple=None,
        sysroot_path=None,
        profile=profile,
        bolt_requested=bolt_requested,
    )


def test_link_plan_is_immutable_and_preserves_elf_function_identity(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        linker="lld",
    )

    assert plan.target.object_format is NativeObjectFormat.ELF
    assert plan.policy.preserve_function_identity
    assert "-Wl,--icf=none" in plan.command
    assert "-Wl,--strip-all" not in plan.command
    assert plan.policy.strip_after_link
    with pytest.raises(FrozenInstanceError):
        plan.linker_hint = None  # type: ignore[misc]


def test_macho_plan_preserves_identity_without_suppressing_warnings(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="darwin")

    assert "-Wl,-no_deduplicate" in plan.command
    assert "-Wl,-w" not in plan.command
    assert "-Wl,-x" not in plan.command
    assert "-Wl,-S" not in plan.command


def test_explicit_driver_linker_selection_gets_matching_capability_policy(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        cc="clang -fuse-ld=/opt/llvm/bin/ld.lld",
    )

    assert plan.linker_hint == "lld"
    assert "-Wl,--icf=none" in plan.command


def test_fast_linker_auto_detection_does_not_leak_across_target_formats(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(native_link_command.sys, "platform", "darwin")
    monkeypatch.delenv("MOLT_DEV_LINKER", raising=False)
    monkeypatch.setattr(
        native_link_command,
        "_resolve_available_fast_linker",
        lambda: "mold",
    )

    assert (
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target_triple=None,
        )
        is None
    )


def test_explicit_mold_non_elf_selection_fails_before_link(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(native_link_command.sys, "platform", "darwin")
    monkeypatch.setenv("MOLT_DEV_LINKER", "mold")

    with pytest.raises(RuntimeError, match="Linux ELF"):
        native_link_command._resolve_native_linker_hint(
            profile="dev",
            target_triple=None,
        )


def test_coff_plan_explicitly_disables_icf(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(monkeypatch, tmp_path, host_platform="win32", host_arch="AMD64")

    assert "-Wl,/OPT:REF" in plan.command
    assert "-Wl,/OPT:NOICF" in plan.command
    assert not plan.policy.strip_after_link


def test_bolt_release_plan_emits_relocations_and_defers_stripping(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    plan = _plan(
        monkeypatch,
        tmp_path,
        host_platform="linux",
        linker="mold",
        bolt_requested=True,
    )

    assert "-Wl,--emit-relocs" in plan.command
    assert "-Wl,--strip-all" not in plan.command
    assert not plan.policy.strip_after_link
    assert plan.policy.bolt_requested


@pytest.mark.parametrize(
    ("host_platform", "host_arch", "profile", "message"),
    [
        ("win32", "AMD64", "release", "Linux ELF"),
        ("darwin", "arm64", "release", "Linux ELF"),
        ("linux", "riscv64", "release", "x86_64 and aarch64"),
        ("linux", "x86_64", "dev", "release build profile"),
    ],
)
def test_bolt_unsupported_cells_fail_during_link_planning(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    host_platform: str,
    host_arch: str,
    profile: str,
    message: str,
) -> None:
    with pytest.raises(RuntimeError, match=message):
        _plan(
            monkeypatch,
            tmp_path,
            host_platform=host_platform,
            host_arch=host_arch,
            profile=profile,
            bolt_requested=True,
        )


def test_cli_threads_bolt_intent_into_the_single_build_pipeline(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    entry = tmp_path / "main.py"
    entry.write_text("print('ok')\n", encoding="utf-8")
    received: dict[str, object] = {}

    def fake_build(*_args: object, **kwargs: object) -> int:
        received.update(kwargs)
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "molt",
            "build",
            "--bolt",
            "--bolt-training-cmd",
            "{binary} --train",
            str(entry),
        ],
    )

    assert cli.main() == 0
    assert received["bolt"] is True
    assert received["bolt_training_cmd"] == "{binary} --train"


def test_planned_release_strip_failure_is_loud(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    monkeypatch.setattr(build_results.sys, "platform", "linux")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("/usr/bin/strip"),),
    )
    monkeypatch.setattr(
        build_results,
        "_run_completed_command",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(
            ["strip"], 1, "", "unsupported relocation"
        ),
    )

    assert build_results._post_link_strip(binary, None) == (
        "post-link strip failed: unsupported relocation"
    )


def test_cross_target_strip_requires_target_capable_llvm_strip(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    monkeypatch.setattr(build_results.sys, "platform", "win32")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "AMD64")
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("C:/Windows/System32/strip.exe"),),
    )

    assert build_results._post_link_strip(
        binary, "aarch64-unknown-linux-gnu"
    ) == (
        "post-link target-capable llvm-strip is unavailable for linux/aarch64"
    )


def test_cross_target_strip_uses_llvm_strip_and_target_format_flags(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    binary = tmp_path / "app"
    binary.write_bytes(b"ELF")
    llvm_strip = tmp_path / "llvm-strip.exe"
    llvm_strip.write_bytes(b"tool")
    received: list[list[str]] = []
    monkeypatch.setattr(build_results.sys, "platform", "win32")
    monkeypatch.setattr(build_results.platform, "machine", lambda: "AMD64")
    monkeypatch.setattr(
        build_results,
        "llvm_tool_candidates",
        lambda _role: (Path("C:/Windows/System32/strip.exe"), llvm_strip),
    )

    def fake_run(command, **_kwargs):
        received.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(build_results, "_run_completed_command", fake_run)

    assert (
        build_results._post_link_strip(binary, "aarch64-unknown-linux-gnu") is None
    )
    assert received == [[str(llvm_strip), "--strip-all", str(binary)]]


def test_native_candidate_is_finalized_before_atomic_publication(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    candidate = tmp_path / ".app.link-candidate"
    output = tmp_path / "app"
    candidate.write_bytes(b"linked")
    output.write_bytes(b"previous")
    events: list[str] = []

    def fake_strip(path: Path, _target: str | None) -> None:
        assert path == candidate
        events.append("strip")
        path.write_bytes(b"stripped")
        return None

    def fake_validate(path: Path, _target: str | None) -> None:
        assert path.read_bytes() == b"stripped"
        events.append("validate")

    def fake_publish(path: Path, destination: Path, *, codesign: bool) -> None:
        assert codesign
        assert path.read_bytes() == b"stripped"
        assert destination.read_bytes() == b"previous"
        events.append("publish")
        destination.write_bytes(path.read_bytes())

    monkeypatch.setattr(build_results, "_post_link_strip", fake_strip)
    monkeypatch.setattr(build_results, "_assert_native_binary_valid", fake_validate)
    monkeypatch.setattr(build_results, "_atomic_copy_file", fake_publish)

    assert (
        build_results._finalize_native_link_candidate(
            candidate=candidate,
            output_binary=output,
            target_triple=None,
            strip=True,
        )
        is None
    )
    assert events == ["strip", "validate", "publish"]
    assert output.read_bytes() == b"stripped"
    assert not candidate.exists()


def test_native_candidate_failure_preserves_previous_published_artifact(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    candidate = tmp_path / ".app.link-candidate"
    output = tmp_path / "app"
    candidate.write_bytes(b"linked")
    output.write_bytes(b"previous")
    monkeypatch.setattr(
        build_results,
        "_post_link_strip",
        lambda *_args, **_kwargs: "unsupported target relocation",
    )

    assert build_results._finalize_native_link_candidate(
        candidate=candidate,
        output_binary=output,
        target_triple="aarch64-unknown-linux-gnu",
        strip=True,
    ) == "unsupported target relocation"
    assert output.read_bytes() == b"previous"
    assert candidate.read_bytes() == b"linked"


@pytest.mark.parametrize(
    ("host_platform", "cc", "expected"),
    [
        ("win32", ("clang-cl",), ("/link", "/OPT:REF", "/OPT:NOICF")),
        (
            "linux",
            ("clang", "-fuse-ld=lld"),
            ("-Wl,--gc-sections", "-Wl,--icf=none"),
        ),
        (
            "darwin",
            ("clang",),
            ("-Wl,-dead_strip", "-Wl,-no_deduplicate"),
        ),
    ],
)
def test_extension_links_share_native_identity_and_dead_strip_policy(
    monkeypatch: pytest.MonkeyPatch,
    host_platform: str,
    cc: tuple[str, ...],
    expected: tuple[str, ...],
) -> None:
    monkeypatch.setattr(source_extensions.sys, "platform", host_platform)
    monkeypatch.setattr(source_extensions.platform, "machine", lambda: "x86_64")

    assert tuple(
        source_extensions._source_extension_link_policy_args(
            cc_cmd=cc,
            target_triple=None,
        )
    ) == expected
