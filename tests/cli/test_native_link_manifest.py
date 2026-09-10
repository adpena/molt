from __future__ import annotations

from functools import partial

import contextlib
import inspect
import json
import os
import shlex
import shutil
import subprocess
from collections.abc import Mapping
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TypedDict, cast

import pytest

from molt.cli import (
    cargo_execution,
    native_link_deps,
    native_link_manifest,
    runtime_callable_symbols,
)
from molt.cli import runtime_native_build as runtime_build
from molt.cli import static_archive_identity as archive_identity
from molt.cli.models import _RuntimeArtifactState
from molt.cli.native_link_custody import copy_native_link_custody_archive
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    manifest_from_cargo_json,
    native_link_dependency_manifest_path,
    native_link_flags_from_manifest,
    read_native_link_dependency_manifest,
    read_native_link_flags,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from tests.cli.native_link_test_support import write_test_static_archive
from tests.cli.process_guard import run_cli_test_process
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    native_runtime_staticlib_identity,
    runtime_cargo_plan,
)

_RUNTIME_BUILD_IDENTITY = native_runtime_staticlib_identity(
    cargo_profile="dev-fast",
    target_triple=None,
    family_seed="native-link-family",
)


@pytest.fixture(autouse=True)
def _cargo_plan_authority(
    runtime_fixture_root: RuntimeFixtureRoot, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        runtime_build,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )


class _RuntimeBuildArgs(TypedDict):
    runtime_build_identity: RuntimeBuildIdentity


def _runtime_build_args(
    *,
    cargo_profile: str = "dev-fast",
    target_triple: str | None = None,
    family_seed: str = "native-link-family",
) -> _RuntimeBuildArgs:
    return {
        "runtime_build_identity": native_runtime_staticlib_identity(
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            family_seed=family_seed,
        )
    }


def _mapping_field(
    value: Mapping[str, object],
    field: str,
) -> Mapping[str, object]:
    return cast(Mapping[str, object], value[field])


def _link_plan_items(
    manifest: Mapping[str, object],
) -> list[Mapping[str, object]]:
    return cast(
        list[Mapping[str, object]], _mapping_field(manifest, "link_plan")["items"]
    )


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


def test_manifest_is_path_neutral_and_custodies_non_system_static_libraries(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime-v1")
    libs_v2 = tmp_path / "libs-v2"
    libs_v1 = tmp_path / "libs-v1"
    raw_libs = tmp_path / "raw-libs"
    for directory in (libs_v2, libs_v1, raw_libs):
        directory.mkdir()
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
                linked_paths=[f"native={libs_v1}"],
                linked_libs=["framework=Metal", "weak_framework=QuartzCore"],
            ),
        ]
    )
    build_args = _runtime_build_args(target_triple="x86_64-apple-darwin")
    path = write_native_link_dependency_manifest(
        _cargo_output(
            messages,
            "-lpsapi -lkernel32 -lexternal_static "
            "-framework Metal -weak_framework QuartzCore",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple="x86_64-apple-darwin",
        **build_args,
    )
    manifest = read_native_link_dependency_manifest(
        runtime,
        target_triple="x86_64-apple-darwin",
    )
    assert path == native_link_dependency_manifest_path(runtime)
    assert manifest["schema_version"] == 5
    build_identity = build_args["runtime_build_identity"]
    assert manifest["runtime_build_identity"] == build_identity.to_dict()
    assert "build_scripts" not in manifest
    assert os.fspath(tmp_path) not in json.dumps(manifest, sort_keys=True)
    assert len(cast(list[object], _mapping_field(manifest, "custody")["entries"])) == 1
    flags = native_link_flags_from_manifest(
        manifest,
        object_format="macho",
        runtime_lib=runtime,
    )
    assert flags[:2] == [
        "-lpsapi",
        "-lkernel32",
    ]
    assert flags[2].startswith(f"-L{runtime.parent / '.molt-native-link-custody-'}")
    assert flags[3:] == [
        "-lexternal_static",
        "-framework",
        "Metal",
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
        **_runtime_build_args(
            cargo_profile="release-output",
            target_triple="aarch64-unknown-linux-gnu",
        ),
    )
    second = manifest_from_cargo_json(
        _cargo_output(f"{other}\n{message}", "-lz -lz -lother"),
        runtime_lib=runtime,
        cargo_profile="release-output",
        target_triple="aarch64-unknown-linux-gnu",
        **_runtime_build_args(
            cargo_profile="release-output",
            target_triple="aarch64-unknown-linux-gnu",
        ),
    )
    assert first == second
    assert [item["argument"] for item in _link_plan_items(first)] == [
        "-lz",
        "-lz",
        "-lother",
    ]
    deduped = manifest_from_cargo_json(
        _cargo_output(f"{message}\n{message}", "-lz -lz"),
        runtime_lib=runtime,
        cargo_profile="release-output",
        target_triple="aarch64-unknown-linux-gnu",
        **_runtime_build_args(
            cargo_profile="release-output",
            target_triple="aarch64-unknown-linux-gnu",
        ),
    )
    assert len(_link_plan_items(deduped)) == 2


def test_direct_static_link_input_is_custodied_and_replayed_after_pruning(
    tmp_path: Path,
) -> None:
    target_triple = "aarch64-unknown-linux-gnu"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    direct = tmp_path / "producer" / "libdirect.a"
    runtime.parent.mkdir(parents=True)
    direct.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    direct.write_bytes(b"direct archive")
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="direct-static-input-family",
    )
    write_native_link_dependency_manifest(
        _cargo_output("", shlex.quote(os.fspath(direct))),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )
    direct.unlink()

    flags = read_native_link_flags(
        runtime,
        target_triple=target_triple,
        object_format="elf",
        runtime_build_identity=build_identity,
    )
    assert len(flags) == 1
    replayed = Path(flags[0])
    assert replayed != direct
    assert replayed.read_bytes() == b"direct archive"
    assert runtime.parent in replayed.parents


@pytest.mark.parametrize("suffix", [".so", ".so.3", ".dylib", ".dll", ".dll.a", ".tbd"])
def test_direct_dynamic_link_input_requires_runtime_distribution_contract(
    tmp_path: Path,
    suffix: str,
) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    direct = tmp_path / "producer" / f"libdynamic{suffix}"
    runtime.parent.mkdir(parents=True)
    direct.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    direct.write_bytes(b"dynamic library")

    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="explicit runtime distribution contract",
    ):
        manifest_from_cargo_json(
            _cargo_output("", shlex.quote(os.fspath(direct))),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            **_runtime_build_args(target_triple=target_triple),
        )


def test_search_resolved_dynamic_library_is_not_misclassified_as_system(
    tmp_path: Path,
) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    out_dir = tmp_path / "out"
    lib_dir = tmp_path / "lib"
    for directory in (runtime.parent, out_dir, lib_dir):
        directory.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    (lib_dir / "libowned.so").write_bytes(b"dynamic library")

    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="explicit runtime distribution contract",
    ):
        manifest_from_cargo_json(
            _cargo_output(
                _cargo_message(
                    "registry#owned-sys@1.0.0",
                    os.fspath(out_dir),
                    linked_paths=[f"native={lib_dir}"],
                    linked_libs=["dylib=owned"],
                ),
                "-lowned",
            ),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            **_runtime_build_args(target_triple=target_triple),
        )


@pytest.mark.parametrize(
    ("linked_kind", "error"),
    [
        ("static=owned", None),
        ("dylib=owned", "explicit runtime distribution contract"),
        (None, "ambiguous between a static archive and a dynamic import library"),
    ],
)
def test_local_coff_library_requires_explicit_static_or_runtime_custody(
    tmp_path: Path,
    linked_kind: str | None,
    error: str | None,
) -> None:
    target_triple = "aarch64-pc-windows-msvc"
    runtime = tmp_path / "dev-fast" / "molt_runtime.lib"
    out_dir = tmp_path / "out"
    lib_dir = tmp_path / "lib"
    for directory in (runtime.parent, out_dir, lib_dir):
        directory.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    owned = lib_dir / "owned.lib"
    owned.write_bytes(b"coff library")
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed=f"coff-library-{linked_kind}",
    )

    def write() -> None:
        write_native_link_dependency_manifest(
            _cargo_output(
                _cargo_message(
                    "registry#owned-sys@1.0.0",
                    os.fspath(out_dir),
                    linked_paths=[f"native={lib_dir}"],
                    linked_libs=[] if linked_kind is None else [linked_kind],
                ),
                "owned.lib",
            ),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            runtime_build_identity=build_identity,
        )

    if error is not None:
        with pytest.raises(NativeLinkDependencyManifestError, match=error):
            write()
        return
    write()
    owned.unlink()
    flags = read_native_link_flags(
        runtime,
        target_triple=target_triple,
        object_format="coff",
        runtime_build_identity=build_identity,
    )
    assert len(flags) == 1
    replayed = Path(flags[0])
    assert replayed.read_bytes() == b"coff library"
    assert replayed != owned


def test_direct_linker_script_requires_a_typed_relocation_contract(
    tmp_path: Path,
) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    script = tmp_path / "producer" / "symbols.ld"
    runtime.parent.mkdir(parents=True)
    script.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    script.write_text("SECTIONS {}", encoding="utf-8")

    with pytest.raises(NativeLinkDependencyManifestError, match="no supported.*role"):
        manifest_from_cargo_json(
            _cargo_output("", shlex.quote(os.fspath(script))),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            **_runtime_build_args(target_triple=target_triple),
        )


def test_non_system_framework_requires_runtime_distribution_contract(
    tmp_path: Path,
) -> None:
    target_triple = "aarch64-apple-darwin"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    out_dir = tmp_path / "out"
    frameworks = tmp_path / "Frameworks"
    framework = frameworks / "Owned.framework"
    for directory in (runtime.parent, out_dir, framework):
        directory.mkdir(parents=True)
    runtime.write_bytes(b"runtime")

    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="non-system frameworks are not relocatable",
    ):
        manifest_from_cargo_json(
            _cargo_output(
                _cargo_message(
                    "registry#owned-framework@1.0.0",
                    os.fspath(out_dir),
                    linked_paths=[f"framework={frameworks}"],
                    linked_libs=["framework=Owned"],
                ),
                "-framework Owned",
            ),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            **_runtime_build_args(target_triple=target_triple),
        )


def test_system_framework_remains_an_explicit_path_neutral_link_plan_item(
    tmp_path: Path,
) -> None:
    target_triple = "x86_64-apple-darwin"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    manifest = manifest_from_cargo_json(
        _cargo_output("", "-framework Metal -weak_framework QuartzCore"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        **_runtime_build_args(target_triple=target_triple),
    )

    assert _mapping_field(manifest, "custody")["archive"] is None
    assert _link_plan_items(manifest) == [
        {"kind": "system-framework", "name": "Metal", "weak": False},
        {"kind": "system-framework", "name": "QuartzCore", "weak": True},
    ]


@pytest.mark.parametrize(
    ("target_triple", "argument"),
    [
        ("x86_64-unknown-linux-gnu", "-Lrelative"),
        ("x86_64-unknown-linux-gnu", "-Wl,-rpath,$ORIGIN"),
        ("x86_64-unknown-linux-gnu", "-Wl,-force_load,relative.a"),
        ("x86_64-apple-darwin", "-Wl,-framework,Metal"),
        ("x86_64-unknown-linux-gnu", "--sysroot=relative"),
        ("x86_64-unknown-linux-gnu", "@response.rsp"),
        ("x86_64-pc-windows-msvc", "/LIBPATH:C:\\sdk\\lib"),
        ("x86_64-pc-windows-msvc", "/WHOLEARCHIVE:local.lib"),
        ("x86_64-pc-windows-msvc", "/tmp/local.lib"),
    ],
)
def test_path_bearing_linker_arguments_cannot_bypass_custody(
    tmp_path: Path,
    target_triple: str,
    argument: str,
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")

    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="path-bearing|relocatable custody",
    ):
        manifest_from_cargo_json(
            _cargo_output("", argument),
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=target_triple,
            **_runtime_build_args(target_triple=target_triple),
        )


@pytest.mark.parametrize(
    ("target", "note", "object_format", "flags"),
    [
        ("x86_64-unknown-linux-gnu", "-lc -lm", "elf", ["-lc", "-lm"]),
        (
            "aarch64-apple-darwin",
            "-framework CoreFoundation",
            "macho",
            ["-framework", "CoreFoundation"],
        ),
        ("x86_64-pc-windows-msvc", '"kernel32.lib"', "coff", ["-Wl,kernel32.lib"]),
    ],
)
def test_implicit_native_link_protocol_uses_captured_target_not_reader_host(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    target: str,
    note: str,
    object_format: str,
    flags: list[str],
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", host_target=target
    )
    resolve = native_link_manifest.resolve_native_target_spec

    def require_captured_target(actual: str):
        assert actual == target
        return resolve(actual)

    monkeypatch.setattr(
        native_link_manifest, "resolve_native_target_spec", require_captured_target
    )
    write_native_link_dependency_manifest(
        "",
        cargo_stderr=f"note: native-static-libs: {note}\n",
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=build_identity,
    )
    manifest = read_native_link_dependency_manifest(
        runtime,
        target_triple=None,
        runtime_build_identity=build_identity,
    )
    assert (
        native_link_flags_from_manifest(
            manifest, object_format=object_format, runtime_lib=runtime
        )
        == flags
    )
    assert (
        read_native_link_flags(
            runtime,
            target_triple=None,
            runtime_build_identity=build_identity,
            object_format=object_format,
        )
        == flags
    )
    wrong_format = "coff" if object_format != "coff" else "elf"
    with pytest.raises(
        NativeLinkDependencyManifestError, match="captured runtime target"
    ):
        native_link_flags_from_manifest(
            manifest, object_format=wrong_format, runtime_lib=runtime
        )


def test_manifest_reader_rejects_object_format_target_disagreement(
    tmp_path: Path,
) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="object-format-disagreement-family",
    )
    write_native_link_dependency_manifest(
        _cargo_output("", "-lm"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )

    with pytest.raises(NativeLinkDependencyManifestError, match="object format"):
        read_native_link_flags(
            runtime,
            target_triple=target_triple,
            object_format="coff",
            runtime_build_identity=build_identity,
        )


@pytest.mark.parametrize("kind", [None, True, 5, [], {}, ["system-library"]])
def test_manifest_rejects_nonstring_item_kind_with_domain_diagnostic(
    tmp_path: Path, kind: object
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    path = write_native_link_dependency_manifest(
        "",
        cargo_stderr="note: native-static-libs: -lc\n",
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    manifest = json.loads(path.read_text(encoding="utf-8"))
    manifest["link_plan"]["items"][0]["kind"] = kind
    path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="kind must be a string"
    ):
        read_native_link_dependency_manifest(runtime, target_triple=None)


def test_manifest_reader_bounds_input_before_reading_payload(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "oversized.json"
    path.write_bytes(b"x" * 65)
    monkeypatch.setattr(native_link_manifest, "RUNTIME_ARTIFACT_METADATA_MAX_BYTES", 64)
    with pytest.raises(NativeLinkDependencyManifestError, match="exceeds size limit"):
        native_link_manifest.read_native_link_dependency_manifest_payload(path)


def test_manifest_reader_consumes_shared_generation_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "manifest.json"
    path.write_text("{}", encoding="utf-8")
    from molt import exact_json

    open_source = exact_json.open_stable_regular_file

    @contextlib.contextmanager
    def changed(path: Path, *, label: str):
        with open_source(path, label=label) as opened:
            yield opened
            raise ValueError("source changed during identity read")

    monkeypatch.setattr(exact_json, "open_stable_regular_file", changed)
    with pytest.raises(NativeLinkDependencyManifestError, match="source changed"):
        native_link_manifest.read_native_link_dependency_manifest_payload(path)


@pytest.mark.parametrize("size", [1.0, True])
def test_manifest_rejects_resealed_runtime_numeric_coercion(
    tmp_path: Path, size: object
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"x")
    manifest_path = write_native_link_dependency_manifest(
        _cargo_output(""),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_runtime_build_args(),
    )
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["runtime"]["size_bytes"] = size
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(NativeLinkDependencyManifestError, match="nonnegative integer"):
        read_native_link_dependency_manifest(runtime, target_triple=None)


def test_manifest_rejects_archive_target_profile_and_json_drift(tmp_path: Path) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir()
    runtime.write_bytes(b"runtime")
    write_native_link_dependency_manifest(
        _cargo_output(""),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        **_runtime_build_args(),
    )
    manifest_path = native_link_dependency_manifest_path(runtime)
    original_manifest = manifest_path.read_text(encoding="utf-8")
    manifest = json.loads(original_manifest)
    manifest["schema_version"] = 5.0
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="unsupported manifest schema"
    ):
        read_native_link_dependency_manifest(runtime, target_triple=None)
    manifest_path.write_text(original_manifest, encoding="utf-8")
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
    manifest_path.write_text('{"schema_version":NaN}', encoding="utf-8")
    with pytest.raises(
        NativeLinkDependencyManifestError, match="non-finite JSON number"
    ):
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
            **_runtime_build_args(),
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
        **_runtime_build_args(),
    )

    assert native_link_deps._collect_cargo_native_link_deps(
        runtime,
        object_format="elf",
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
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
    (provider_lib / "libprovider.a").write_bytes(b"provider static library")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "path+file:///repo#dep@1.0.0",
                str(provider_out),
                linked_paths=[f"native={provider_lib}"],
                linked_libs=["static=provider"],
            ),
            "-lprovider",
        ),
        runtime_lib=provider,
        cargo_profile="dev-fast",
        target_triple=None,
        **_runtime_build_args(),
    )
    shutil.copyfile(provider, consumer)
    with pytest.raises(NativeLinkDependencyManifestError, match="cannot read"):
        read_native_link_dependency_manifest(consumer, target_triple=None)
    shutil.copyfile(
        native_link_dependency_manifest_path(provider),
        native_link_dependency_manifest_path(consumer),
    )
    with pytest.raises(NativeLinkDependencyManifestError, match="custody archive"):
        read_native_link_dependency_manifest(
            consumer,
            target_triple=None,
            cargo_profile="dev-fast",
        )
    provider_manifest = json.loads(
        native_link_dependency_manifest_path(provider).read_text(encoding="utf-8")
    )
    copy_native_link_custody_archive(
        provider,
        consumer,
        provider_manifest["custody"],
    )
    hydrated = read_native_link_dependency_manifest(
        consumer,
        target_triple=None,
        cargo_profile="dev-fast",
    )
    assert _mapping_field(hydrated, "runtime")["sha256"]


def test_manifest_replay_survives_pruned_producer_link_directories(
    tmp_path: Path,
) -> None:
    out_dir = tmp_path / "target" / "build" / "dep" / "out"
    linked_dir = tmp_path / "target" / "native"
    runtime = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    for directory in (out_dir, linked_dir, runtime.parent):
        directory.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(b"runtime")
    (linked_dir / "libdep.a").write_bytes(b"owned static dependency")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "registry#dep@1.0.0",
                str(out_dir),
                linked_paths=[f"native={linked_dir}"],
                linked_libs=["static=dep"],
            ),
            "-ldep",
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    manifest = read_native_link_dependency_manifest(
        runtime,
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    encoded = json.dumps(manifest, sort_keys=True)
    assert os.fspath(out_dir) not in encoded
    assert os.fspath(linked_dir) not in encoded

    shutil.rmtree(linked_dir)
    shutil.rmtree(out_dir)
    flags = read_native_link_flags(
        runtime,
        target_triple=None,
        object_format="elf",
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    assert flags[-1] == "-ldep"
    assert flags[-2].startswith(f"-L{runtime.parent / '.molt-native-link-custody-'}")


def test_hydrated_manifest_refuses_foreign_runtime_build_identity(
    tmp_path: Path,
) -> None:
    out_dir = tmp_path / "target" / "build" / "dep" / "out"
    linked_dir = tmp_path / "target" / "native"
    provider = tmp_path / "provider" / "dev-fast" / "libmolt_runtime.a"
    consumer = tmp_path / "consumer" / "dev-fast" / "libmolt_runtime.a"
    for directory in (
        out_dir,
        linked_dir,
        provider.parent,
        consumer.parent,
    ):
        directory.mkdir(parents=True, exist_ok=True)
    provider.write_bytes(b"same-runtime")
    (linked_dir / "libdep.a").write_bytes(b"dependency static library")
    write_native_link_dependency_manifest(
        _cargo_output(
            _cargo_message(
                "registry#dep@1.0.0",
                str(out_dir),
                linked_paths=[f"native={linked_dir}"],
                linked_libs=["static=dep"],
            ),
            "-ldep",
        ),
        runtime_lib=provider,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    shutil.copyfile(provider, consumer)
    shutil.copyfile(
        native_link_dependency_manifest_path(provider),
        native_link_dependency_manifest_path(consumer),
    )
    provider_manifest = json.loads(
        native_link_dependency_manifest_path(provider).read_text(encoding="utf-8")
    )
    copy_native_link_custody_archive(
        provider,
        consumer,
        provider_manifest["custody"],
    )
    hydrated = read_native_link_dependency_manifest(
        consumer,
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    assert hydrated["runtime_build_identity"] == _RUNTIME_BUILD_IDENTITY.to_dict()
    foreign_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=None,
        family_seed="foreign-native-link-family",
    )
    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="runtime build identity mismatch",
    ):
        read_native_link_dependency_manifest(
            consumer,
            target_triple=None,
            runtime_build_identity=foreign_identity,
        )


def test_repeated_publication_and_concurrent_reads_preserve_exact_build_identity(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "shared" / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"shared-runtime")

    def publish() -> Path:
        return write_native_link_dependency_manifest(
            "",
            cargo_stderr="note: native-static-libs: -lc\n",
            runtime_lib=runtime,
            cargo_profile="dev-fast",
            target_triple=None,
            runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
        )

    path = publish()
    first = path.read_bytes()
    publish()
    final = path.read_bytes()
    assert final == first

    def consume(_index: int) -> Mapping[str, object]:
        return read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
        )

    with ThreadPoolExecutor(max_workers=4) as executor:
        snapshots = list(executor.map(consume, range(16)))

    assert all(
        snapshot["runtime_build_identity"] == _RUNTIME_BUILD_IDENTITY.to_dict()
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
        **_runtime_build_args(target_triple="x86_64-pc-windows-msvc"),
    )
    assert [item["argument"] for item in _link_plan_items(manifest)] == [
        "user32.lib",
        "advapi32.lib",
        "user32.lib",
    ]


def test_native_static_lib_note_ignores_terminal_color_decoration(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    # Exact shape produced by rustc when CI exports CARGO_TERM_COLOR=always.
    colored_note = (
        "\x1b[1m\x1b[92mnote\x1b[0m\x1b[1m\x1b[97m: "
        "native-static-libs: -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc\x1b[0m\n"
    )
    manifest = manifest_from_cargo_json(
        "",
        cargo_stderr=colored_note,
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple="x86_64-unknown-linux-gnu",
        **_runtime_build_args(target_triple="x86_64-unknown-linux-gnu"),
    )
    assert [item["argument"] for item in _link_plan_items(manifest)] == [
        "-lgcc_s",
        "-lutil",
        "-lrt",
        "-lpthread",
        "-lm",
        "-ldl",
        "-lc",
    ]


def test_native_runtime_failure_reaches_cli_json_with_durable_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    runtime_lib = tmp_path / "dev-fast" / "libmolt_runtime.stdlib_micro.a"
    state = _RuntimeArtifactState(runtime_lib=runtime_lib)
    cargo_stdout = json.dumps(
        {
            "reason": "compiler-message",
            "message": {
                "level": "error",
                "message": "cannot find value `BROKEN_RUNTIME` in this scope",
                "rendered": (
                    "\x1b[31merror[E0425]\x1b[0m: cannot find value "
                    "`BROKEN_RUNTIME` in this scope"
                ),
            },
        }
    )
    monkeypatch.setattr(
        runtime_build,
        "_runtime_build_identity_for_plan",
        lambda *_args, **_kwargs: _RUNTIME_BUILD_IDENTITY,
    )
    monkeypatch.setattr(
        runtime_build,
        "_runtime_fingerprint_path",
        lambda *_args, **_kwargs: tmp_path / "runtime.fingerprint.json",
    )
    monkeypatch.setattr(runtime_build, "_read_runtime_fingerprint", lambda _path: None)
    monkeypatch.setattr(
        runtime_build,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_kwargs: False,
    )
    monkeypatch.setattr(
        runtime_build,
        "_build_lock",
        lambda *_args, **_kwargs: contextlib.nullcontext(),
    )
    monkeypatch.setattr(
        runtime_build,
        "_build_state_root",
        lambda _root: tmp_path / "state",
    )
    monkeypatch.setattr(
        runtime_build,
        "_run_resolved_cargo_plan",
        lambda plan, **_kwargs: subprocess.CompletedProcess(
            list(plan.command),
            101,
            cargo_stdout,
            "",
        ),
    )
    assert not runtime_build._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cargo_timeout=30.0,
        stdlib_profile="micro",
        runtime_state=state,
    )
    assert state.native_runtime_build_failure is not None
    assert "error[E0425]" in state.native_runtime_build_failure.summary
    assert "\x1b" not in state.native_runtime_build_failure.summary
    evidence_path = state.native_runtime_build_failure.evidence_path
    assert evidence_path is not None and evidence_path.is_file()
    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
    assert evidence["returncode"] == 101
    assert evidence["cargo_stdout"] == cargo_stdout

    monkeypatch.setattr(
        runtime_callable_symbols,
        "_ensure_native_runtime_lib_ready_before_link",
        lambda *_args, **_kwargs: False,
    )
    _digest, rc = (
        runtime_callable_symbols._stage_runtime_callable_symbols_for_native_codegen(
            state,
            target_triple=None,
            json_output=True,
            runtime_cargo_profile="dev-fast",
            molt_root=tmp_path,
            cargo_timeout=None,
        )
    )
    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["data"]["returncode"] == 2
    runtime_failure = payload["data"]["runtime_build_failure"]
    assert runtime_failure["stage"] == "cargo"
    assert runtime_failure["returncode"] == 101
    assert runtime_failure["evidence_path"] == str(evidence_path)
    assert "BROKEN_RUNTIME" in payload["errors"][0]


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
            **_runtime_build_args(target_triple="x86_64-pc-windows-msvc"),
        )


@pytest.mark.parametrize(
    "identity_path",
    [
        ("digest",),
        ("compile_digest",),
        ("family_digest",),
        ("payload", "family", "compile", "sources", "digest"),
    ],
)
def test_manifest_reader_rejects_tampered_runtime_build_identity(
    tmp_path: Path,
    identity_path: tuple[str, ...],
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    path = write_native_link_dependency_manifest(
        _cargo_output("", "-lc"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )
    payload = json.loads(path.read_text(encoding="utf-8"))
    identity = payload["runtime_build_identity"]
    for part in identity_path[:-1]:
        identity = identity[part]
    identity[identity_path[-1]] = "tampered"
    path.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(
        NativeLinkDependencyManifestError,
        match="runtime build identity is not the selected native staticlib identity",
    ):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
        )


@pytest.mark.parametrize("mutation", ["in-place", "replacement"])
def test_runtime_admission_reads_current_content_and_rejects_preserved_metadata_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    runtime = tmp_path / "dev-fast" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    write_test_static_archive(runtime, b"runtime-a")
    write_native_link_dependency_manifest(
        _cargo_output("", "-lc"),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
    )

    real_hash_exact = archive_identity._hash_exact
    hash_calls: list[int] = []

    def counting_hash_exact(stream, size: int) -> str:
        hash_calls.append(size)
        return real_hash_exact(stream, size)

    monkeypatch.setattr(archive_identity, "_hash_exact", counting_hash_exact)
    for _ in range(3):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
        )
    assert hash_calls == [9, 9, 9]

    original = runtime.stat()
    replacement = (
        runtime.with_suffix(".replacement") if mutation == "replacement" else runtime
    )
    write_test_static_archive(replacement, b"runtime-b")
    os.utime(replacement, ns=(original.st_atime_ns, original.st_mtime_ns))
    if mutation == "replacement":
        os.replace(replacement, runtime)
    with pytest.raises(
        NativeLinkDependencyManifestError, match="archive digest mismatch"
    ):
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
        )
    assert hash_calls == [9, 9, 9, 9]


def test_rustc_native_static_lib_order_is_replayed_exactly(tmp_path: Path) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="native-link-order-family",
    )
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
            target_triple=target_triple,
            runtime_build_identity=build_identity,
        )
        return native_link_deps._collect_cargo_native_link_deps(
            runtime,
            target_triple=target_triple,
            object_format="elf",
            runtime_build_identity=build_identity,
        )

    good = flags("-lconsumer -lprovider")
    bad = flags("-lprovider -lconsumer")
    assert good[1::2] == ["-lconsumer", "-lprovider"]
    assert bad[1::2] == ["-lprovider", "-lconsumer"]
    for rendered_flags, names in (
        (good, ("libconsumer.a", "libprovider.a")),
        (bad, ("libprovider.a", "libconsumer.a")),
    ):
        for directory_flag, name in zip(rendered_flags[::2], names, strict=True):
            assert directory_flag.startswith(f"-L{runtime.parent}")
            custody_directory = Path(directory_flag[2:])
            assert custody_directory != lib_dir
            assert (custody_directory / name).read_bytes() == (
                b"consumer" if name == "libconsumer.a" else b"provider"
            )
    assert good != bad


def test_coff_rustc_linker_tokens_are_forwarded_through_driver_exactly(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "dev-fast" / "molt_runtime.lib"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple="x86_64-pc-windows-msvc",
        family_seed="coff-native-link-family",
    )
    write_native_link_dependency_manifest(
        "",
        cargo_stderr=(
            "note: native-static-libs: kernel32.lib /defaultlib:msvcrt kernel32.lib\n"
        ),
        runtime_lib=runtime,
        cargo_profile="dev-fast",
        target_triple="x86_64-pc-windows-msvc",
        runtime_build_identity=build_identity,
    )
    assert read_native_link_flags(
        runtime,
        target_triple="x86_64-pc-windows-msvc",
        object_format="coff",
        runtime_build_identity=build_identity,
    ) == [
        "-Wl,kernel32.lib",
        "-Wl,/defaultlib:msvcrt",
        "-Wl,kernel32.lib",
    ]


def test_rustc_lowered_native_flags_are_not_reconstructed(tmp_path: Path) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="native-link-lowered-flags-family",
    )
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
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )
    flags = native_link_deps._collect_cargo_native_link_deps(
        runtime,
        target_triple=target_triple,
        object_format="elf",
        runtime_build_identity=build_identity,
    )
    assert flags[:1] == [
        "-Wl,--whole-archive",
    ]
    assert flags[1].startswith(f"-L{runtime.parent}")
    custody_directory = Path(flags[1][2:])
    assert custody_directory != lib_dir
    assert (custody_directory / "libwhole.a").read_bytes() == b"archive"
    assert flags[2:] == [
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
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
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
    assert command[:8] == [
        "cargo",
        "rustc",
        "--color=never",
        "-p",
        "molt-runtime",
        "--profile",
        "dev-fast",
        "--message-format=json-render-diagnostics",
    ]
    assert command[-5:] == [
        "--crate-type",
        "staticlib",
        "--",
        "--print",
        "native-static-libs",
    ]
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
        "_run_resolved_cargo_plan",
        lambda plan, **_kwargs: (
            captured.append(list(plan.command))
            or subprocess.CompletedProcess(list(plan.command), 0, cargo_stdout, "")
        ),
    )
    monkeypatch.setattr(
        runtime_build,
        "_runtime_build_identity_for_plan",
        lambda *_args, **_kwargs: _RUNTIME_BUILD_IDENTITY,
    )
    plan = runtime_build._NativeRuntimeBuildPlan(
        runtime_lib=runtime,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        cargo_timeout=1.0,
        stage_timings_ms=None,
        runtime_state=None,
        cargo_plan=runtime_cargo_plan(
            tmp_path, fixture_root=runtime_fixture_root, env={}, cargo_command=command
        ),
        fingerprint_features=("stdlib_micro",),
        fingerprint_path=tmp_path / "runtime.fingerprint",
        stored_fingerprint=None,
        fingerprint=runtime_build.runtime_build_fingerprint(_RUNTIME_BUILD_IDENTITY),
        build_identity=_RUNTIME_BUILD_IDENTITY,
        session_key=None,
    )

    assert plan.refresh_manifest()
    assert captured == [list(plan.cargo_plan.command)]
    assert native_link_deps._collect_cargo_native_link_deps(
        runtime,
        object_format="elf",
        runtime_build_identity=_RUNTIME_BUILD_IDENTITY,
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
    monkeypatch.setattr(
        runtime_build,
        "_runtime_build_identity_for_plan",
        lambda *_a, **_k: _RUNTIME_BUILD_IDENTITY,
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
        runtime_build._NativeRuntimeBuildPlan,
        "refresh_manifest",
        lambda self: refreshes.append(self.runtime_lib) or True,
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


@pytest.mark.parametrize("rejection_stage", ["canonical", "copied"])
def test_hydration_without_matching_manifest_requires_exact_refresh(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    rejection_stage: str,
) -> None:
    runtime = tmp_path / "isolated" / "dev-fast" / "molt_runtime.stdlib_micro.lib"
    canonical = tmp_path / "canonical" / "dev-fast" / runtime.name
    canonical.parent.mkdir(parents=True)
    canonical.write_bytes(b"runtime")
    if rejection_stage == "copied":
        native_link_dependency_manifest_path(canonical).write_text(
            "{}", encoding="utf-8"
        )

        def read_copied_manifest(path: Path, **_kwargs) -> Mapping[str, object]:
            if path == canonical:
                return {
                    "custody": {
                        "schema": "molt.native-link-custody.v2",
                        "archive": None,
                        "entries": [],
                    }
                }
            raise NativeLinkDependencyManifestError(
                "cannot read copied native manifest"
            )

        monkeypatch.setattr(
            runtime_build, "read_native_link_dependency_manifest", read_copied_manifest
        )
    refreshes: list[Path] = []
    monkeypatch.setattr(
        runtime_build, "_build_state_root", lambda _root: tmp_path / "state"
    )
    monkeypatch.setattr(runtime_build, "_cargo_build_env", lambda: {})
    monkeypatch.setattr(runtime_build, "_cargo_target_root", lambda _root: tmp_path)
    monkeypatch.setattr(
        runtime_build,
        "_runtime_build_identity_for_plan",
        lambda *_a, **_k: _RUNTIME_BUILD_IDENTITY,
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

    def refresh(self) -> bool:
        records = list(
            (tmp_path / "state" / "build_failures").glob(
                "native-runtime-canonical-hydration-rejection-*.json"
            )
        )
        assert len(records) == 1
        assert (
            "cannot read"
            in json.loads(records[0].read_text(encoding="utf-8"))["summary"]
        )
        refreshes.append(self.runtime_lib)
        return True

    monkeypatch.setattr(
        runtime_build._NativeRuntimeBuildPlan, "refresh_manifest", refresh
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
    assert "Evidence:" in capsys.readouterr().err


def test_link_dependency_authority_cannot_return_to_build_directory_scanning() -> None:
    deps_source = inspect.getsource(native_link_deps._collect_cargo_native_link_deps)
    assert ".iterdir(" not in deps_source
    assert "read_text(" not in deps_source
    assert "read_native_link_flags(" in deps_source
    assert "runtime_build_identity=runtime_build_identity" in deps_source

    publication_source = inspect.getsource(runtime_build._publish_native_runtime_build)
    assert "write_native_link_dependency_manifest(" in publication_source
    refresh_source = inspect.getsource(
        runtime_build._NativeRuntimeBuildPlan.refresh_manifest
    )
    assert "_run_resolved_cargo_plan(" in refresh_source
    assert "identity_is_current(" in refresh_source
    assert "write_native_link_dependency_manifest(" in refresh_source
    command_source = inspect.getsource(runtime_build._native_runtime_cargo_command)
    assert '"rustc"' in command_source
    assert "--message-format=json-render-diagnostics" in command_source
    assert "RUNTIME_STATICLIB_ARTIFACTS.select_in(cmd)" in command_source
    assert "native-static-libs" in command_source
    manifest_source = inspect.getsource(native_link_deps.read_native_link_flags)
    assert "runtime_build_identity=runtime_build_identity" in manifest_source
    cargo_source = inspect.getsource(cargo_execution._run_resolved_cargo_plan)
    attempt_source = inspect.getsource(cargo_execution._run_cargo_attempt)
    assert "_sccache_wrapper_failure_reason" in cargo_source
    assert "without_sccache" not in cargo_source
    assert cargo_source.count("_run_cargo_attempt(") == 1
    assert cargo_source.count("plan.verify()") == 2
    assert attempt_source.count('encoding="utf-8"') == 1
    assert attempt_source.count('errors="strict"') == 1
