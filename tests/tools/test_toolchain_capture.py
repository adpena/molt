from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import gc
import gzip
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tracemalloc

import pytest

from tools.proof_queue_pkg import custody_cas, toolchain_capture


def _identity(path: Path, *, rows: int = 1) -> dict[str, object]:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    files = [
        {
            "resolved_path": str(path),
            "lexical_path": str(path),
            "sha256": digest,
            "size": path.stat().st_size,
            "relative": f"copy-{index}",
        }
        for index in range(rows)
    ]
    return {
        "python": {
            "executable": str(path),
            "executable_sha256": digest,
            "implementation": "CPython",
            "version": "3.12.test",
            "runtime_closure_sha256": "a" * 64,
            "distribution_inventory_sha256": "b" * 64,
            "identity_sha256": "c" * 64,
            "runtime": {
                "runtime_file_count": 1,
                "runtime_unique_file_count": 1,
                "explicit_authority_files": files,
            },
            "distributions": [
                {
                    "name": "fixture",
                    "version": "1",
                    "installed_files": files,
                    "file_manifest_sha256": "d" * 64,
                }
            ],
            "inventory_profile": {"total_s": 1.0},
        }
    }


def test_toolchain_capture_cas_is_atomic_and_content_addressed(tmp_path: Path) -> None:
    owned = tmp_path / "owned.py"
    owned.write_text("owned\n", encoding="utf-8")
    payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": "unit",
        "rows": list(range(2_000)),
    }
    with ThreadPoolExecutor(max_workers=8) as executor:
        references = list(
            executor.map(
                lambda _index: custody_cas.put_json(tmp_path / "cas", payload),
                range(16),
            )
        )
    assert len({reference.path for reference in references}) == 1
    assert len({reference.blob_sha256 for reference in references}) == 1
    custody_cas.verify_ref(references[0].as_dict(), expected_root=tmp_path / "cas")
    reference_path = Path(references[0].path)
    assert reference_path.name == f"{references[0].blob_sha256[2:]}.json.gz"
    assert reference_path.parent.name == references[0].blob_sha256[:2]
    assert reference_path.parent.parent.name == "sha256"
    assert reference_path.parent.parent.parent.name == "blobs"
    assert not list((tmp_path / "cas").rglob(".custody-*"))


def test_toolchain_capture_cas_rejects_corruption(tmp_path: Path) -> None:
    reference = custody_cas.put_json(
        tmp_path / "cas", {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": "unit"}
    )
    path = Path(reference.path)
    content = bytearray(path.read_bytes())
    content[len(content) // 2] ^= 0xFF
    path.write_bytes(content)
    with pytest.raises(ValueError, match="blob digest changed"):
        custody_cas.verify_ref(reference.as_dict(), expected_root=tmp_path / "cas")


def test_toolchain_capture_frozen_manifest_rehash_detects_mutation(
    tmp_path: Path,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_text("before\n", encoding="utf-8")
    summaries, reference, telemetry = toolchain_capture.publish_capture(
        tmp_path / "cas", _identity(owned, rows=10)
    )
    assert telemetry["full_capture_count"] == 1
    # Ordinary installed inventories live only in CAS; only editable source
    # custody remains in the compact receipt summary.
    assert summaries["python"]["distributions"] == []  # type: ignore[index]
    assert toolchain_capture.verify_capture(
        reference, workers=2, cas_root=tmp_path / "cas"
    )["stable"] is True
    owned.write_text("after\n", encoding="utf-8")
    verification = toolchain_capture.verify_capture(
        reference, workers=2, cas_root=tmp_path / "cas"
    )
    assert verification["stable"] is False
    assert verification["mismatches"][0]["path"] == str(owned)  # type: ignore[index]


def test_toolchain_capture_deduplicates_references_and_rejects_conflicts(
    tmp_path: Path,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_text("owned\n", encoding="utf-8")
    identity = _identity(owned, rows=1_000)
    assert len(toolchain_capture.frozen_files(identity)) == 1
    conflict = json.loads(json.dumps(identity))
    conflict["python"]["distributions"][0]["installed_files"][0]["sha256"] = "f" * 64
    with pytest.raises(ValueError, match="conflicting identities"):
        toolchain_capture.frozen_files(conflict)


def test_toolchain_capture_compact_receipt_allocation_benchmark(tmp_path: Path) -> None:
    owned = tmp_path / "owned.py"
    owned.write_text("owned\n", encoding="utf-8")
    identity = _identity(owned, rows=5_000)
    legacy = {
        "toolchains": identity,
        "toolchain_custody": {
            "prelaunch": identity,
            "postcompletion": identity,
        },
    }
    tracemalloc.start()
    legacy_bytes = json.dumps(legacy, sort_keys=True).encode()
    _legacy_current, legacy_peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    legacy_size = len(legacy_bytes)
    del legacy_bytes, legacy
    gc.collect()
    tracemalloc.start()
    summaries, reference, telemetry = toolchain_capture.publish_capture(
        tmp_path / "cas", identity
    )
    compact = {
        "toolchains": summaries,
        "toolchain_custody": {
            "prelaunch": summaries,
            "postcompletion": summaries,
            "identical": True,
        },
        "toolchain_capture": {"artifact": reference, "telemetry": telemetry},
    }
    compact_bytes = json.dumps(compact, sort_keys=True).encode()
    _compact_current, compact_peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    assert len(compact_bytes) < legacy_size // 100
    assert len(compact_bytes) < 64 * 1024
    assert reference["compressed_bytes"] < reference["uncompressed_bytes"] // 10
    assert compact_peak < legacy_peak


def test_custody_file_publication_is_immutable_after_source_changes(
    tmp_path: Path,
) -> None:
    source = tmp_path / "supervisor.exe"
    source.write_bytes(b"first-supervisor")
    reference = custody_cas.put_file(
        tmp_path / "cas", source, logical_name=source.name, executable=True
    ).as_dict()
    source.write_bytes(b"changed-after-publication")
    custody_cas.verify_file_ref(reference, expected_root=tmp_path / "cas")
    assert Path(str(reference["path"])).read_bytes() == b"first-supervisor"
    assert reference["path"] != str(source)


def test_custody_cas_binds_media_root_layout_and_rejects_links(
    tmp_path: Path,
) -> None:
    root = tmp_path / "cas"
    reference = custody_cas.put_json(
        root, {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": "root-bound"}
    ).as_dict()

    wrong_media = dict(reference)
    wrong_media["media_type"] = "application/json"
    with pytest.raises(ValueError, match="media type mismatch"):
        custody_cas.verify_ref(wrong_media, expected_root=root)

    other_root = tmp_path / "other-cas"
    other_root.mkdir()
    with pytest.raises(ValueError, match="canonical CAS layout"):
        custody_cas.verify_ref(reference, expected_root=other_root)

    original = Path(str(reference["path"]))
    external = tmp_path / "outside.json.gz"
    external.write_bytes(original.read_bytes())
    original.unlink()
    try:
        original.symlink_to(external)
    except OSError as exc:
        pytest.skip(f"file symlinks unavailable on this host: {exc}")
    with pytest.raises(ValueError, match="link or junction"):
        custody_cas.verify_ref(reference, expected_root=root)


def test_custody_file_mode_is_part_of_immutable_namespace_and_contract(
    tmp_path: Path,
) -> None:
    root = tmp_path / "cas"
    source = tmp_path / "payload.bin"
    source.write_bytes(b"same immutable bytes")
    executable = custody_cas.put_file(root, source, executable=True).as_dict()
    data = custody_cas.put_file(root, source, executable=False).as_dict()

    assert executable["path"] != data["path"]
    assert Path(str(executable["path"])).parts[-4] == "executable"
    assert Path(str(data["path"])).parts[-4] == "data"
    custody_cas.verify_file_ref(executable, expected_root=root)
    custody_cas.verify_file_ref(data, expected_root=root)

    wrong_media = dict(data)
    wrong_media["media_type"] = "application/x-executable"
    with pytest.raises(ValueError, match="media type mismatch"):
        custody_cas.verify_file_ref(wrong_media, expected_root=root)
    (tmp_path / "wrong-root").mkdir()
    with pytest.raises(ValueError, match="canonical CAS layout"):
        custody_cas.verify_file_ref(data, expected_root=tmp_path / "wrong-root")

    executable_as_data = dict(executable)
    executable_as_data["executable"] = False
    with pytest.raises(ValueError, match="canonical CAS layout"):
        custody_cas.verify_file_ref(executable_as_data, expected_root=root)
    data_as_executable = dict(data)
    data_as_executable["executable"] = True
    with pytest.raises(ValueError, match="canonical CAS layout"):
        custody_cas.verify_file_ref(data_as_executable, expected_root=root)

    if os.name != "nt":
        Path(str(executable["path"])).chmod(0o444)
        with pytest.raises(ValueError, match="executable file mode changed"):
            custody_cas.verify_file_ref(executable, expected_root=root)
        Path(str(data["path"])).chmod(0o555)
        with pytest.raises(ValueError, match="non-executable file mode changed"):
            custody_cas.verify_file_ref(data, expected_root=root)


def _write_raw_cas_blob(
    root: Path,
    compressed: bytes,
    *,
    semantic: bytes,
    declared_uncompressed_bytes: int | None = None,
) -> dict[str, object]:
    blob_sha256 = hashlib.sha256(compressed).hexdigest()
    path = root / "blobs" / "sha256" / blob_sha256[:2] / f"{blob_sha256[2:]}.json.gz"
    path.parent.mkdir(parents=True)
    path.write_bytes(compressed)
    return {
        "schema": custody_cas.REF_SCHEMA,
        "path": str(path.resolve()),
        "media_type": custody_cas.JSON_GZIP_MEDIA_TYPE,
        "blob_sha256": blob_sha256,
        "semantic_sha256": hashlib.sha256(semantic).hexdigest(),
        "compressed_bytes": len(compressed),
        "uncompressed_bytes": (
            len(semantic)
            if declared_uncompressed_bytes is None
            else declared_uncompressed_bytes
        ),
    }


def test_custody_cas_streaming_reader_rejects_trailing_and_overexpansion(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    trailing_root = tmp_path / "trailing"
    semantic = json.dumps(
        {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": "bounded"},
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    trailing = _write_raw_cas_blob(
        trailing_root, gzip.compress(semantic, mtime=0) + b"trailing", semantic=semantic
    )
    with pytest.raises(ValueError, match="trailing gzip data"):
        custody_cas.verify_ref(trailing, expected_root=trailing_root)

    expansion_root = tmp_path / "expansion"
    expanded = json.dumps(
        {"schema": custody_cas.ARTIFACT_SCHEMA, "rows": ["x" * 4096]},
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    overexpanded = _write_raw_cas_blob(
        expansion_root,
        gzip.compress(expanded, mtime=0),
        semantic=expanded,
        declared_uncompressed_bytes=128,
    )
    monkeypatch.setattr(custody_cas, "MAX_UNCOMPRESSED_BYTES", 128)
    with pytest.raises(ValueError, match="declared uncompressed size"):
        custody_cas.verify_ref(overexpanded, expected_root=expansion_root)

    oversized = dict(overexpanded)
    oversized["compressed_bytes"] = custody_cas.MAX_COMPRESSED_BYTES + 1
    with pytest.raises(ValueError, match="compressed size ceiling"):
        custody_cas.verify_ref(oversized, expected_root=expansion_root)

    monkeypatch.setattr(
        custody_cas,
        "MAX_COMPRESSED_BYTES",
        int(overexpanded["compressed_bytes"]) - 1,
    )
    with pytest.raises(ValueError, match="compressed size ceiling"):
        custody_cas.verify_ref(overexpanded, expected_root=expansion_root)


@pytest.mark.skipif(os.name == "nt", reason="directory fsync is POSIX custody")
def test_custody_cas_recursively_fsyncs_new_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    real_fsync = os.fsync
    directory_fsyncs: list[str] = []

    def observed_fsync(descriptor: int) -> None:
        if stat.S_ISDIR(os.fstat(descriptor).st_mode):
            directory_fsyncs.append("directory")
        real_fsync(descriptor)

    monkeypatch.setattr(custody_cas.os, "fsync", observed_fsync)
    reference = custody_cas.put_json(
        tmp_path / "new" / "nested" / "cas",
        {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": "durable-directories"},
    )
    assert directory_fsyncs
    custody_cas.verify_ref(
        reference.as_dict(), expected_root=tmp_path / "new" / "nested" / "cas"
    )


def test_rust_link_capture_uses_exact_target_environment_and_selected_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cargo = tmp_path / ("cargo.exe" if os.name == "nt" else "cargo")
    rustc = tmp_path / ("rustc.exe" if os.name == "nt" else "rustc")
    linker = tmp_path / (
        "selected-linker.exe" if os.name == "nt" else "selected-linker"
    )
    for path in (cargo, rustc, linker):
        path.write_bytes(path.name.encode())
        path.chmod(0o755)
    observed: list[tuple[list[str], dict[str, str]]] = []
    observed_manifests: list[str] = []

    def fake_run(command, **kwargs):
        argv = [str(value) for value in command]
        environment = dict(kwargs["env"])
        observed.append((argv, environment))
        if "--manifest-path" in argv:
            observed_manifests.append(
                Path(argv[argv.index("--manifest-path") + 1]).read_text(
                    encoding="utf-8"
                )
            )
        return subprocess.CompletedProcess(
            argv,
            0,
            json.dumps(str(linker)) + ' "--exact-probe-argument"\n',
            "",
        )

    monkeypatch.setattr(toolchain_capture.subprocess, "run", fake_run)
    environment = {
        "PATH": str(tmp_path),
        "CARGO_TARGET_TEST_TRIPLE_LINKER": str(linker),
    }
    command_argv = [
        "cargo",
        "rustc",
        "--profile",
        "frontier-fast",
        "--features",
        "pkg/fast,simd",
        "--",
        "-C",
        f"linker={linker}",
        "-Clink-arg=/DEBUG:NONE",
        "--crate-type",
        "cdylib",
    ]
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=rustc,
        cargo=cargo,
        cwd=tmp_path,
        env=environment,
        target="test-triple",
        command_argv=command_argv,
    )
    assert observed[0][0][0] == str(cargo)
    assert observed[0][0][-2:] == ["--print", "link-args"]
    assert observed[0][0][observed[0][0].index("--target") + 1] == "test-triple"
    assert observed[0][0][observed[0][0].index("--profile") + 1] == "frontier-fast"
    assert observed[0][0][observed[0][0].index("--features") + 1] == "fast,simd"
    assert ["-C", f"linker={linker}"] == observed[0][0][
        observed[0][0].index("-C") : observed[0][0].index("-C") + 2
    ]
    assert "-Clink-arg=/DEBUG:NONE" in observed[0][0]
    assert observed[0][0][observed[0][0].index("--crate-type") + 1] == "cdylib"
    assert '[profile.frontier-fast]\ninherits="release"' in observed_manifests[0]
    assert observed[0][1]["CARGO_TARGET_TEST_TRIPLE_LINKER"] == str(linker)
    assert images == [
        {
            "schema": toolchain_capture.PROCESS_IMAGE_SCHEMA,
            "role": "rust-linker",
            "path": str(linker.resolve()),
            "sha256": hashlib.sha256(linker.read_bytes()).hexdigest(),
            "size_bytes": linker.stat().st_size,
        }
    ]
    assert telemetry["target"] == "test-triple"
    assert telemetry["selected_process_count"] == 1
    assert telemetry["selection_probe_count"] == 1

    selected_identity = {
        "process_images": images,
        "link_selection": telemetry,
    }
    revalidated, reused_telemetry = (
        toolchain_capture.revalidate_rust_link_process_images(
            selected_identity, target="test-triple", command_argv=command_argv
        )
    )
    assert revalidated == images
    assert reused_telemetry == telemetry
    assert len(observed) == 1

    linker.write_bytes(b"substituted-linker")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            selected_identity, target="test-triple", command_argv=command_argv
        )
