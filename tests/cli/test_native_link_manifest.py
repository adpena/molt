from __future__ import annotations

import contextlib
from collections.abc import Mapping
from concurrent.futures import ThreadPoolExecutor
import inspect
import json
import os
from pathlib import Path
import shutil
import subprocess

import pytest

from molt.cli import native_link_deps
from molt.cli import native_link_manifest
from molt.cli import runtime_build
from molt.cli import cargo_execution
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    manifest_from_cargo_json,
    native_link_dependency_manifest_path,
    native_link_flags_from_manifest,
    read_native_link_dependency_manifest,
    read_native_link_flags,
    write_native_link_dependency_manifest,
)
from tests.cli.process_guard import run_cli_test_process


_SOURCE_FINGERPRINT = {
    "hash": "1" * 64,
    "inputs_digest": "2" * 64,
    "meta_digest": "3" * 64,
    "rustc": "rustc 1.91.0",
}


def _source_args(root: Path) -> dict[str, object]:
    return {
        "source_root": root,
        "source_fingerprint": _SOURCE_FINGERPRINT,
    }


def _cargo_message(
    package_id: str,
    out_dir: str,
    *,
    linked_paths: list[str],
    linked_libs: list[str],
) -> str:
    return json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": package_id,
            "linked_libs": linked_libs,
            "linked_paths": linked_paths,
            "cfgs": [],
            "env": [],
            "out_dir": out_dir,
        }
    )


def _cargo_output(cargo_stdout: str, native_arguments: str = "") -> str:
    native_note = json.dumps(
        {
            "reason": "compiler-message",
            "message": {
                "message": f"native-static-libs: {native_arguments}",
                "level": "note",
                "code": None,
                "spans": [],
                "children": [],
                "rendered": None,
            },
        }
    )
    return f"{cargo_stdout}\n{native_note}" if cargo_stdout else native_note


def test_manifest_keeps_exact_package_versions_and_canonical_windows_forms(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "target" / "dev-fast" / "molt_runtime.lib"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime-v1")
    libs_v2 = tmp_path / "libs-v2"
    libs_v1 = tmp_path / "libs-v1"
    raw_libs = tmp_path / "raw-libs"
    frameworks = tmp_path / "Frameworks"
    for directory in (libs_v2, libs_v1, raw_libs, frameworks):
        directory.mkdir()
    (frameworks / "Metal.framework").mkdir()
    (frameworks / "QuartzCore.framework").mkdir()
    out_v2 = tmp_path / "out-v2"
    out_v1 = tmp_path / "out-v1"
    out_v2.mkdir()
    out_v1.mkdir()
    (libs_v2 / "exact.lib").write_bytes(b"exact")
    (libs_v2 / "libexternal_static.a").write_bytes(b"static")
    messages = "\n".join(
        [
            json.dumps({"reason": "compiler-artifact", "package_id": "runtime"}),
            _cargo_message(
                "registry+https://example.invalid#index-sys@2.0.0",
                str(out_v2),
                linked_paths=[f"native={libs_v2}", str(raw_libs)],
                linked_libs=[
                    "dylib=psapi",
                    "raw-dylib=kernel32",
                    "dylib:+verbatim=exact.lib",
                    "static=bundled",
                    "static:-bundle=external_static",
                ],
            ),
            _cargo_message(
                "registry+https://example.invalid#index-sys@1.0.0",
                str(out_v1),
                linked_paths=[f"framework={frameworks}", f"native={libs_v1}"],
                linked_libs=["framework=Metal", "weak_framework=QuartzCore"],
            ),
        ]
    )
    path = write_native_link_dependency_manifest(
        _cargo_output(
            messages,
            "-lpsapi -lkernel32 -lexternal_static "
            "-framework Metal -weak_framework QuartzCore",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_source_args(tmp_path),
    )
    manifest = read_native_link_dependency_manifest(runtime, target_triple=None)
    assert path == native_link_dependency_manifest_path(runtime)
    assert [item["package_id"] for item in manifest["build_scripts"]] == [
        "registry+https://example.invalid#index-sys@1.0.0",
        "registry+https://example.invalid#index-sys@2.0.0",
    ]
    assert native_link_flags_from_manifest(manifest, object_format="macho") == [
        "-lpsapi",
        "-lkernel32",
        f"-L{libs_v2}",
        "-lexternal_static",
        f"-F{frameworks}",
        "-framework",
        "Metal",
        f"-F{frameworks}",
        "-weak_framework",
        "QuartzCore",
    ]


def test_manifest_is_deterministic_without_reordering_semantic_messages(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "release-output" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    out_dir = tmp_path / "crate-out"
    other_out_dir = tmp_path / "other-out"
    lib_dir = tmp_path / "lib"
    different_lib_dir = tmp_path / "different-lib"
    for directory in (out_dir, other_out_dir, lib_dir, different_lib_dir):
        directory.mkdir()
    message = _cargo_message(
        "path+file:///repo#crate@1.0.0",
        str(out_dir),
        linked_paths=[f"native={lib_dir}", f"native={lib_dir}"],
        linked_libs=["dylib=z", "dylib=z"],
    )
    other = _cargo_message(
        "path+file:///repo#crate@1.0.0",
        str(other_out_dir),
        linked_paths=[f"native={different_lib_dir}"],
        linked_libs=["dylib=other"],
    )
    first = manifest_from_cargo_json(
        _cargo_output(f"{message}\n{other}", "-lz -lz -lother"),
        runtime_lib=runtime,
        cargo_profile="release-output",
        target_triple="aarch64-unknown-linux-gnu",
        **_source_args(tmp_path),
    )
    second = manifest_from_cargo_json(
        _cargo_output(f"{other}\n{message}", "-lz -lz -lother"),
        runtime_lib=runtime,
        cargo_profile="release-output",
        target_triple="aarch64-unknown-linux-gnu",
        **_source_args(tmp_path),
    )
    assert first == second
    assert first["native_static_libs"]["arguments"] == ["-lz", "-lz", "-lother"]
    deduped = manifest_from_cargo_json(
        _cargo_output(f"{message}\n{message}", "-lz -lz"),
        runtime_lib=runtime,
        cargo_profile="release-output",
        target_triple="aarch64-unknown-linux-gnu",
        **_source_args(tmp_path),
    )
    assert len(deduped["build_scripts"]) == 1


def test_manifest_rejects_archive_target_profile_and_json_drift(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    write_native_link_dependency_manifest(
        _cargo_output(""),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_source_args(tmp_path),
    )
    with pytest.raises(NativeLinkDependencyManifestError, match="target mismatch"):
        read_native_link_dependency_manifest(
            runtime,
            target_triple="x86_64-unknown-linux-gnu",
        )
    with pytest.raises(
        NativeLinkDependencyManifestError, match="Cargo profile mismatch"
    ):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            cargo_profile="release",
        )

    runtime.write_bytes(b"runtime-mutated")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="archive digest mismatch"
    ):
        read_native_link_dependency_manifest(runtime, target_triple=None)

    manifest_path = native_link_dependency_manifest_path(runtime)
    manifest_path.write_bytes(b"\xff")
    with pytest.raises(NativeLinkDependencyManifestError, match="cannot read"):
        read_native_link_dependency_manifest(runtime, target_triple=None)
    manifest_path.write_text('{"kind":"a","kind":"b"}', encoding="utf-8")
    with pytest.raises(NativeLinkDependencyManifestError, match="duplicate JSON key"):
        read_native_link_dependency_manifest(runtime, target_triple=None)

    with pytest.raises(
        NativeLinkDependencyManifestError, match="out_dir must be absolute"
    ):
        manifest_from_cargo_json(
            _cargo_output(
                _cargo_message(
                    "registry#dep@1.0.0",
                    "relative/out",
                    linked_paths=[],
                    linked_libs=[],
                )
            ),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=None,
            **_source_args(tmp_path),
        )


def test_manifest_not_name_matching_selects_exact_build_script_instance(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    exact_out = tmp_path / "exact-out"
    exact_lib = tmp_path / "exact-lib"
    exact_out.mkdir()
    exact_lib.mkdir()
    for directory, lib in (
        ("same-sys-oldconfig", "stale_old"),
        ("same-sys-newconfig", "stale_new"),
    ):
        output = runtime.parent / "build" / directory / "output"
        output.parent.mkdir(parents=True)
        output.write_text(f"cargo:rustc-link-lib={lib}\n", encoding="utf-8")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "registry+https://example.invalid#same-sys@2.0.0",
                str(exact_out),
                linked_paths=[f"native={exact_lib}"],
                linked_libs=["dylib=exact"],
            ),
            "-lexact",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_source_args(tmp_path),
    )

    assert native_link_deps._collect_cargo_native_link_deps(
        runtime,
        object_format="elf",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    ) == ["-lexact"]


def test_hydrated_byte_identical_artifact_requires_matching_sidecar(
    tmp_path: Path,
) -> None:
    provider = tmp_path / "provider" / "dev-fast" / "libmolt_runtime.a"
    consumer = tmp_path / "consumer" / "dev-fast" / "libmolt_runtime.a"
    provider.parent.mkdir(parents=True)
    consumer.parent.mkdir(parents=True)
    provider.write_bytes(b"same-runtime")
    provider_out = tmp_path / "provider-out"
    provider_lib = tmp_path / "provider-lib"
    provider_out.mkdir()
    provider_lib.mkdir()
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "path+file:///repo#dep@1.0.0",
                str(provider_out),
                linked_paths=[f"native={provider_lib}"],
                linked_libs=["dylib=provider"],
            ),
            "-lprovider",
        ),
        runtime_lib=provider,
        cargo_profile="dev-fast",
        target_triple=None,
        **_source_args(tmp_path),
    )
    shutil.copyfile(provider, consumer)
    with pytest.raises(NativeLinkDependencyManifestError, match="cannot read"):
        read_native_link_dependency_manifest(consumer, target_triple=None)
    shutil.copyfile(
        native_link_dependency_manifest_path(provider),
        native_link_dependency_manifest_path(consumer),
    )
    assert read_native_link_dependency_manifest(
        consumer,
        target_triple=None,
        cargo_profile="dev-fast",
    )["runtime"]["sha256"]


def test_manifest_replay_rejects_stale_paths_and_pruned_workspaces(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    out_dir = tmp_path / "target" / "build" / "dep" / "out"
    linked_dir = tmp_path / "target" / "native"
    runtime = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    for directory in (workspace, out_dir, linked_dir, runtime.parent):
        directory.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"runtime")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "registry#dep@1.0.0",
                str(out_dir),
                linked_paths=[f"native={linked_dir}"],
                linked_libs=["dylib=dep"],
            ),
            "-ldep",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        source_root=workspace,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    read_native_link_dependency_manifest(
        runtime,
        target_triple=None,
        source_root=workspace,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )

    shutil.rmtree(linked_dir)
    with pytest.raises(NativeLinkDependencyManifestError, match="existing directory"):
        read_native_link_dependency_manifest(runtime, target_triple=None)
    linked_dir.mkdir()
    shutil.rmtree(workspace)
    with pytest.raises(
        NativeLinkDependencyManifestError, match="cannot resolve expected source"
    ):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=workspace,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )


def test_hydrated_manifest_is_worktree_neutral_but_refuses_foreign_fingerprint(
    tmp_path: Path,
) -> None:
    workspace_a = tmp_path / "workspace-a"
    workspace_b = tmp_path / "workspace-b"
    out_dir = tmp_path / "target" / "build" / "dep" / "out"
    linked_dir = tmp_path / "target" / "native"
    provider = tmp_path / "provider" / "dev-fast" / "libmolt_runtime.a"
    consumer = tmp_path / "consumer" / "dev-fast" / "libmolt_runtime.a"
    for directory in (
        workspace_a,
        workspace_b,
        out_dir,
        linked_dir,
        provider.parent,
        consumer.parent,
    ):
        directory.mkdir(parents=True, exist_ok=True)
    provider.write_bytes(b"same-runtime")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "registry#dep@1.0.0",
                str(out_dir),
                linked_paths=[f"native={linked_dir}"],
                linked_libs=["dylib=dep"],
            ),
            "-ldep",
        ),
        runtime_lib=provider,
        cargo_profile="dev-fast",
        target_triple=None,
        source_root=workspace_a,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    shutil.copyfile(provider, consumer)
    shutil.copyfile(
        native_link_dependency_manifest_path(provider),
        native_link_dependency_manifest_path(consumer),
    )
    hydrated = read_native_link_dependency_manifest(
        consumer,
        target_triple=None,
        source_root=workspace_b,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    assert hydrated["source"] == {"fingerprint": _SOURCE_FINGERPRINT}
    changed_fingerprint = {**_SOURCE_FINGERPRINT, "hash": "9" * 64}
    with pytest.raises(NativeLinkDependencyManifestError, match="fingerprint mismatch"):
        read_native_link_dependency_manifest(
            consumer,
            target_triple=None,
            source_root=workspace_a,
            source_fingerprint=changed_fingerprint,
        )


def test_equivalent_worktrees_share_one_semantic_manifest_without_writer_drift(
    tmp_path: Path,
) -> None:
    workspace_a = tmp_path / "workspace-a"
    workspace_b = tmp_path / "workspace-b"
    runtime = tmp_path / "shared" / "dev-fast" / "libmolt_runtime.a"
    for directory in (workspace_a, workspace_b, runtime.parent):
        directory.mkdir(parents=True)
    runtime.write_bytes(b"shared-runtime")

    def publish(source_root: Path) -> Path:
        return write_native_link_dependency_manifest(
            "",
            cargo_stderr="note: native-static-libs: -lc\n",
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=None,
            source_root=source_root,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )

    path = publish(workspace_a)
    first = path.read_bytes()
    publish(workspace_b)
    final = path.read_bytes()
    assert final == first

    def consume(source_root: Path) -> Mapping[str, object]:
        return read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=source_root,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )

    roots = [workspace_a, workspace_b] * 8
    with ThreadPoolExecutor(max_workers=4) as executor:
        snapshots = list(executor.map(consume, roots))

    assert str(workspace_a).encode() not in final
    assert str(workspace_b).encode() not in final
    assert all(
        snapshot["source"] == {"fingerprint": _SOURCE_FINGERPRINT}
        for snapshot in snapshots
    )


def test_native_static_lib_note_is_captured_from_exact_rustc_stderr(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.lib"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    manifest = manifest_from_cargo_json(
        "",
        cargo_stderr=(
            "Compiling molt-runtime\n"
            "note: native-static-libs: user32.lib advapi32.lib user32.lib\n"
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple="x86_64-pc-windows-msvc",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    assert manifest["native_static_libs"]["arguments"] == [
        "user32.lib",
        "advapi32.lib",
        "user32.lib",
    ]


def test_multiple_native_static_lib_notes_fail_closed(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.lib"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="exactly one native-static-libs"
    ):
        manifest_from_cargo_json(
            _cargo_output("", "user32.lib"),
            cargo_stderr="note: native-static-libs: advapi32.lib\n",
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple="x86_64-pc-windows-msvc",
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )


@pytest.mark.parametrize(
    ("field", "invalid"),
    [
        ("hash", "A" * 64),
        ("meta_digest", "not-a-digest"),
        ("inputs_digest", "0" * 63),
    ],
)
def test_manifest_rejects_non_sha256_source_authority(
    tmp_path: Path, field: str, invalid: str
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    fingerprint = {**_SOURCE_FINGERPRINT, field: invalid}
    with pytest.raises(
        NativeLinkDependencyManifestError, match="lowercase SHA-256 digest"
    ):
        manifest_from_cargo_json(
            _cargo_output("", "-lc"),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=None,
            source_root=tmp_path,
            source_fingerprint=fingerprint,
        )


def test_manifest_reader_rejects_mutated_source_digest(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    path = write_native_link_dependency_manifest(
        _cargo_output("", "-lc"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["source"]["fingerprint"]["hash"] = "G" * 64
    path.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="lowercase SHA-256 digest"
    ):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )


def test_runtime_digest_cache_reuses_identity_and_rejects_same_size_replacement(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime-a")
    write_native_link_dependency_manifest(
        _cargo_output("", "-lc"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )

    native_link_manifest._artifact_digest_for_identity.cache_clear()
    real_sha256_file = native_link_manifest._sha256_file
    hash_calls: list[Path] = []

    def counting_sha256_file(path: Path) -> str:
        hash_calls.append(path)
        return real_sha256_file(path)

    monkeypatch.setattr(native_link_manifest, "_sha256_file", counting_sha256_file)
    for _ in range(3):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )
    assert hash_calls == [runtime.resolve()]

    original = runtime.stat()
    replacement = runtime.with_suffix(".replacement")
    replacement.write_bytes(b"runtime-b")
    os.utime(replacement, ns=(original.st_atime_ns, original.st_mtime_ns))
    os.replace(replacement, runtime)
    with pytest.raises(NativeLinkDependencyManifestError, match="archive digest mismatch"):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )
    assert hash_calls == [runtime.resolve(), runtime.resolve()]
    native_link_manifest._artifact_digest_for_identity.cache_clear()


def test_rustc_native_static_lib_order_is_replayed_exactly(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    out_dir = tmp_path / "out"
    lib_dir = tmp_path / "lib"
    for directory in (runtime.parent, out_dir, lib_dir):
        directory.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"runtime")
    consumer = lib_dir / "libconsumer.a"
    provider = lib_dir / "libprovider.a"
    consumer.write_bytes(b"consumer")
    provider.write_bytes(b"provider")

    def flags(native_arguments: str) -> list[str]:
        write_native_link_dependency_manifest(
            _cargo_output(
                _cargo_message(
                    "path+file:///workspace#native@1.0.0",
                    str(out_dir),
                    linked_paths=[f"native={lib_dir}"],
                    linked_libs=[
                        "static-nobundle=consumer",
                        "static-nobundle=provider",
                    ],
                ),
                native_arguments,
            ),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=None,
            **_source_args(tmp_path),
        )
        return native_link_deps._collect_cargo_native_link_deps(
            runtime,
            object_format="elf",
            source_root=tmp_path,
            source_fingerprint=_SOURCE_FINGERPRINT,
        )

    good = flags("-lconsumer -lprovider")
    bad = flags("-lprovider -lconsumer")
    assert good == [f"-L{lib_dir}", "-lconsumer", f"-L{lib_dir}", "-lprovider"]
    assert bad == [f"-L{lib_dir}", "-lprovider", f"-L{lib_dir}", "-lconsumer"]
    assert good != bad


def test_coff_rustc_linker_tokens_are_forwarded_through_driver_exactly(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.lib"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    write_native_link_dependency_manifest(
        "",
        cargo_stderr=(
            "note: native-static-libs: kernel32.lib /defaultlib:msvcrt "
            "kernel32.lib\n"
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple="x86_64-pc-windows-msvc",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    assert read_native_link_flags(
        runtime,
        target_triple="x86_64-pc-windows-msvc",
        object_format="coff",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    ) == [
        "-Wl,kernel32.lib",
        "-Wl,/defaultlib:msvcrt",
        "-Wl,kernel32.lib",
    ]


def test_rustc_lowered_native_flags_are_not_reconstructed(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    out_dir = tmp_path / "out"
    lib_dir = tmp_path / "lib"
    for directory in (runtime.parent, out_dir, lib_dir):
        directory.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"runtime")
    archive = lib_dir / "libwhole.a"
    archive.write_bytes(b"archive")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "path+file:///workspace#native@1.0.0",
                str(out_dir),
                linked_paths=[f"native={lib_dir}"],
                linked_libs=[
                    "static-nobundle:+whole-archive=whole",
                    "dylib:-as-needed=required",
                ],
            ),
            "-Wl,--whole-archive -lwhole -Wl,--no-whole-archive "
            "-Wl,--no-as-needed -lrequired -Wl,--as-needed",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_source_args(tmp_path),
    )
    assert native_link_deps._collect_cargo_native_link_deps(
        runtime,
        object_format="elf",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    ) == [
        "-Wl,--whole-archive",
        f"-L{lib_dir}",
        "-lwhole",
        "-Wl,--no-whole-archive",
        "-Wl,--no-as-needed",
        "-lrequired",
        "-Wl,--as-needed",
    ]


@pytest.mark.skipif(os.name == "nt", reason="COFF archive resolution rescans inputs")
def test_static_nobundle_order_changes_real_archive_resolution(tmp_path: Path) -> None:
    clang = shutil.which("clang")
    ar = shutil.which("llvm-ar") or shutil.which("ar")
    if clang is None or ar is None:
        pytest.skip("clang and an ar implementation are required")
    sources = {
        "main": "extern int consumer(void); int main(void){return consumer()!=7;}",
        "consumer": "extern int provider(void); int consumer(void){return provider();}",
        "provider": "int provider(void){return 7;}",
    }
    objects: dict[str, Path] = {}
    for name, source in sources.items():
        source_path = tmp_path / f"{name}.c"
        object_path = tmp_path / f"{name}.o"
        source_path.write_text(source, encoding="utf-8")
        run_cli_test_process(
            [clang, "-c", str(source_path), "-o", str(object_path)],
            text=True,
            timeout=30,
            check=True,
        )
        objects[name] = object_path
    consumer = tmp_path / "libconsumer.a"
    provider = tmp_path / "libprovider.a"
    run_cli_test_process(
        [ar, "rcs", str(consumer), str(objects["consumer"])],
        text=True,
        timeout=30,
        check=True,
    )
    run_cli_test_process(
        [ar, "rcs", str(provider), str(objects["provider"])],
        text=True,
        timeout=30,
        check=True,
    )
    good = run_cli_test_process(
        [
            clang,
            str(objects["main"]),
            str(consumer),
            str(provider),
            "-o",
            str(tmp_path / "good"),
        ],
        text=True,
        timeout=30,
        check=False,
    )
    bad = run_cli_test_process(
        [
            clang,
            str(objects["main"]),
            str(provider),
            str(consumer),
            "-o",
            str(tmp_path / "bad"),
        ],
        text=True,
        timeout=30,
        check=False,
    )
    assert good.returncode == 0, good.stderr
    assert bad.returncode != 0


def test_runtime_manifest_refresh_uses_exact_cargo_json_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.stdlib_micro.lib"
    scratch = runtime.with_name("molt_runtime.lib")
    runtime.parent.mkdir()
    runtime.write_bytes(b"same")
    scratch.write_bytes(b"same")
    (tmp_path / "out").mkdir()
    (tmp_path / "lib").mkdir()
    command = runtime_build._native_runtime_cargo_command(
        cargo_profile="dev-fast",
        concrete_stdlib_profile="micro",
        runtime_features=("native_feature",),
        builtin_features=("builtin_set",),
        concrete_stdlib_feature="stdlib_micro",
        target_triple=None,
    )
    assert command[:7] == [
        "cargo",
        "rustc",
        "-p",
        "molt-runtime",
        "--profile",
        "dev-fast",
        "--message-format=json-render-diagnostics",
    ]
    assert command[-3:] == ["--", "--print", "native-static-libs"]
    captured: list[list[str]] = []
    cargo_stdout = _cargo_output(
        _cargo_message(
            "registry#dep@1.0.0",
            str(tmp_path / "out"),
            linked_paths=[f"native={tmp_path / 'lib'}"],
            linked_libs=["dylib=dep"],
        ),
        "-ldep",
    )
    monkeypatch.setattr(
        runtime_build,
        "_build_slot",
        lambda: contextlib.nullcontext(0),
    )
    monkeypatch.setattr(
        runtime_build,
        "_run_cargo_with_sccache_retry",
        lambda cmd, **_kwargs: (
            captured.append(list(cmd))
            or subprocess.CompletedProcess(cmd, 0, cargo_stdout, "")
        ),
    )

    assert runtime_build._refresh_native_link_manifest(
        runtime_lib=runtime,
        target_triple=None,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cmd=command,
        build_env={},
        cargo_timeout=1.0,
        json_output=True,
        source_fingerprint=_SOURCE_FINGERPRINT,
    )
    assert captured == [command]
    assert native_link_deps._collect_cargo_native_link_deps(
        runtime,
        object_format="elf",
        source_root=tmp_path,
        source_fingerprint=_SOURCE_FINGERPRINT,
    ) == ["-ldep"]


def test_artifact_reuse_without_manifest_requires_exact_refresh(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.stdlib_micro.lib"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    refreshes: list[Path] = []
    monkeypatch.setattr(runtime_build, "_cargo_build_env", lambda: {})
    monkeypatch.setattr(runtime_build, "_cargo_target_root", lambda _root: tmp_path)
    monkeypatch.setattr(runtime_build, "_maybe_enable_sccache", lambda _env: None)
    monkeypatch.setattr(
        runtime_build,
        "_runtime_fingerprint",
        lambda *_a, **_k: dict(_SOURCE_FINGERPRINT),
    )
    monkeypatch.setattr(runtime_build, "_read_runtime_fingerprint", lambda _path: None)
    monkeypatch.setattr(
        runtime_build,
        "_runtime_artifact_fingerprint_matches",
        lambda *_a, **_k: True,
    )
    monkeypatch.setattr(
        runtime_build,
        "_runtime_fingerprint_metadata_needs_refresh",
        lambda *_a, **_k: False,
    )
    monkeypatch.setattr(
        runtime_build, "_native_link_manifest_matches", lambda *_a, **_k: False
    )
    monkeypatch.setattr(
        runtime_build,
        "_refresh_native_link_manifest",
        lambda *, runtime_lib, **_kwargs: refreshes.append(runtime_lib) or True,
    )
    monkeypatch.setattr(
        runtime_build,
        "_build_lock",
        lambda *_a, **_k: contextlib.nullcontext(),
    )
    runtime_build._RUNTIME_LIB_VERIFIED.clear()
    try:
        assert runtime_build._ensure_runtime_lib(
            runtime,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
    finally:
        runtime_build._RUNTIME_LIB_VERIFIED.clear()
    assert refreshes == [runtime]


def test_hydration_without_matching_manifest_requires_exact_refresh(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "isolated" / "dev-fast" / "molt_runtime.stdlib_micro.lib"
    canonical = tmp_path / "canonical" / "dev-fast" / runtime.name
    canonical.parent.mkdir(parents=True)
    canonical.write_bytes(b"runtime")
    refreshes: list[Path] = []
    monkeypatch.setattr(runtime_build, "_cargo_build_env", lambda: {})
    monkeypatch.setattr(runtime_build, "_cargo_target_root", lambda _root: tmp_path)
    monkeypatch.setattr(runtime_build, "_maybe_enable_sccache", lambda _env: None)
    monkeypatch.setattr(
        runtime_build,
        "_runtime_fingerprint",
        lambda *_a, **_k: dict(_SOURCE_FINGERPRINT),
    )
    monkeypatch.setattr(runtime_build, "_read_runtime_fingerprint", lambda _path: None)
    monkeypatch.setattr(
        runtime_build,
        "_runtime_artifact_fingerprint_matches",
        lambda *_a, **_k: False,
    )
    monkeypatch.setattr(
        runtime_build,
        "_canonical_target_root",
        lambda _root: tmp_path / "canonical",
    )
    monkeypatch.setattr(
        runtime_build,
        "_canonical_build_state_root",
        lambda _root: tmp_path / "state",
    )
    monkeypatch.setattr(
        runtime_build,
        "_artifact_state_path_for_build_state_root",
        lambda *_a, **_k: tmp_path / "canonical.fingerprint",
    )

    def hydrate(**_kwargs) -> bool:
        runtime.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(canonical, runtime)
        return True

    monkeypatch.setattr(
        runtime_build, "_maybe_hydrate_artifact_from_canonical_target", hydrate
    )
    monkeypatch.setattr(
        runtime_build, "_native_link_manifest_matches", lambda *_a, **_k: False
    )
    monkeypatch.setattr(
        runtime_build,
        "_refresh_native_link_manifest",
        lambda *, runtime_lib, **_kwargs: refreshes.append(runtime_lib) or True,
    )
    monkeypatch.setattr(
        runtime_build,
        "_build_lock",
        lambda *_a, **_k: contextlib.nullcontext(),
    )
    runtime_build._RUNTIME_LIB_VERIFIED.clear()
    try:
        assert runtime_build._ensure_runtime_lib(
            runtime,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
    finally:
        runtime_build._RUNTIME_LIB_VERIFIED.clear()
    assert refreshes == [runtime]


def test_link_dependency_authority_cannot_return_to_build_directory_scanning() -> None:
    deps_source = inspect.getsource(native_link_deps._collect_cargo_native_link_deps)
    assert ".iterdir(" not in deps_source
    assert "read_text(" not in deps_source
    assert "read_native_link_flags(" in deps_source
    assert "source_fingerprint=source_fingerprint" in deps_source

    build_source = inspect.getsource(runtime_build._ensure_runtime_lib)
    assert "write_native_link_dependency_manifest(" in build_source
    assert "_refresh_native_link_manifest(" in build_source
    command_source = inspect.getsource(runtime_build._native_runtime_cargo_command)
    assert '"rustc"' in command_source
    assert "--message-format=json-render-diagnostics" in command_source
    assert "native-static-libs" in command_source
    manifest_source = inspect.getsource(native_link_deps.read_native_link_flags)
    assert "source_fingerprint=source_fingerprint" in manifest_source
    cargo_source = inspect.getsource(cargo_execution._run_cargo_with_sccache_retry)
    assert cargo_source.count('encoding="utf-8"') == 2
    assert cargo_source.count('errors="strict"') == 2
