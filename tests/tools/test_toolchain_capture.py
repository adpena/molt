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
import sys
import tracemalloc
from types import SimpleNamespace

import pytest

from molt.exact_json import canonical_json_sha256
from tools.proof_queue_pkg import (
    custody_cas,
    process_image_capture,
    toolchain_capture,
)


def _identity(path: Path, *, rows: int = 1) -> dict[str, object]:
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    files = [
        {
            "path": str(path),
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
            "schema": "molt.proof-python-toolchain.v3",
            "identity_kind": "executable",
            "identity_sha256": "c" * 64,
            "location": {"selected_executable": str(path)},
            "environment": {
                "implementation": "cpython",
                "version": "3.12.test",
                "environment_closure_sha256": "b" * 64,
                "runtime": {
                    "runtime_closure_sha256": "a" * 64,
                    "file_nodes": [{"id": "file-0", "sha256": digest}],
                    "runtime_roots": [{"entries": files}],
                },
                "distributions": [
                    {
                        "name": "fixture",
                        "version": "1",
                        "installed_files": files,
                        "file_manifest_sha256": "d" * 64,
                    }
                ],
                "external_roots": [],
            },
            "file_custody": files,
            "node_custody": [{"node": "/runtime/file_nodes/0", "file_index": 0}],
            "process_images": [],
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


@pytest.mark.parametrize("kind", ["json", "file"])
@pytest.mark.parametrize("corrupt_competitor", [False, True])
def test_custody_cas_publication_collision_preserves_and_verifies_winner(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    kind: str,
    corrupt_competitor: bool,
) -> None:
    root = tmp_path / "cas"
    source = tmp_path / "payload.bin"
    source.write_bytes(b"immutable payload")
    competitors: dict[Path, bytes] = {}
    real_publish = custody_cas.file_publication.durable_publish_exclusive

    def compete(staged: Path, target: Path) -> None:
        content = bytearray(staged.read_bytes())
        if corrupt_competitor:
            # Alter gzip's timestamp without changing decompression behavior:
            # the transport digest itself must reject an otherwise-valid blob.
            content[4 if kind == "json" else len(content) // 2] ^= 0xFF
        target.write_bytes(content)
        if os.name != "nt":
            target.chmod(stat.S_IMODE(staged.stat().st_mode))
        competitors[target] = bytes(content)
        real_publish(staged, target)

    def publish() -> dict[str, object]:
        if kind == "json":
            return custody_cas.put_json(
                root, {"schema": custody_cas.ARTIFACT_SCHEMA, "kind": "collision"}
            ).as_dict()
        return custody_cas.put_file(root, source).as_dict()

    monkeypatch.setattr(
        custody_cas.file_publication, "durable_publish_exclusive", compete
    )
    if corrupt_competitor:
        with pytest.raises(ValueError, match="digest changed"):
            publish()
    else:
        reference = publish()
        verify = (
            custody_cas.verify_ref if kind == "json" else custody_cas.verify_file_ref
        )
        verify(reference, expected_root=root)
    assert len(competitors) == 1
    assert all(path.read_bytes() == content for path, content in competitors.items())
    assert not list(root.rglob(".custody-*"))


@pytest.mark.skipif(os.name == "nt", reason="POSIX immutable mode contract")
def test_custody_cas_publication_collision_rejects_winner_with_mutable_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = tmp_path / "cas"
    source = tmp_path / "payload.bin"
    source.write_bytes(b"immutable payload")
    real_publish = custody_cas.file_publication.durable_publish_exclusive

    def compete(staged: Path, target: Path) -> None:
        target.write_bytes(staged.read_bytes())
        target.chmod(0o644)
        real_publish(staged, target)

    monkeypatch.setattr(
        custody_cas.file_publication, "durable_publish_exclusive", compete
    )
    with pytest.raises(ValueError, match="non-executable file mode changed"):
        custody_cas.put_file(root, source)
    assert not list(root.rglob(".custody-*"))


def test_toolchain_capture_frozen_manifest_rehash_detects_mutation(
    tmp_path: Path,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_text("before\n", encoding="utf-8")
    summaries, reference, telemetry = toolchain_capture.publish_capture(
        tmp_path / "cas", _identity(owned, rows=10)
    )
    assert telemetry["full_capture_count"] == 1
    # Bulky file-node and installed-file inventories live only in CAS.
    summary = summaries["python"]
    assert "file_custody" not in summary  # type: ignore[operator]
    assert summary["environment"]["distributions"] == []  # type: ignore[index]
    assert summary["environment"]["distribution_count"] == 1  # type: ignore[index]
    assert summary["version"] == "3.12.test"  # type: ignore[index]
    assert (
        toolchain_capture.verify_capture(
            reference, workers=2, cas_root=tmp_path / "cas"
        )["stable"]
        is True
    )
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
    conflict["python"]["environment"]["distributions"][0]["installed_files"][0][
        "sha256"
    ] = "f" * 64
    with pytest.raises(ValueError, match="conflicting identities"):
        toolchain_capture.frozen_files(conflict)


def test_python_relative_node_inventory_keeps_full_file_custody_in_cas(
    tmp_path: Path,
) -> None:
    owned = tmp_path / "stdlib" / "module.py"
    owned.parent.mkdir()
    owned.write_bytes(b"before")
    identity = _identity(owned)
    python = identity["python"]
    python["environment"]["runtime"]["runtime_roots"] = [
        {"id": "runtime-0", "entries": [{"path": "module.py", "node": "file-0"}]}
    ]
    python["environment"]["distributions"] = []
    summaries, reference, telemetry = toolchain_capture.publish_capture(
        tmp_path / "cas", identity
    )
    assert telemetry["frozen_file_count"] == 1
    assert "file_custody" not in summaries["python"]
    assert "node_custody" not in summaries["python"]
    loaded = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
    assert loaded["files"][0]["path"] == str(owned)
    assert loaded["toolchains"]["python"]["file_custody"] == python["file_custody"]
    assert loaded["toolchains"]["python"]["node_custody"] == python["node_custody"]
    owned.write_bytes(b"after!")
    verification = toolchain_capture.verify_capture(
        reference, workers=1, cas_root=tmp_path / "cas"
    )
    assert verification["stable"] is False
    assert verification["mismatches"][0]["path"] == str(owned)


@pytest.mark.parametrize(
    "damage", ["missing", "empty", "relative", "boolean-size", "digest"]
)
def test_python_capture_rejects_missing_or_malformed_frozen_file_authority(
    tmp_path: Path, damage: str
) -> None:
    owned = tmp_path / "module.py"
    owned.write_bytes(b"owned")
    identity = _identity(owned)
    python = identity["python"]
    if damage == "missing":
        del python["file_custody"]
    elif damage == "empty":
        python["file_custody"] = []
    elif damage == "relative":
        python["file_custody"][0]["path"] = "module.py"
    elif damage == "boolean-size":
        python["file_custody"][0]["size"] = True
    else:
        python["file_custody"][0]["sha256"] = "g" * 64
    with pytest.raises(ValueError, match="frozen file custody"):
        toolchain_capture.frozen_files(identity)


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


@pytest.mark.parametrize("image_count", [0, 48, 2_000])
def test_compact_process_inventories_are_bounded_and_full_capture_is_preserved(
    tmp_path: Path,
    image_count: int,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_bytes(b"owned\n")
    identity = _identity(owned)
    owned_sha256 = hashlib.sha256(owned.read_bytes()).hexdigest()
    images = [
        {
            "role": f"image-{index}",
            "path": str(owned),
            "sha256": owned_sha256,
            "selection": {"details": "full captured selection" * 20},
        }
        for index in range(image_count)
    ]
    inventory = [{"observed_image_count": image_count, "images": images}]
    selection = {"selection_probe_count": 1, "selected_images": images}
    identity["python"]["process_images"] = images
    for name in ("cargo", "rustc", "git", "uv"):
        identity[name] = {
            "identity_sha256": "c" * 64,
            "process_images": images,
            "process_image_inventories": inventory,
            "link_selection": selection,
        }
    compact = toolchain_capture.compact_toolchains(identity)
    # Production's non-toolchain custody already occupies ~52KiB. The summary
    # must stay bounded even when runtime/helper inventories grow by thousands.
    assert len(json.dumps(compact, sort_keys=True).encode()) < 8 * 1024
    for name, full in identity.items():
        for field in ("process_images", "process_image_inventories", "link_selection"):
            if field in full:
                assert compact[name][field] == {
                    "count": len(full[field]),
                    "semantic_sha256": canonical_json_sha256(full[field]),
                }
    summaries, reference, _telemetry = toolchain_capture.publish_capture(
        tmp_path / "cas",
        identity,
    )
    assert summaries == compact
    full_capture = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
    assert full_capture["toolchains"] == identity
    assert toolchain_capture.compact_toolchains(full_capture["toolchains"]) == compact
    # Same count, different selection: digest authority must still notice.
    identity["rustc"]["link_selection"] = {**selection, "selection_probe_count": 2}
    changed = toolchain_capture.compact_toolchains(identity)
    assert (
        changed["rustc"]["link_selection"]["count"]
        == compact["rustc"]["link_selection"]["count"]
    )
    assert changed["rustc"]["link_selection"] != compact["rustc"]["link_selection"]


@pytest.mark.parametrize("toolchain", ["python", "rustc"])
@pytest.mark.parametrize(
    "field,value",
    [
        ("process_images", {"count": 1}),
        ("process_image_inventories", "missing"),
        ("link_selection", []),
    ],
)
def test_compact_process_inventory_rejects_malformed_or_already_compact_input(
    tmp_path: Path,
    toolchain: str,
    field: str,
    value: object,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_bytes(b"owned\n")
    identity = _identity(owned)
    identity.setdefault(toolchain, {})[field] = value
    with pytest.raises(ValueError, match=f"{field} has malformed inventory"):
        toolchain_capture.compact_toolchains(identity)


def test_compact_python_package_count_does_not_expand_receipt_or_allocation(
    tmp_path: Path,
) -> None:
    owned = tmp_path / "owned.py"
    owned.write_bytes(b"owned\n")
    editable = {
        "name": "editable-fixture",
        "version": "1",
        "file_manifest_sha256": "d" * 64,
        "direct_url_sha256": "e" * 64,
        "record_sha256": "f" * 64,
        "external_source": {"root": "external-root-0", "path": "src"},
    }
    peaks: list[int] = []
    sizes: list[int] = []
    for package_count in (0, 2_000):
        identity = _identity(owned)
        bindings = [
            {"node": f"/tree/file_nodes/{index}", "file_index": index}
            for index in range(package_count)
        ]
        identity["python"]["node_custody"] = bindings
        environment = identity["python"]["environment"]
        distributions = [
            editable,
            *[
                {
                    "name": f"package-{index:04}",
                    "version": "1",
                    "file_manifest_sha256": "a" * 64,
                    "direct_url_sha256": "b" * 64,
                    "record_sha256": "c" * 64,
                    "external_source": None,
                }
                for index in range(package_count)
            ],
        ]
        environment["distributions"] = distributions
        environment["distribution_inventory_sha256"] = canonical_json_sha256(
            distributions
        )
        environment["external_roots"] = [
            {"id": "external-root-0", "path": str(tmp_path)}
        ]
        gc.collect()
        tracemalloc.start()
        try:
            summaries = toolchain_capture.compact_toolchains(identity)
            receipt = {
                "toolchains": summaries,
                "toolchain_custody": {
                    "prelaunch": summaries,
                    "postcompletion": summaries,
                    "identical": True,
                },
            }
            encoded = json.dumps(receipt, sort_keys=True).encode()
            _current, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        peaks.append(peak)
        sizes.append(len(encoded))
        compact_environment = summaries["python"]["environment"]
        assert "node_custody" not in summaries["python"]
        assert compact_environment["distributions"] == [editable]
        assert compact_environment["distribution_count"] == package_count + 1
        assert (
            compact_environment["distribution_inventory_sha256"]
            == (environment["distribution_inventory_sha256"])
        )
        assert compact_environment["external_roots"] == environment["external_roots"]
        published, reference, _telemetry = toolchain_capture.publish_capture(
            tmp_path / "cas", identity
        )
        assert published == summaries
        captured = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
        assert captured["toolchains"]["python"]["environment"]["distributions"] == (
            distributions
        )
        assert captured["toolchains"]["python"]["node_custody"] == bindings
    assert max(sizes) < 64 * 1024
    assert sizes[1] <= sizes[0] + 64
    assert peaks[1] <= peaks[0] + 16 * 1024


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


@pytest.mark.parametrize(
    ("stdout", "stderr"),
    [
        ('"/usr/bin/cc" "-o" "probe"\n', ""),
        ("", 'note: emitted on stderr\n"/usr/bin/cc" "-o" "probe"\n'),
        ('LC_ALL="C" PATH="/rust/lib:/usr/bin" "/usr/bin/cc" "-o" "probe"\n', ""),
        ("", 'LC_ALL="C" PATH="/rust/lib:/usr/bin" "/usr/bin/cc" "-o" "probe"\n'),
        (
            'RUST_EMPTY="" RUST_FLAGS="value with spaces" "/usr/bin/cc" "-o" "probe"\n',
            "",
        ),
        (
            'RUST_FLAGS="escaped \\"quote\\" and \\\\path" "/usr/bin/cc" "-o" "probe"\n',
            "",
        ),
        # rustc strips Apple deployment targets with env_remove, which
        # std::process::Command's Debug form prints as an `env -u` prefix.
        (
            "env -u IPHONEOS_DEPLOYMENT_TARGET -u TVOS_DEPLOYMENT_TARGET "
            'LC_ALL="C" PATH="/rust/lib:/usr/bin" "/usr/bin/cc" "-o" "probe"\n',
            "",
        ),
        ('env -i PATH="/usr/bin" "/usr/bin/cc" "-o" "probe"\n', ""),
        ('cd "/work dir" && env -u SDKROOT "/usr/bin/cc" "-o" "probe"\n', ""),
        ('["/usr/bin/cc"] "cc-display-name" "-o" "probe"\n', ""),
    ],
)
def test_rust_link_selection_accepts_exactly_one_command_from_either_channel(
    stdout: str, stderr: str
) -> None:
    assert toolchain_capture._selected_rust_link_command(stdout, stderr) == [
        "/usr/bin/cc",
        "-o",
        "probe",
    ]


@pytest.mark.parametrize(
    ("stdout", "stderr", "count"),
    [
        ("selection emitted no quoted command\n", "", 0),
        ('LC_ALL="C" PATH="/usr/bin"\n', "", 0),
        ('LC_ALL="C" not a command\n', "", 0),
        ('LC_ALL="C""/usr/bin/cc" "-o" "probe"\n', "", 0),
        ('env -u SDKROOT PATH="/usr/bin"\n', "", 0),
        ('cd "/work" "/usr/bin/cc" "-o" "probe"\n', "", 0),
        ('["/usr/bin/cc" "cc" "-o" "probe"\n', "", 0),
        (
            'LC_ALL="C" "/usr/bin/cc" "one"\n',
            'PATH="/usr/bin" "/usr/bin/ld" "two"\n',
            2,
        ),
        (
            '"/usr/bin/cc" "one"\n',
            '"/usr/bin/ld" "two"\n',
            2,
        ),
    ],
)
def test_rust_link_selection_fails_closed_on_zero_or_multiple_commands(
    stdout: str, stderr: str, count: int
) -> None:
    with pytest.raises(ValueError, match=rf"returned {count} commands"):
        toolchain_capture._selected_rust_link_command(stdout, stderr)


# Shapes from GitHub's runners: gcc 13 quotes only arguments with characters
# outside [A-Za-z0-9_./-]; Apple clang quotes every argument. Both escape a
# double quote, backslash, or dollar sign inside quotes.
_GCC_DRY_RUN = (
    "Using built-in specs.\n"
    "COLLECT_GCC=cc\n"
    "Target: x86_64-linux-gnu\n"
    "COLLECT_GCC_OPTIONS='-m64' '-o' '/tmp/probe'\n"
    " /usr/libexec/gcc/x86_64-linux-gnu/13/collect2 -plugin"
    ' "-plugin-opt=-fresolution=/tmp/cc.res" --build-id -o /tmp/probe'
    ' "" "quote\\"slash\\\\dollar\\$"\n'
)
_CLANG_DRY_RUN = (
    "Apple clang version 21.0.0 (clang-2100.1.1.101)\n"
    "Target: arm64-apple-darwin25.6.0\n"
    "InstalledDir: /Applications/Xcode.app/usr/bin\n"
    ' "/Applications/Xcode.app/usr/bin/ld" "-demangle" "-arch" "arm64"'
    ' "-o" "/Users/runner/probe"\n'
)


def test_driver_dry_run_reports_gcc_and_clang_helper_commands() -> None:
    assert toolchain_capture._driver_command_lines(_GCC_DRY_RUN) == [
        [
            "/usr/libexec/gcc/x86_64-linux-gnu/13/collect2",
            "-plugin",
            "-plugin-opt=-fresolution=/tmp/cc.res",
            "--build-id",
            "-o",
            "/tmp/probe",
            "",
            'quote"slash\\dollar$',
        ]
    ]
    assert toolchain_capture._driver_command_lines(_CLANG_DRY_RUN) == [
        [
            "/Applications/Xcode.app/usr/bin/ld",
            "-demangle",
            "-arch",
            "arm64",
            "-o",
            "/Users/runner/probe",
        ]
    ]


@pytest.mark.parametrize(
    "line",
    [
        ' "unterminated',
        ' "unknown \\n escape"',
        ' "joined""argument"',
        ' bare"quote',
        "  doubly-indented banner",
        "Target: column-zero banner",
    ],
)
def test_driver_dry_run_skips_malformed_or_banner_lines(line: str) -> None:
    assert toolchain_capture._driver_command_lines(line + "\n") == []


def _rust_metadata_probe(command, root: Path):
    print_kinds = [
        command[index + 1]
        for index, value in enumerate(command[:-1])
        if value == "--print"
    ]
    assert "link-args" not in print_kinds or all(
        kind in {"link-args", "native-static-libs"} for kind in print_kinds
    ), "metadata-only print requests stop rustc before linking"
    if list(command[1:]) == ["-vV"]:
        host = (
            "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"
        )
        return subprocess.CompletedProcess(
            command, 0, f"rustc fixture\nhost: {host}\n", ""
        )
    if list(command[1:]) == ["--print", "sysroot"]:
        return subprocess.CompletedProcess(command, 0, str(root) + "\n", "")
    if "sysroot" in print_kinds:
        selected = (
            command[command.index("--sysroot") + 1] if "--sysroot" in command else root
        )
        selected = next(
            (
                value.split("=", 1)[1]
                for value in command
                if value.startswith("--sysroot=")
            ),
            selected,
        )
        return subprocess.CompletedProcess(command, 0, str(selected) + "\n", "")
    return None


@pytest.mark.parametrize(
    "command_prefix", ["", 'LC_ALL="C" PATH="/rust/lib:/usr/bin" ']
)
def test_rust_link_capture_uses_exact_target_environment_and_selected_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command_prefix: str
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
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        argv = [str(value) for value in command]
        environment = dict(kwargs["env"])
        observed.append((argv, environment))
        if "--manifest-path" in argv:
            probe_root = Path(argv[argv.index("--manifest-path") + 1]).parent
            if "--lib" in argv:
                assert (probe_root / "host.rs").is_file()
                assert not (probe_root / "main.rs").exists()
            else:
                assert (probe_root / "main.rs").is_file()
                assert not (probe_root / "host.rs").exists()
            observed_manifests.append(
                Path(argv[argv.index("--manifest-path") + 1]).read_text(
                    encoding="utf-8"
                )
            )
        return subprocess.CompletedProcess(
            argv,
            0,
            command_prefix + json.dumps(str(linker)) + ' "--exact-probe-argument"\n',
            "",
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=fake_run))
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
        "--config",
        "build.incremental=false",
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
    assert "[[bin]]" in observed_manifests[0] and "[lib]" not in observed_manifests[0]
    assert "[lib]" in observed_manifests[1] and "[[bin]]" not in observed_manifests[1]
    assert observed[0][1]["CARGO_TARGET_TEST_TRIPLE_LINKER"] == str(linker)
    assert images == [
        {
            "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
            "role": "rust-linker",
            "path": str(linker.resolve()),
            "sha256": hashlib.sha256(linker.read_bytes()).hexdigest(),
            "size_bytes": linker.stat().st_size,
        }
    ]
    assert telemetry["target"] == "test-triple"
    assert telemetry["selected_process_count"] == 1
    assert telemetry["selection_probe_count"] == 2
    assert [unit["unit"] for unit in telemetry["units"]] == [
        "target",
        "host-proc-macro",
    ]
    assert "--lib" in observed[1][0]
    assert "-Clink-arg=/DEBUG:NONE" not in observed[1][0]
    assert "--crate-type" not in observed[1][0]
    assert all(
        argv[argv.index("--config") + 1] == "build.incremental=false"
        for argv, _ in observed
    )

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
    assert len(observed) == 2

    linker.write_bytes(b"substituted-linker")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            selected_identity, target="test-triple", command_argv=command_argv
        )


@pytest.mark.parametrize(
    "command_prefix", ["", 'LC_ALL="C" PATH="/rust/lib:/usr/bin" ']
)
@pytest.mark.parametrize("cargo_mode", [False, True])
def test_rust_capture_metadata_and_link_prints_are_disjoint_real_rustc_phases(
    tmp_path, monkeypatch, cargo_mode, command_prefix
):
    linker = tmp_path / ("linker.exe" if os.name == "nt" else "linker")
    linker.write_bytes(b"linker")
    linker.chmod(0o755)
    phases = []

    def run(command, **kwargs):
        kinds = [
            command[index + 1]
            for index, value in enumerate(command[:-1])
            if value == "--print"
        ]
        if "--crate-name" in command or "--manifest-path" in command:
            phases.append(kinds)
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        assert kinds == ["link-args"], (
            "rustc links only after metadata early-exit requests are removed"
        )
        return subprocess.CompletedProcess(
            command, 0, command_prefix + json.dumps(str(linker)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    _, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=tmp_path / "rustc",
        cargo=tmp_path / "cargo" if cargo_mode else None,
        cwd=tmp_path,
        env={"PATH": ""},
        target="wasm32-wasip1",
        command_argv=["cargo", "build"] if cargo_mode else ["rustc"],
    )
    assert phases == [["sysroot"], ["link-args"]] * (2 if cargo_mode else 1)
    assert all(
        unit["metadata_probe_count"] == 1 and unit["selection_probe_count"] == 1
        for unit in telemetry["units"]
    )


@pytest.mark.parametrize(
    "phase",
    ["compiler-metadata", "selected-sysroot", "link-selection", "driver-helpers"],
)
def test_rust_capture_failure_retains_complete_phase_transcript_without_environment(
    tmp_path, monkeypatch, phase
):
    linker = tmp_path / ("clang.exe" if os.name == "nt" else "clang")
    linker.write_bytes(b"driver")
    linker.chmod(0o755)
    raw_stdout = "raw-start\n" + "x" * 32_768 + "\nraw-end\n"
    raw_stderr = "stderr-start\n" + "y" * 4096 + "\nstderr-end\n"
    failed_argv = []

    def run(command, **kwargs):
        current = (
            "driver-helpers"
            if "-###" in command
            else "link-selection"
            if "link-args" in command
            else "selected-sysroot"
            if "--crate-name" in command
            else "compiler-metadata"
        )
        if current == phase:
            failed_argv.extend(command)
            return subprocess.CompletedProcess(
                command, 0 if phase == "link-selection" else 1, raw_stdout, raw_stderr
            )
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(linker)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    with pytest.raises(toolchain_capture.RustLinkCaptureError) as caught:
        toolchain_capture.capture_rust_link_process_images(
            rustc=tmp_path / "rustc",
            cargo=None,
            cwd=tmp_path,
            env={"PATH": "", "PRIVATE_ENV": "do-not-record-this-environment-value"},
            target=None,
        )
    diagnostic = caught.value.diagnostic
    assert diagnostic["phase"] == phase
    assert diagnostic["unit"] == (
        "compiler" if phase == "compiler-metadata" else "target"
    )
    failed = diagnostic["probes"][-1]
    assert failed["argv"] == failed_argv
    assert failed["cwd"] == str(tmp_path) and failed["compiler_cwd"] == str(tmp_path)
    assert failed["stdout"] == raw_stdout and failed["stderr"] == raw_stderr
    assert "do-not-record-this-environment-value" not in json.dumps(diagnostic)
    assert caught.value.returncode == failed["returncode"]
    assert caught.value.stderr == raw_stderr
    assert "raw-start" not in str(caught.value)
    if failed["returncode"] != 0:
        assert f"exit status {failed['returncode']}" in str(caught.value)
        assert raw_stderr in str(caught.value)
    else:
        assert len(str(caught.value)) < 300


@pytest.mark.parametrize("feature", ["link-args", "sysroot"])
def test_rust_cargo_legal_feature_names_are_not_print_argument_positions(
    tmp_path, monkeypatch, feature
):
    linker = tmp_path / ("linker.exe" if os.name == "nt" else "linker")
    linker.write_bytes(b"linker")
    linker.chmod(0o755)
    probes = []

    def run(command, **kwargs):
        if "--manifest-path" in command:
            probes.append(list(command))
            assert command[command.index("--features") + 1] == feature
            manifest = Path(command[command.index("--manifest-path") + 1])
            assert json.dumps(feature) + "=[]" in manifest.read_text(encoding="utf-8")
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(linker)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    toolchain_capture.capture_rust_link_process_images(
        rustc=tmp_path / "rustc",
        cargo=tmp_path / "cargo",
        cwd=tmp_path,
        env={"PATH": ""},
        target="wasm32-wasip1",
        command_argv=["cargo", "build", "--features", feature],
    )
    assert len(probes) == 4
    for metadata, selection in zip(probes[::2], probes[1::2], strict=True):
        assert metadata[:-2] == selection[:-4]
        assert metadata[-2:] == ["--print", "sysroot"]
        assert selection[-4:] == ["-C", "save-temps", "--print", "link-args"]


@pytest.mark.parametrize("cargo_mode", [False, True])
@pytest.mark.parametrize("relative_sysroot", [False, True])
@pytest.mark.parametrize("relative_linker", [False, True])
def test_rust_link_capture_resolves_sysroot_override_and_host_consumer(
    tmp_path, monkeypatch, cargo_mode, relative_sysroot, relative_linker
):
    host = "x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu"
    suffix = ".exe" if os.name == "nt" else ""
    compiler, override = tmp_path / "compiler", tmp_path / "override"
    override_arg = "override" if relative_sysroot else str(override)
    linker = override / "lib" / "rustlib" / host / "bin" / ("rust-lld" + suffix)
    linker.parent.mkdir(parents=True)
    linker.write_bytes(b"target linker")
    linker.chmod(0o755)
    compiler.mkdir()
    native = compiler / ("native-linker" + suffix)
    native.write_bytes(b"host linker")
    native.chmod(0o755)
    invocation_cwd = tmp_path / "invocation" if cargo_mode else tmp_path
    invocation_cwd.mkdir(exist_ok=True)
    config = invocation_cwd / "link-config.toml"
    config.write_text("[build]\nincremental=false\n", encoding="utf-8")
    calls = []

    def fake_run(command, **kwargs):
        if command[1] == "metadata":
            assert kwargs["cwd"] == invocation_cwd
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "workspace_root": str(tmp_path),
                        "workspace_default_members": ["fixture"],
                        "packages": [
                            {
                                "id": "fixture",
                                "source": None,
                                "manifest_path": str(tmp_path / "Cargo.toml"),
                                "targets": [
                                    {
                                        "name": "fixture",
                                        "kind": ["bin"],
                                        "src_path": str(tmp_path / "src/main.rs"),
                                    }
                                ],
                            }
                        ],
                    }
                ),
                "",
            )
        metadata = _rust_metadata_probe(command, compiler)
        if metadata is not None:
            return metadata
        calls.append(list(command))
        assert kwargs["env"].get("PATH", "") == "", "custody must not patch PATH"
        host_unit = "--lib" in command
        if host_unit:
            assert "--sysroot" not in command
            selected = str(native)
        else:
            expected_override = str(override) if cargo_mode else override_arg
            assert command[command.index("--sysroot") + 1] == expected_override
            selected = (
                str(linker)
                if cargo_mode and relative_linker
                else str(linker.relative_to(tmp_path))
                if relative_linker
                else "rust-lld"
            )
            if relative_linker:
                assert "linker=" + selected in command
        return subprocess.CompletedProcess(command, 0, json.dumps(selected) + "\n", "")

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=fake_run))
    argv = (
        [
            "cargo",
            "rustc",
            "--config=build.incremental=false",
            "--config",
            "link-config.toml",
            "--",
            "--sysroot",
            override_arg,
        ]
        if cargo_mode
        else ["rustc", "--sysroot", override_arg]
    )
    if relative_linker:
        argv.extend(("-C", "linker=" + str(linker.relative_to(tmp_path))))
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=compiler / "rustc",
        cargo=compiler / "cargo" if cargo_mode else None,
        cwd=invocation_cwd,
        env={"PATH": ""},
        target="wasm32-wasip1",
        command_argv=argv,
    )
    assert {row["path"] for row in images} == {
        str(linker.resolve()),
        *([str(native.resolve())] if cargo_mode else []),
    }
    assert len(calls) == (2 if cargo_mode else 1)
    target_unit = telemetry["units"][0]
    assert target_unit["process_resolution"][0]["origin"] == (
        "explicit-path" if relative_linker else "rust-sysroot-host-tool"
    )
    assert target_unit["process_resolution"][0]["selected_sysroot"] == str(override)
    if cargo_mode:
        assert all(
            command[command.index("--config") + 1] == "build.incremental=false"
            for command in calls
        )
        assert telemetry["units"][1]["unit"] == "host-proc-macro"
        assert all(str(config) in command for command in calls)
        assert target_unit["configuration_files"][0]["path"] == str(config)
        if relative_sysroot or relative_linker:
            assert target_unit["forwarded_compiler_context"]["compiler_cwd"] == str(
                tmp_path
            )
        if relative_sysroot:
            assert target_unit["path_projections"][0]["original"] == "override"
    selected, reused = toolchain_capture.revalidate_rust_link_process_images(
        {"process_images": images, "link_selection": telemetry},
        target="wasm32-wasip1",
        command_argv=argv,
    )
    assert selected == images and reused == telemetry
    assert len(calls) == (2 if cargo_mode else 1), (
        "armed custody must not reselect tools"
    )
    if cargo_mode:
        config.write_text("[build]\nincremental=true\n", encoding="utf-8")
        with pytest.raises(ValueError, match="--config file changed"):
            toolchain_capture.revalidate_rust_link_process_images(
                {"process_images": images, "link_selection": telemetry},
                target="wasm32-wasip1",
                command_argv=argv,
            )


def test_rust_cargo_configuration_relative_sysroot_reports_executor_boundary(
    tmp_path, monkeypatch
):
    def run(command, **kwargs):
        if "--manifest-path" in command:
            assert "link-args" not in command, "reject before synthetic linking"
            return subprocess.CompletedProcess(
                command, 0, "../per-package-sysroot\n", ""
            )
        return _rust_metadata_probe(command, tmp_path)

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    with pytest.raises(
        ValueError,
        match="configuration-selected relative sysroot.*compiler-execution context hook",
    ):
        toolchain_capture.capture_rust_link_process_images(
            rustc=tmp_path / "rustc",
            cargo=tmp_path / "cargo",
            cwd=tmp_path,
            env={"PATH": ""},
            target="wasm32-wasip1",
            command_argv=["cargo", "build"],
        )


@pytest.mark.parametrize("external", [False, True])
def test_rust_cargo_compiler_cwd_follows_selected_source_not_invocation(
    tmp_path, monkeypatch, external
):
    workspace, package, invocation = (
        tmp_path / "workspace",
        tmp_path / "package",
        tmp_path / "invocation",
    )
    for directory in (workspace, package, invocation):
        directory.mkdir()
    source = (package if external else workspace) / "src/main.rs"
    metadata = {
        "workspace_root": str(workspace),
        "workspace_default_members": ["different"],
        "packages": [
            {
                "id": "selected-package",
                "source": None,
                "manifest_path": str(package / "Cargo.toml"),
                "targets": [
                    {"name": "selected", "kind": ["bin"], "src_path": str(source)},
                    {
                        "name": "irrelevant",
                        "kind": ["example"],
                        "src_path": str(tmp_path / "external.rs"),
                    },
                ],
            },
        ],
    }
    queries = []

    def run(command, **kwargs):
        queries.append(command)
        assert kwargs["cwd"] == invocation
        assert "--offline" in command and "--locked" in command
        output = "selected-package\n" if command[1] == "pkgid" else json.dumps(metadata)
        return subprocess.CompletedProcess(command, 0, output, "")

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    context = toolchain_capture._cargo_forwarded_compiler_context(
        tmp_path / "cargo",
        [
            "cargo",
            "rustc",
            "--manifest-path",
            "../workspace/Cargo.toml",
            "--package",
            "selected@1",
            "--bin",
            "selected",
            "--",
            "--sysroot=relative",
        ],
        cwd=invocation,
        env={"PATH": ""},
    )
    assert context["compiler_cwd"] == str(package if external else workspace)
    assert len(context["sources"]) == 1
    assert queries[1][queries[1].index("--package") + 1] == "selected@1"


def test_rust_cargo_configuration_relative_linker_reports_executor_boundary(
    tmp_path, monkeypatch
):
    def run(command, **kwargs):
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command,
            1,
            '"tools/linker"\n',
            "linker tools/linker not found",
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    with pytest.raises(
        ValueError,
        match="configuration-selected relative linker.*compiler-execution context hook",
    ):
        toolchain_capture.capture_rust_link_process_images(
            rustc=tmp_path / "rustc",
            cargo=tmp_path / "cargo",
            cwd=tmp_path,
            env={"PATH": ""},
            target=None,
            command_argv=["cargo", "build"],
        )


def test_rust_driver_alias_preserves_invocation_and_revalidates_selection(
    tmp_path, monkeypatch
):
    suffix = ".exe" if os.name == "nt" else ""
    driver = tmp_path / ("llvm-driver" + suffix)
    alias = tmp_path / ("clang++" + suffix)
    helper = tmp_path / ("ld" + suffix)
    for path in (driver, helper):
        path.write_bytes(path.name.encode())
        path.chmod(0o755)
    try:
        alias.symlink_to(driver)
    except OSError as exc:
        pytest.skip(f"executable symlinks unavailable: {exc}")
    dry_runs = []

    def run(command, **kwargs):
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        if "-###" in command:
            assert command[0] == str(alias), "driver role must not become llvm-driver"
            assert kwargs["cwd"] == tmp_path
            dry_runs.append(command)
            # Drivers print each helper command indented one space, quoted.
            quoted = str(helper).replace("\\", "\\\\")
            return subprocess.CompletedProcess(command, 0, "", f' "{quoted}"\n')
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(alias)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=tmp_path / "rustc",
        cargo=None,
        cwd=tmp_path,
        env={"PATH": ""},
        target=None,
    )
    assert len(dry_runs) == 1
    assert {row["path"] for row in images} == {str(alias), str(driver), str(helper)}
    assert (
        next(row for row in images if row["path"] == str(alias))["path_kind"]
        == "selection"
    )
    alias.unlink()
    alias.symlink_to(helper)
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            {"process_images": images, "link_selection": telemetry},
            target=None,
        )


def _gcc_link_fixture(tmp_path: Path, collect2_report) -> dict[str, object]:
    """A GCC driver whose `-###` stops at collect2, as gcc prints it.

    `collect2_report(command)` answers the `-Wl,-debug` relink. Any other
    linker-discovery query (for example `-print-prog-name=ld`) fails the test.
    """
    suffix = ".exe" if os.name == "nt" else ""
    driver = tmp_path / "bin" / ("cc" + suffix)
    collect2 = tmp_path / "libexec" / ("collect2" + suffix)
    # The driver's own program directory has an `ld` that collect2 does not run.
    decoy = tmp_path / "bin" / ("ld" + suffix)
    for path in (driver, collect2, decoy):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(path.name.encode())
        path.chmod(0o755)
    link_args = ["-o", str(tmp_path / "probe"), str(tmp_path / "probe.o")]
    calls: list[list[str]] = []

    def run(command, **kwargs):
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        command = [str(value) for value in command]
        calls.append(command)
        assert not any(value.startswith("-print-prog-name") for value in command)
        if "-###" in command:
            assert command == [str(driver), "-###", *link_args]
            quoted = str(collect2).replace("\\", "\\\\")
            return subprocess.CompletedProcess(
                command,
                0,
                "",
                "Using built-in specs.\nCOLLECT_GCC=cc\n"
                f' "{quoted}" -plugin --build-id -o probe probe.o\n',
            )
        if "-Wl,-debug" in command:
            assert kwargs["cwd"] == tmp_path
            return collect2_report(command)
        return subprocess.CompletedProcess(
            command,
            0,
            " ".join(json.dumps(value) for value in (str(driver), *link_args)) + "\n",
            "",
        )

    return {
        "driver": driver,
        "collect2": collect2,
        "decoy": decoy,
        "link_args": link_args,
        "calls": calls,
        "run": run,
    }


def _collect2_debug(linker_line: str) -> str:
    # Shape of gcc 15 collect2 -debug stderr.
    return (
        "Looking for 'real-ld'\nLooking for 'collect-ld'\nLooking for 'ld'\n"
        "collect2 version 15.2.0\n"
        f"{linker_line}\n"
        "c_file_name         = /usr/bin/gcc\nnm_file_name        = /usr/bin/nm\n"
    )


def test_gcc_link_capture_seals_the_linker_collect2_reports(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    suffix = ".exe" if os.name == "nt" else ""
    # collect2 searched PATH and chose this file, not the driver-directory `ld`.
    reported = tmp_path / "path dir" / ("ld" + suffix)
    reported.parent.mkdir()
    reported.write_bytes(b"path-selected linker")
    reported.chmod(0o755)
    fixture = _gcc_link_fixture(
        tmp_path,
        lambda command: subprocess.CompletedProcess(
            command, 0, "", _collect2_debug(f"ld_file_name        = {reported}")
        ),
    )
    monkeypatch.setattr(
        toolchain_capture, "_COMMANDS", SimpleNamespace(run=fixture["run"])
    )

    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=tmp_path / "rustc",
        cargo=None,
        cwd=tmp_path,
        env={"PATH": ""},
        target=None,
    )

    sealed = {Path(str(row["path"])).resolve() for row in images}
    assert sealed == {
        Path(str(fixture["driver"])).resolve(),
        Path(str(fixture["collect2"])).resolve(),
        reported.resolve(),
    }
    assert Path(str(fixture["decoy"])).resolve() not in sealed
    relinks = [call for call in fixture["calls"] if "-Wl,-debug" in call]
    assert relinks == [[str(fixture["driver"]), *fixture["link_args"], "-Wl,-debug"]], (
        "collect2 is asked once, through the exact selected driver argv"
    )
    [unit] = telemetry["units"]
    assert unit["collect2_probe_count"] == 1
    assert unit["process_resolution"][-1]["origin"] == "collect2-report"
    assert unit["process_resolution"][-1]["requested"] == str(reported)


@pytest.mark.parametrize(
    ("linker_lines", "returncode", "message"),
    [
        (["ld_file_name        = not found"], 1, "found no linker"),
        ([], 0, "reported 0 linker selections"),
        (
            ["ld_file_name        = /a/ld", "ld_file_name        = /b/ld"],
            0,
            "reported 2 linker selections",
        ),
        (["ld_file_name        = /absent/ld"], 0, "selected linker is unavailable"),
    ],
)
def test_gcc_collect2_linker_report_fails_closed_with_child_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    linker_lines: list[str],
    returncode: int,
    message: str,
) -> None:
    stderr = _collect2_debug("\n".join(linker_lines))
    fixture = _gcc_link_fixture(
        tmp_path,
        lambda command: subprocess.CompletedProcess(command, returncode, "", stderr),
    )
    monkeypatch.setattr(
        toolchain_capture, "_COMMANDS", SimpleNamespace(run=fixture["run"])
    )
    with pytest.raises(toolchain_capture.RustLinkCaptureError, match=message) as caught:
        toolchain_capture.capture_rust_link_process_images(
            rustc=tmp_path / "rustc",
            cargo=None,
            cwd=tmp_path,
            env={"PATH": ""},
            target=None,
        )
    assert caught.value.diagnostic["phase"] == "collect2-linker"
    assert caught.value.stderr == stderr
    assert caught.value.returncode == returncode


def test_gcc_link_capture_follows_the_rust_lld_wrapper_collect2_selects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # rustc 1.90+ on x86_64 Linux: cc -fuse-ld=lld -B<sysroot bin>/gcc-ld.
    # `_rust_metadata_probe` reports tmp_path as the sysroot.
    windows = os.name == "nt"
    host = "x86_64-pc-windows-msvc" if windows else "x86_64-unknown-linux-gnu"
    suffix = ".exe" if windows else ""
    bin_dir = tmp_path / "lib" / "rustlib" / host / "bin"
    wrapper = bin_dir / "gcc-ld" / ("ld.lld" + suffix)
    rust_lld = bin_dir / ("rust-lld" + suffix)
    for path in (wrapper, rust_lld):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(path.name.encode())
        path.chmod(0o755)
    fixture = _gcc_link_fixture(
        tmp_path,
        lambda command: subprocess.CompletedProcess(
            command, 0, "", _collect2_debug(f"ld_file_name        = {wrapper}")
        ),
    )
    monkeypatch.setattr(
        toolchain_capture, "_COMMANDS", SimpleNamespace(run=fixture["run"])
    )

    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=tmp_path / "rustc",
        cargo=None,
        cwd=tmp_path,
        env={"PATH": ""},
        target=None,
    )

    roles = {Path(str(row["path"])).resolve(): row["role"] for row in images}
    assert roles[wrapper.resolve()] == "rust-link-helper"
    assert roles[rust_lld.resolve()] == "rust-link-helper"
    [unit] = telemetry["units"]
    assert [row["origin"] for row in unit["process_resolution"][-2:]] == [
        "collect2-report",
        "rust-lld-wrapper",
    ]
    revalidated, _ = toolchain_capture.revalidate_rust_link_process_images(
        {"process_images": images, "link_selection": telemetry}, target=None
    )
    assert revalidated == images
    rust_lld.write_bytes(b"substituted rust-lld")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            {"process_images": images, "link_selection": telemetry}, target=None
        )


def test_process_image_capture_revalidates_exact_identity(tmp_path: Path) -> None:
    executable = tmp_path / "captured-tool.exe"
    executable.write_bytes(b"canonical-process-image")

    image = process_image_capture.capture_image("fixture-tool", executable)

    assert image == {
        "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
        "role": "fixture-tool",
        "path": str(executable.resolve()),
        "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        "size_bytes": executable.stat().st_size,
    }
    assert process_image_capture.revalidate_images([image]) == [image]

    terminated = process_image_capture.capture_image(
        "fixture-helper", executable, root_exit_disposition="terminate"
    )
    assert terminated["root_exit_disposition"] == "terminate"
    assert process_image_capture.revalidate_images([terminated]) == [terminated]


def test_process_image_revalidation_rejects_mutation(tmp_path: Path) -> None:
    executable = tmp_path / "mutable-tool.exe"
    executable.write_bytes(b"before")
    image = process_image_capture.capture_image("fixture-tool", executable)

    executable.write_bytes(b"after-with-a-different-size")

    with pytest.raises(ValueError, match="changed while live custody armed"):
        process_image_capture.revalidate_images([image])


def test_platform_auxiliary_images_are_absent_off_windows_or_for_leaf_custody(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(process_image_capture.sys, "platform", "linux")
    assert process_image_capture.platform_auxiliary_images("declared-toolchains") == []

    monkeypatch.setattr(process_image_capture.sys, "platform", "win32")
    assert process_image_capture.platform_auxiliary_images("forbidden") == []


@pytest.mark.skipif(sys.platform != "win32", reason="Windows console broker custody")
def test_platform_auxiliary_images_capture_exact_windows_console_broker() -> None:
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    get_system_directory = kernel32.GetSystemDirectoryW
    get_system_directory.argtypes = [wintypes.LPWSTR, wintypes.UINT]
    get_system_directory.restype = wintypes.UINT
    buffer = ctypes.create_unicode_buffer(32_768)
    length = int(get_system_directory(buffer, len(buffer)))
    assert 0 < length < len(buffer)
    conhost = (Path(buffer.value) / "conhost.exe").resolve(strict=True)

    images = process_image_capture.platform_auxiliary_images("declared-toolchains")

    assert images == [
        {
            "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
            "role": "windows-console-broker",
            "path": str(conhost),
            "sha256": hashlib.sha256(conhost.read_bytes()).hexdigest(),
            "size_bytes": conhost.stat().st_size,
            "root_exit_disposition": "terminate",
        }
    ]
    assert process_image_capture.revalidate_images(images) == images


def test_rust_link_capture_declares_exact_platform_linker_helper_family(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    rustc = tmp_path / "rustc"
    cargo = tmp_path / "cargo"
    linker = tmp_path / "link.exe"
    helper = tmp_path / "vctip.exe"
    for path in (rustc, cargo, linker, helper):
        path.write_bytes(path.name.encode())
        path.chmod(0o755)

    def fake_run(command, **_kwargs):
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(linker)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=fake_run))
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=rustc,
        cargo=cargo,
        cwd=tmp_path,
        env={"PATH": str(tmp_path)},
        target=None,
        command_argv=("cargo", "build"),
        linker_process_helpers={"link.exe": ["vctip.exe"]},
    )

    assert [row["role"] for row in images] == ["rust-linker", "rust-link-helper"]
    assert [Path(str(row["path"])).name for row in images] == [
        "link.exe",
        "vctip.exe",
    ]
    assert all(unit["declared_helper_count"] == 1 for unit in telemetry["units"])
    assert all(unit["selected_helper_count"] == 1 for unit in telemetry["units"])
    revalidated, reused = toolchain_capture.revalidate_rust_link_process_images(
        {"process_images": images, "link_selection": telemetry},
        target=None,
        command_argv=("cargo", "build"),
    )
    assert revalidated == images
    assert reused == telemetry


def test_rust_link_capture_declares_exact_msvc_build_tool_family(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    rustc = tmp_path / "rustc.exe"
    cargo = tmp_path / "cargo.exe"
    linker = tmp_path / "link.exe"
    compiler = tmp_path / "cl.exe"
    archiver = tmp_path / "lib.exe"
    for path in (rustc, cargo, linker, compiler, archiver):
        path.write_bytes(path.name.encode())
        path.chmod(0o755)

    def fake_run(command, **_kwargs):
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(linker)) + "\n", ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=fake_run))
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=rustc,
        cargo=cargo,
        cwd=tmp_path,
        env={"PATH": str(tmp_path)},
        target=None,
        command_argv=("cargo", "build"),
        linker_build_tools={
            "link.exe": {
                "cl.exe": "rust-build-c-compiler",
                "lib.exe": "rust-build-archiver",
            }
        },
    )

    assert [(row["role"], Path(str(row["path"])).name) for row in images] == [
        ("rust-build-c-compiler", "cl.exe"),
        ("rust-build-archiver", "lib.exe"),
        ("rust-linker", "link.exe"),
    ]
    assert all(
        row.get("root_exit_disposition", "require-exit") == "require-exit"
        for row in images
    )
    assert all(unit["declared_build_tool_count"] == 2 for unit in telemetry["units"])
    assert all(unit["selected_build_tool_count"] == 2 for unit in telemetry["units"])

    revalidated, reused = toolchain_capture.revalidate_rust_link_process_images(
        {"process_images": images, "link_selection": telemetry},
        target=None,
        command_argv=("cargo", "build"),
    )
    assert revalidated == images
    assert reused == telemetry
