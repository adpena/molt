from __future__ import annotations

from molt.llvm_toolchain import capture_wasi_sdk_selection

from concurrent.futures import ThreadPoolExecutor
import gzip
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
from types import SimpleNamespace

from tests.executable_test_support import custody_spelling
import pytest

from tests.process_guard_common import run_isolated_python_probe

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
    measured = run_isolated_python_probe(
        """
        import gc
        import json
        from pathlib import Path
        import sys
        import tracemalloc
        from tools.proof_queue_pkg import toolchain_capture
        # Load publish_capture's lazy policy module before the measured work.
        import tools.proof_plan

        identity = json.load(sys.stdin)
        cas = Path(sys.argv[1])
        legacy = {
            "toolchains": identity,
            "toolchain_custody": {"prelaunch": identity, "postcompletion": identity},
        }
        if tracemalloc.is_tracing():
            raise RuntimeError("probe requires exclusive allocation tracing")
        gc.collect()
        tracemalloc.start()
        try:
            legacy_bytes = json.dumps(legacy, sort_keys=True).encode()
            _, legacy_peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        legacy_size = len(legacy_bytes)
        del legacy_bytes, legacy
        gc.collect()
        tracemalloc.start()
        try:
            summaries, reference, telemetry = toolchain_capture.publish_capture(cas, identity)
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
            _, compact_peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        print(json.dumps({
            "legacy_size": legacy_size,
            "legacy_peak": legacy_peak,
            "compact_size": len(compact_bytes),
            "compact_peak": compact_peak,
            "compact": compact,
        }))
        """,
        args=[tmp_path / "cas"],
        payload=identity,
    )
    compact = measured["compact"]
    reference = compact["toolchain_capture"]["artifact"]
    assert len(json.dumps(compact, sort_keys=True).encode()) == measured["compact_size"]
    assert compact["toolchain_custody"] == {
        "prelaunch": compact["toolchains"],
        "postcompletion": compact["toolchains"],
        "identical": True,
    }
    assert measured["compact_size"] < measured["legacy_size"] // 100
    assert measured["compact_size"] < 64 * 1024
    assert reference["compressed_bytes"] < reference["uncompressed_bytes"] // 10
    # Publishing streams its digest check, so the compact path peaks near one
    # encoding of the capture: 0.35-0.38 of the legacy peak measured on
    # 2026-10-09, against 0.89 when publication re-parsed the blob (HF-51).
    assert measured["compact_peak"] < measured["legacy_peak"] // 2
    captured = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
    assert captured["toolchains"] == identity
    assert (
        toolchain_capture.compact_toolchains(captured["toolchains"])
        == compact["toolchains"]
    )


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
    rust_images = [process_image_capture.capture_image("rust-linker", owned)]
    identity["rustc"]["process_images"] = rust_images
    identity["rustc"]["link_selection"] = {
        "schema": "molt.proof-rust-link-selection-telemetry.v5",
        "target": None,
        "compiler_host": "x86_64-unknown-linux-gnu",
        "selection_probe_count": 1,
        "selected_process_count": 1,
        "units": [
            {
                "schema": "molt.proof-rust-link-unit.v2",
                "unit": "target",
                "selection_probe_count": 1,
                "selected_process_count": 1,
                "command_semantics_sha256": canonical_json_sha256(["rustc"]),
                "artifact_context": None,
                "process_image_refs": [{"role": "rust-linker", "path": str(owned)}],
                "process_resolution": [
                    {"path": str(owned), "content_path": str(owned)}
                ],
                "artifact_selection": {
                    "cargo_crate_types": None,
                    "manifest_crate_types": None,
                    "rustc_crate_types": [],
                    "link_required": True,
                },
            }
        ],
        "admitted_command": ["rustc"],
        "producer_command": ["rustc"],
        "native_required": {},
        "native_build": [],
        "command_semantics_sha256": canonical_json_sha256(["rustc"]),
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
    identity["rustc"]["link_selection"] = {
        **identity["rustc"]["link_selection"],
        "selection_probe_count": 2,
    }
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
    identities = []
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
        identities.append(identity)
    measurements = run_isolated_python_probe(
        """
        import gc
        import json
        import sys
        import tracemalloc
        from tools.proof_queue_pkg import toolchain_capture

        identities = json.load(sys.stdin)
        measurements = []
        if tracemalloc.is_tracing():
            raise RuntimeError("probe requires exclusive allocation tracing")
        for identity in identities:
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
                _, peak = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            measurements.append({"summaries": summaries, "size": len(encoded), "peak": peak})
        print(json.dumps(measurements))
        """,
        payload=identities,
    )
    peaks = [row["peak"] for row in measurements]
    sizes = [row["size"] for row in measurements]
    for package_count, identity, measured in zip(
        (0, 2_000), identities, measurements, strict=True
    ):
        summaries = measured["summaries"]
        environment = identity["python"]["environment"]
        distributions = environment["distributions"]
        bindings = identity["python"]["node_custody"]
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
        # The retained physical compiler is also the Cargo probe's RUSTC.
        # Its real file boundary must exist even when execution is substituted.
        compiler = Path(command[0])
        assert compiler.parent == root
        if not compiler.exists():
            compiler.write_bytes(b"fixture physical Rust compiler")
            compiler.chmod(0o755)
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
@pytest.mark.parametrize("metadata_mutation", [False, True])
def test_rust_link_capture_uses_exact_target_environment_and_selected_image(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    command_prefix: str,
    metadata_mutation: bool,
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
    original_manifest = tmp_path / "Cargo.toml"
    original_manifest.write_text(
        '[package]\nname="fixture"\nversion="0.1.0"\n[lib]\ncrate-type=["cdylib"]\n',
        encoding="utf-8",
    )
    (tmp_path / "src").mkdir()
    (tmp_path / "src/lib.rs").write_text("pub fn fixture() {}\n", encoding="utf-8")
    (tmp_path / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
    metadata_calls = []
    package_calls = []

    def fake_run(command, **kwargs):
        if command[1] == "pkgid":
            assert command[command.index("--package") + 1] == "fixture"
            package_calls.append(list(command))
            return subprocess.CompletedProcess(command, 0, "fixture\n", "")
        if command[1] == "metadata":
            metadata_calls.append(list(command))
            if metadata_mutation and len(metadata_calls) == 2:
                original_manifest.write_text(
                    original_manifest.read_text(encoding="utf-8").replace(
                        '"cdylib"', '"rlib"'
                    ),
                    encoding="utf-8",
                )
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
                                "manifest_path": str(original_manifest),
                                "targets": [
                                    {
                                        "name": "fixture",
                                        "kind": ["lib"],
                                        "crate_types": ["cdylib"],
                                        "src_path": str(tmp_path / "src/lib.rs"),
                                    },
                                    {
                                        "name": "other",
                                        "kind": ["bin"],
                                        "crate_types": ["bin"],
                                        "src_path": str(tmp_path / "src/main.rs"),
                                    },
                                ],
                            }
                        ],
                    }
                ),
                "",
            )
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        argv = [str(value) for value in command]
        environment = dict(kwargs["env"])
        observed.append((argv, environment))
        if "--manifest-path" in argv:
            probe_root = Path(argv[argv.index("--manifest-path") + 1]).parent
            if (probe_root / "host.rs").exists():
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
        "--package",
        "fixture",
        "--lib",
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
        "staticlib",
    ]
    if metadata_mutation:
        with pytest.raises(
            toolchain_capture.RustLinkCaptureError, match="Cargo artifact input changed"
        ):
            toolchain_capture.capture_rust_link_process_images(
                rustc=rustc,
                cargo=cargo,
                cwd=tmp_path,
                env=environment,
                target="test-triple",
                command_argv=command_argv,
            )
        assert len(metadata_calls) == 2
        assert not observed, "mutation must reject before synthetic compilation"
        return
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
    assert observed[0][0][observed[0][0].index("--crate-type") + 1] == "staticlib"
    assert '[profile.frontier-fast]\ninherits="release"' in observed_manifests[0]
    assert (
        "[lib]" in observed_manifests[0]
        and 'crate-type=["cdylib"]' in observed_manifests[0]
    )
    assert len(metadata_calls) == len(package_calls) == 2
    assert "[lib]" in observed_manifests[1] and "[[bin]]" not in observed_manifests[1]
    assert observed[0][1]["CARGO_TARGET_TEST_TRIPLE_LINKER"] == str(linker)
    assert all(environment["RUSTC"] == str(rustc) for _, environment in observed)
    assert "RUSTC" not in environment
    assert images == [
        {
            "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
            "role": "rust-linker",
            "path": process_image_capture._image_path_key(linker.resolve()),
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

    assert str(original_manifest) in {
        row.path for row in toolchain_capture.frozen_files(selected_identity)
    }
    _, reference, _ = toolchain_capture.publish_capture(
        tmp_path / "cas", {"rustc": selected_identity}
    )
    loaded = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
    for mutation in ("context", "manifest", "base", "selected-target"):
        altered = __import__("copy").deepcopy(loaded)
        unit = altered["toolchains"]["rustc"]["link_selection"]["units"][0]
        if mutation == "context":
            unit["artifact_context"] = None
        elif mutation == "manifest":
            del unit["artifact_context"]["sources"][0]["manifest"]
        elif mutation == "base":
            # Keep the exact Cargo transcript and manifest bytes. Erasing the
            # manifest's cdylib obligation must not make this archive-only.
            unit["artifact_context"]["sources"][0]["crate_types"] = ["rlib"]
            unit["artifact_selection"].update(
                manifest_crate_types=["rlib"], link_required=False
            )
            unit.update(
                selected_process_count=0,
                process_resolution=[],
                process_image_refs=[],
                link_argv_sha256=canonical_json_sha256([]),
            )
        else:
            # Another target exists in the same Cargo metadata. It is not the
            # actual --lib selection even though the package/manifest match.
            source = unit["artifact_context"]["sources"][0]
            source.update(crate_types=["bin"], source=str(tmp_path / "src/main.rs"))
            unit["artifact_selection"].update(manifest_crate_types=["bin"])
        diagnostic = (
            "manifest" if mutation in {"context", "manifest"} else "selected target"
        )
        with pytest.raises(ValueError, match=diagnostic):
            toolchain_capture.publish_capture(tmp_path / "cas", altered["toolchains"])
        altered["files"] = [
            row.as_dict()
            for row in toolchain_capture.frozen_files(altered["toolchains"])
        ]
        reference = custody_cas.put_json(tmp_path / "cas", altered).as_dict()
        with pytest.raises(ValueError, match=diagnostic):
            toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")

    captured_manifest = telemetry["units"][0]["artifact_context"]["sources"][0][
        "manifest"
    ]
    assert (
        captured_manifest["sha256"]
        == hashlib.sha256(original_manifest.read_bytes()).hexdigest()
    )
    toolchain_capture.revalidate_rust_artifact_manifests(telemetry)
    original_manifest.write_text(
        original_manifest.read_text(encoding="utf-8").replace('"cdylib"', '"rlib"'),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="artifact manifest/source changed"):
        toolchain_capture.revalidate_rust_artifact_manifests(telemetry)

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
            command_argv=["rustc"],
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
        # The owning error carries the child's complete stderr exactly once;
        # a phase-local excerpt in the reason would repeat it.
        assert str(caught.value).count("stderr-start") == 1
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
@pytest.mark.parametrize(
    "relative_linker,codegen_spelling",
    [
        (False, "-C"),
        (True, "-C"),
        (True, "-Cjoined"),
        (True, "--codegen"),
        (True, "--codegen="),
        (True, "-gC"),
        (True, "-gCjoined"),
    ],
)
def test_rust_link_capture_resolves_sysroot_override_and_host_consumer(
    tmp_path,
    monkeypatch,
    cargo_mode,
    relative_sysroot,
    relative_linker,
    codegen_spelling,
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
            assert kwargs["env"]["RUSTC"] == str(compiler / "rustc")
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
                expected = "linker=" + selected
                assert (
                    (expected in command)
                    if codegen_spelling in {"-C", "--codegen", "-gC"}
                    else (
                        (codegen_spelling.removesuffix("joined")) + expected in command
                    )
                )
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
        value = "linker=" + str(linker.relative_to(tmp_path))
        argv.extend(
            (codegen_spelling, value)
            if codegen_spelling in {"-C", "--codegen", "-gC"}
            else ((codegen_spelling.removesuffix("joined")) + value,)
        )
    images, telemetry = toolchain_capture.capture_rust_link_process_images(
        rustc=compiler / "rustc",
        cargo=compiler / "cargo" if cargo_mode else None,
        cwd=invocation_cwd,
        env={"PATH": ""},
        target="wasm32-wasip1",
        command_argv=argv,
    )
    assert {row["path"] for row in images} == {
        toolchain_capture._image_path_key(linker.resolve()),
        *([toolchain_capture._image_path_key(native.resolve())] if cargo_mode else []),
    }
    assert len(calls) == (2 if cargo_mode else 1)
    assert telemetry["producer_command"] == argv
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
        command_argv=["rustc"],
        rustc=tmp_path / "rustc",
        cargo=None,
        cwd=tmp_path,
        env={"PATH": ""},
        target=None,
    )
    assert len(dry_runs) == 1
    assert {row["path"] for row in images} == {
        toolchain_capture._image_path_key(path) for path in (alias, driver, helper)
    }
    assert (
        next(
            row
            for row in images
            if row["path"] == toolchain_capture._image_path_key(alias)
        )["path_kind"]
        == "selection"
    )
    alias.unlink()
    alias.symlink_to(helper)
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            {"process_images": images, "link_selection": telemetry},
            command_argv=["rustc"],
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
        command_argv=["rustc"],
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
            command_argv=["rustc"],
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
        command_argv=["rustc"],
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
        {"process_images": images, "link_selection": telemetry},
        command_argv=["rustc"],
        target=None,
    )
    assert revalidated == images
    rust_lld.write_bytes(b"substituted rust-lld")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            {"process_images": images, "link_selection": telemetry},
            command_argv=["rustc"],
            target=None,
        )


def test_process_image_capture_revalidates_exact_identity(tmp_path: Path) -> None:
    executable = tmp_path / "captured-tool.exe"
    executable.write_bytes(b"canonical-process-image")

    image = process_image_capture.capture_image("fixture-tool", executable)

    assert image == {
        "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
        "role": "fixture-tool",
        "path": process_image_capture._image_path_key(executable.resolve()),
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
            "path": process_image_capture._image_path_key(conhost),
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
    # Literal files need no selection alias, including uppercase Windows roots.
    assert all("path_kind" not in row for row in images)
    assert all(unit["declared_build_tool_count"] == 2 for unit in telemetry["units"])
    assert all(unit["selected_build_tool_count"] == 2 for unit in telemetry["units"])

    revalidated, reused = toolchain_capture.revalidate_rust_link_process_images(
        {"process_images": images, "link_selection": telemetry},
        target=None,
        command_argv=("cargo", "build"),
    )
    assert revalidated == images
    assert reused == telemetry


@pytest.mark.parametrize("mutation", ["header", "new-header", "compiler-rt", "linker"])
def test_wasi_sdk_closure_binds_helpers_and_complete_resources(tmp_path, mutation):
    from tests.runtime_build_identity_helper import (
        RuntimeFixtureRoot,
        provisioned_wasi_sdk_fixture,
    )
    from molt.llvm_toolchain import wasi_c_abi_plan
    from tools import proof_plan
    from tools.proof_queue_pkg import execution_environment

    installation = provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(tmp_path))
    plan = wasi_c_abi_plan(installation)
    selection = capture_wasi_sdk_selection(
        root=proof_plan.ROOT, env={"WASI_SDK_PATH": str(installation.sdk)}
    )
    assert "resources" not in selection
    assert execution_environment._broad_toolchain_roots(
        {"wasi-clang": {"wasi_sdk": selection}}
    ) == [
        installation.sdk / "lib",
        installation.sysroot,
    ]
    images = toolchain_capture.capture_wasi_sdk_images(selection)
    identity = {
        "path": process_image_capture._image_path_key(plan.driver),
        "wasi_sdk": selection,
        "process_images": images,
    }
    toolchain_capture.validate_wasi_sdk_closure(
        identity, full_capture=False, selected_role="clang"
    )
    verified_images = toolchain_capture.revalidate_wasi_sdk_selection(identity)
    assert plan.linker in {Path(row.path) for row in verified_images}
    closure = toolchain_capture.capture_wasi_sdk_resources(selection)
    identity["wasi_sdk"] = closure
    toolchain_capture.validate_wasi_sdk_closure(
        identity, full_capture=True, selected_role="clang"
    )
    frozen = {Path(row.path) for row in toolchain_capture.frozen_files(identity)}
    assert {plan.path("compiler_rt"), plan.path("long_double"), plan.linker}.issubset(
        frozen
    )
    if mutation == "linker":
        plan.linker.write_bytes(plan.linker.read_bytes() + b"changed")
        with pytest.raises(ValueError, match="changed|identity|digest|size"):
            process_image_capture.revalidate_images(images)
    else:
        path = (
            plan.path("compiler_rt")
            if mutation == "compiler-rt"
            else plan.include / ("new.h" if mutation == "new-header" else "errno.h")
        )
        path.write_bytes(b"changed SDK resource")
        with pytest.raises(ValueError, match="provisioned generation"):
            toolchain_capture.capture_wasi_sdk_resources(selection)


def _native_build_capture_fixture(
    tmp_path, monkeypatch, *, required=True, resources=False, armed=True, operation="c"
):
    """Real capture boundary with independent C/C++ driver transcripts."""
    from tools import proof_plan

    tools = {}
    for name in (
        "rustc",
        "cargo",
        "linker",
        "selected-gcc",
        "selected-g++",
        "selected-ar",
        "cc1",
        "cc1plus",
        "as",
    ):
        directory = tmp_path / (
            "helpers-outside-bin" if name in {"cc1", "cc1plus", "as"} else "bin"
        )
        directory.mkdir(exist_ok=True)
        path = directory / (name + (".exe" if os.name == "nt" else ""))
        path.write_bytes(name.encode())
        path.chmod(0o755)
        tools[name] = path
    calls = []

    def run(command, **kwargs):
        calls.append(list(command))
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        if str(command[0]) in {str(tools["selected-gcc"]), str(tools["selected-g++"])}:
            assert "-###" in command
            assert Path(kwargs["cwd"]) != tmp_path
            assert all(
                Path(value).is_file()
                for value in command
                if value.endswith((".c", ".cc", ".S"))
            )
            language = command[command.index("-x") + 1]
            # Independent absolute frontend and PATH-resolved assembler.
            frontend = "cc1plus" if language == "c++" else "cc1"
            transcript = " " + json.dumps(str(tools[frontend])) + ' "-E"\n'
            if language in {"c", "c++"}:
                transcript += " " + json.dumps(tools["as"].name) + ' "-o" "unit.o"\n'
            return subprocess.CompletedProcess(command, 0, "", transcript)
        # rustc archive creation has no linker command to print. Inspect the
        # actual compiler argv rather than the production artifact projection;
        # Cargo's host proc-macro has no staticlib override and still links.
        if (
            "--crate-type" in command
            and command[command.index("--crate-type") + 1] == "staticlib"
        ):
            return subprocess.CompletedProcess(command, 0, "", "")
        return subprocess.CompletedProcess(
            command, 0, json.dumps(str(tools["linker"])) + ' "probe.o"\n', ""
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    command_id, requirements = {
        "c": ("wasm.build.host", {"target": ["c"]}),
        "c++": ("mlir.test.backend", {"host": ["c++"]}),
        "both": ("rust.test.runtime-extension-admission", {"target": ["c", "c++"]}),
    }[operation]
    if not required:
        requirements = {}
    command = (
        list(
            next(
                row.argv
                for row in proof_plan.ProofPlan.load().commands
                if row.id == command_id
            )
        )
        if required
        else ["cargo", "build"]
    )
    environment = {
        "PATH": str(tools["as"].parent),
        "CC": str(tools["selected-gcc"]),
        "CXX": str(tools["selected-g++"]),
        "AR": str(tools["selected-ar"]),
    }
    if resources:
        includes = tmp_path / "include"
        includes.mkdir()
        (includes / "header.h").write_text("#define CAPTURED 1\n", encoding="utf-8")
        forced = tmp_path / "forced.h"
        forced.write_text("#define FORCED 1\n", encoding="utf-8")
        environment["CFLAGS"] = f"-I{includes} -include {forced}"
        environment["CXXFLAGS"] = f"-I{includes} -include {forced}"
    images, selection = toolchain_capture.capture_rust_link_process_images(
        rustc=tools["rustc"],
        cargo=tools["cargo"],
        cwd=tmp_path,
        env=environment,
        target=None,
        rustc_version="rustc 1.99.0\nhost: x86_64-unknown-linux-gnu\n",
        command_argv=command,
        admitted_command=command,
        native_units=requirements,
    )
    if armed:
        _, selection = toolchain_capture.revalidate_rust_link_process_images(
            {"process_images": images, "link_selection": selection},
            target=None,
            command_argv=command,
            required_native_units=requirements,
        )
    compiler = process_image_capture.capture_image("rustc", tools["rustc"])
    identity = {
        "process_images": [compiler, *images],
        "link_selection": selection,
        "path": str(tools["rustc"]),
        "content_path": str(tools["rustc"]),
        "launcher_sha256": compiler["sha256"],
        "executable_sha256": compiler["sha256"],
        "version": "rustc 1.99.0",
        "probe_cwd": str(tmp_path),
        "policy_sha256": "a" * 64,
        "configuration_files": [],
    }
    identity["identity_sha256"] = canonical_json_sha256(identity)
    return identity, tools, environment, command, calls


def _equivalent_image_spelling(path: str, spelling: str) -> str:
    selected = Path(path)
    if spelling == "dot":
        result = str(selected.parent) + os.sep + "." + os.sep + selected.name
    elif spelling == "case":
        result = str(selected).swapcase()
    else:
        result = "\\\\?\\" + str(selected)
    assert result != path
    assert Path(result).samefile(selected), (
        "positive spelling must name the actual same file"
    )
    return result


@pytest.mark.parametrize(
    "spelling",
    [
        "dot",
        pytest.param(
            "case",
            marks=pytest.mark.skipif(os.name != "nt", reason="Windows path spelling"),
        ),
        pytest.param(
            "device",
            marks=pytest.mark.skipif(os.name != "nt", reason="Windows device path"),
        ),
    ],
)
@pytest.mark.parametrize(
    "coordinate",
    [
        "images",
        "unit-refs",
        "resolution",
        "native-compiler",
        "native-helper",
        "archiver",
    ],
)
def test_rust_image_membership_preserves_os_equivalent_spellings(
    tmp_path, monkeypatch, spelling, coordinate
):
    identity, _tools, env, _command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    prior_calls = list(calls)
    selection = identity["link_selection"]
    native = selection["native_build"][0]
    if coordinate == "images":
        for row in identity["process_images"]:
            row["path"] = _equivalent_image_spelling(row["path"], spelling)
    elif coordinate == "unit-refs":
        for unit in selection["units"]:
            for row in unit["process_image_refs"]:
                row["path"] = _equivalent_image_spelling(row["path"], spelling)
    elif coordinate == "resolution":
        for unit in selection["units"]:
            for row in unit["process_resolution"]:
                for field in ("path", "content_path"):
                    row[field] = _equivalent_image_spelling(row[field], spelling)
    elif coordinate == "native-compiler":
        command = native["selection"]["compilers"]["c"]
        changed = _equivalent_image_spelling(command[0], spelling)
        command[0] = changed
        native["compilers"]["c"]["command"][0] = changed
        for probe in native["compilers"]["c"]["probes"]:
            probe["argv"][0] = changed
    elif coordinate == "native-helper":
        for phase in native["compilers"]["c"]["phases"]:
            for helper in phase["helpers"]:
                helper["path"] = _equivalent_image_spelling(helper["path"], spelling)
    else:
        native["selection"]["archiver"] = _equivalent_image_spelling(
            native["selection"]["archiver"], spelling
        )
    assert (
        toolchain_capture.validate_rust_link_selection(identity, full_capture=True)
        == selection
    )
    assert toolchain_capture.native_compiler_selection_is_current(
        native["compilers"]["c"], env=env
    )
    assert calls == prior_calls, "structural verification must not run probes"


@pytest.mark.parametrize(
    "mutation",
    [
        "different-path",
        "relative",
        "missing-content",
        "role",
        "duplicate-ref",
        "digest",
        "disposition",
        "helper-digest",
        "helper-role",
        "archiver",
    ],
)
def test_rust_image_membership_rejects_non_equivalent_custody(
    tmp_path, monkeypatch, mutation
):
    identity, _tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    selection = identity["link_selection"]
    unit = selection["units"][0]
    native = selection["native_build"][0]
    if mutation in {"different-path", "archiver"}:
        original = Path(unit["process_resolution"][0]["path"])
        foreign = tmp_path / "same-bytes-different-coordinate"
        foreign.write_bytes(original.read_bytes())
        if mutation == "archiver":
            native["selection"]["archiver"] = str(foreign)
        else:
            unit["process_resolution"][0]["content_path"] = str(foreign)
    elif mutation == "relative":
        unit["process_resolution"][0]["path"] = Path(
            unit["process_resolution"][0]["path"]
        ).name
    elif mutation == "missing-content":
        del unit["process_resolution"][0]["content_path"]
    elif mutation == "role":
        unit["process_image_refs"][0]["role"] = "rust-link-helper"
    elif mutation == "duplicate-ref":
        row = dict(unit["process_image_refs"][0])
        row["path"] = _equivalent_image_spelling(row["path"], "dot")
        unit["process_image_refs"].append(row)
        unit["selected_process_count"] += 1
    elif mutation in {"digest", "disposition"}:
        row = dict(
            next(
                row
                for row in identity["process_images"]
                if row["role"] == "rust-linker"
            )
        )
        row["path"] = _equivalent_image_spelling(row["path"], "dot")
        row["sha256" if mutation == "digest" else "root_exit_disposition"] = (
            "0" * 64 if mutation == "digest" else "terminate"
        )
        identity["process_images"].append(row)
    elif mutation == "helper-digest":
        native["compilers"]["c"]["phases"][0]["helpers"][0]["sha256"] = "0" * 64
    else:
        helper = native["compilers"]["c"]["phases"][0]["helpers"][0]
        for row in identity["process_images"]:
            if row["path"] == helper["path"]:
                row["role"] = "rust-link-helper"
    with pytest.raises(ValueError) as caught:
        toolchain_capture.validate_rust_link_selection(identity, full_capture=True)
    if mutation == "different-path":
        detail = json.loads(str(caught.value).split("; image_edge=", 1)[1])
        assert detail["unit"] == unit["unit"]
        assert detail["role"] == "rust-linker"
        assert detail["selected_path"] == str(original)
        assert detail["content_path"] == str(foreign)
        assert {
            "role": "rust-linker",
            "path": process_image_capture._image_path_key(original),
        } in detail["captured_images"]
        assert "sha256" not in detail and "environment" not in detail


def test_native_c_capture_uses_actual_helpers_and_independent_archiver(
    tmp_path, monkeypatch
):
    identity, tools, env, command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    selected = toolchain_capture.validate_rust_link_selection(
        identity, required_native_units={"target": ["c"]}
    )
    unit = selected["native_build"][0]
    assert unit["selection"]["compilers"]["c"] == [str(tools["selected-gcc"])]
    assert unit["selection"]["archiver"] == str(tools["selected-ar"])
    assert [row["language"] for row in unit["compilers"]["c"]["phases"]] == [
        "c",
        "assembler-with-cpp",
    ]
    assert {
        process_image_capture._image_path_key(tools[name])
        for name in ("selected-gcc", "selected-ar", "cc1", "as")
    } <= {
        process_image_capture._image_path_key(Path(row.path))
        for row in toolchain_capture.frozen_files(identity)
    }
    assert sum("-###" in command for command in calls) == 2
    before = len(calls)
    toolchain_capture.revalidate_rust_link_process_images(
        identity,
        target=None,
        command_argv=command,
        required_native_units={"target": ["c"]},
    )
    assert len(calls) == before
    assert toolchain_capture.native_build_environment(selected)[
        "CC_x86_64_unknown_linux_gnu"
    ] == str(tools["selected-gcc"])
    assert toolchain_capture.native_compiler_selection_is_current(
        unit["compilers"]["c"], env=env
    )


@pytest.mark.parametrize("member", ["selected-gcc", "selected-ar", "cc1", "as"])
def test_native_c_capture_refuses_changed_actual_image(tmp_path, monkeypatch, member):
    identity, tools, _env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    tools[member].write_bytes(b"replacement")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            identity,
            target=None,
            command_argv=command,
            required_native_units={"target": ["c"]},
        )


def test_rust_only_capture_has_no_native_c_probe_or_selection(tmp_path, monkeypatch):
    identity, _tools, _env, _command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, required=False
    )
    assert identity["link_selection"]["native_build"] == []
    assert not any("-###" in command for command in calls)
    toolchain_capture.validate_rust_link_selection(identity, required_native_units={})


@pytest.mark.parametrize(
    "mutation",
    ["units", "required", "phase", "helper", "archiver", "command", "resources"],
)
def test_native_c_capture_receivers_reject_resealed_omissions(
    tmp_path, monkeypatch, mutation
):
    import copy

    identity, _tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(cas, {"rustc": identity})
    changed = copy.deepcopy(identity)
    if mutation == "units":
        changed["link_selection"]["native_build"] = []
    elif mutation == "required":
        del changed["link_selection"]["native_required"]
    elif mutation == "phase":
        changed["link_selection"]["native_build"][0]["compilers"]["c"]["phases"].pop()
    elif mutation == "command":
        changed["link_selection"]["admitted_command"] = []
        changed["link_selection"]["native_build"] = []
        changed["link_selection"]["native_required"] = []
    elif mutation == "resources":
        changed["link_selection"]["native_build"][0]["resources"] = None
    else:
        changed["process_images"] = [
            row
            for row in changed["process_images"]
            if not (
                row["role"].startswith("rust-build-native-archiver-")
                if mutation == "archiver"
                else "helpers-outside-bin" in row["path"]
            )
        ]
        changed["link_selection"]["selected_process_count"] = len(
            changed["process_images"]
        )
    with pytest.raises(ValueError):
        toolchain_capture.publish_capture(cas, {"rustc": changed})
    raw = custody_cas.read_ref(reference, expected_root=cas)
    raw["toolchains"] = {"rustc": changed}
    raw["files"] = [row.as_dict() for row in toolchain_capture.frozen_files(changed)]
    forged = custody_cas.put_json(cas, raw).as_dict()
    with pytest.raises(ValueError):
        toolchain_capture.load_capture(forged, cas_root=cas)


@pytest.mark.parametrize("mutation", ["none", "new-member", "file-content"])
def test_native_c_armed_capture_covers_membership_and_reads_each_input_once(
    tmp_path, monkeypatch, mutation
):
    from tools.proof_queue_pkg import (
        command_admission,
        command_identity,
        execution_environment,
    )

    directory_calls = []
    real_directory = command_identity._directory_manifest_identity

    def counted_directory(path, **kwargs):
        directory_calls.append(path)
        return real_directory(path, **kwargs)

    monkeypatch.setattr(
        command_identity, "_directory_manifest_identity", counted_directory
    )
    identity, tools, env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, resources=True, armed=False
    )
    assert directory_calls == []
    # Keep the real configuration authority for every admitted toolchain.
    # The handcrafted Rust transcript supplies compiler observations only.
    identity["configuration_files"] = command_identity._tool_configuration_identities(
        "rustc", cwd=tmp_path, env=env, command_argv=command
    )
    identity.pop("identity_sha256")
    identity["identity_sha256"] = canonical_json_sha256(identity)
    from tools import proof_plan

    probes = len(_calls)
    assert command_identity._reused_identity_is_current(
        proof_plan.ToolchainPolicy("rustc", {}),
        identity,
        cwd=tmp_path,
        env=env,
        command_argv=command,
        native_units={"target": ["c"]},
    )
    assert directory_calls == [] and len(_calls) == probes
    if mutation == "new-member":
        (tmp_path / "include" / "added.h").write_text(
            "#define ADDED 1\n", encoding="utf-8"
        )
    elif mutation == "file-content":
        (tmp_path / "forced.h").write_text("#define FORCED 2\n", encoding="utf-8")
    monkeypatch.setattr(
        command_identity, "_python_identity", lambda *args, **kwargs: None
    )
    envelope = command_admission.envelope_for_command(command)
    assert envelope["toolchains"] == ["rustc", "cargo", "git"]
    assert envelope["cargo_native_units"] == {"target": ["c"]}
    env["CARGO"] = str(tools["cargo"])
    # Git is admitted for source custody in addition to the build's Rust tools.
    # Give it a real executable image on this fixture's exclusive PATH.
    tools["git"] = tools["as"].with_name("git.exe" if os.name == "nt" else "git")
    tools["git"].write_bytes(b"fixture git executable")
    tools["git"].chmod(0o755)
    version_calls = []
    # Rust version probes use the resolved component; Git uses its selected
    # custody entrypoint. Product/native-C configuration coordinates stay raw.
    versions = {
        str(tools["cargo"].resolve(strict=True)): "cargo 1.99.0\n",
        process_image_capture._image_path_key(tools["git"]): "git version 2.49.0\n",
    }

    def tool_version(argv, **kwargs):
        version_calls.append(list(argv))
        assert list(argv) == [argv[0], "--version"]
        return subprocess.CompletedProcess(argv, 0, versions[argv[0]], "")

    # Only version processes are synthetic. Resolve and capture all remaining
    # admitted roles with the real policy, image and configuration authorities.
    monkeypatch.setattr(command_identity, "_run_captured", tool_version)
    plan = proof_plan.ProofPlan.load()
    located = {"rustc": identity}
    for name in envelope["toolchains"]:
        if name != "rustc":
            located[name] = command_identity._tool_identity(
                plan, name, envelope, command, cwd=tmp_path, env=env
            )
    assert version_calls == [
        [str(tools["cargo"].resolve(strict=True)), "--version"],
        [process_image_capture._image_path_key(tools["git"]), "--version"],
    ]
    assert set(located) == {"rustc", "cargo", "git"}
    assert located["git"]["path"] == process_image_capture._image_path_key(tools["git"])
    assert located["git"]["version"] == "git version 2.49.0"
    # Git's canonical selector currently has no configuration-file inputs;
    # exercise that authority instead of fabricating configuration custody.
    assert located["git"]["configuration_files"] == []
    original_open = Path.open
    reads = []

    def counted_open(path, mode="r", *args, **kwargs):
        if mode == "rb":
            reads.append(path)
        return original_open(path, mode, *args, **kwargs)

    def capture():
        return execution_environment._capture_toolchains(
            envelope,
            command,
            cwd=tmp_path,
            env=env,
            source_root=tmp_path,
            hash_workers=1,
            located_toolchains=located,
        )

    # Instrument only the operation, never pytest/report teardown.
    with monkeypatch.context() as scope:
        scope.setattr(Path, "open", counted_open)
        _, captured = capture()
    assert set(captured) == {"rustc", "cargo", "git"}
    assert captured["git"]["process_images"] == located["git"]["process_images"]
    for name in located:
        assert (
            captured[name]["configuration_files"]
            == located[name]["configuration_files"]
        )
    assert directory_calls == [tmp_path / "include"]
    assert identity["link_selection"]["native_build"][0]["resources"] is None
    files = {
        process_image_capture._image_path_key(Path(row.path))
        for row in toolchain_capture.frozen_files(captured)
    }
    assert process_image_capture._image_path_key(tools["git"]) in files
    for tool in located.values():
        assert {
            process_image_capture._image_path_key(Path(row["path"]))
            for row in tool["configuration_files"]
        } <= files
    if mutation == "new-member":
        assert (
            process_image_capture._image_path_key(tmp_path / "include" / "added.h")
            in files
        )
    for path in [
        *(
            tools[name]
            for name in (
                "rustc",
                "cargo",
                "git",
                "linker",
                "selected-gcc",
                "selected-ar",
                "cc1",
                "as",
            )
        ),
        tmp_path / "include" / "header.h",
        tmp_path / "forced.h",
    ]:
        assert reads.count(path) == 1, (path, reads)
    # Once the armed inventory exists, both new members and changed bytes are
    # rejected at a subsequent verification boundary.
    if mutation == "new-member":
        (tmp_path / "include" / "late.h").write_text("late header", encoding="utf-8")
    else:
        (tmp_path / "forced.h").write_text("late file mutation", encoding="utf-8")
    with pytest.raises(ValueError, match="resource.*changed while live custody armed"):
        toolchain_capture.revalidate_rust_link_process_images(
            captured["rustc"],
            target=None,
            command_argv=command,
            required_native_units={"target": ["c"]},
        )


@pytest.mark.parametrize(
    "mutation", ["digest", "path", "directory-member", "parent-escape"]
)
def test_native_c_resource_receivers_reject_resealed_substitutions(
    tmp_path, monkeypatch, mutation
):
    import copy

    identity, _tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, resources=True
    )
    cas = tmp_path / "cas"
    _, reference, _ = toolchain_capture.publish_capture(cas, {"rustc": identity})
    changed = copy.deepcopy(identity)
    resources = changed["link_selection"]["native_build"][0]["resources"]
    file = next(row for row in resources if "path" in row)
    if mutation == "digest":
        file.pop("sha256")
    elif mutation == "path":
        file["path"] = str(tmp_path / "include" / "header.h")
        header = tmp_path / "include" / "header.h"
        file["sha256"] = hashlib.sha256(header.read_bytes()).hexdigest()
        file["size_bytes"] = header.stat().st_size
    else:
        directory = next(row for row in resources if "root" in row)
        directory["files"][0]["resolved_path"] = (
            directory["root"] + "/../forced.h"
            if mutation == "parent-escape"
            else file["path"]
        )
        directory["files"][0]["sha256"] = file["sha256"]
        directory["files"][0]["size"] = file["size_bytes"]
        directory["files"][0]["symlinked"] = True
        directory["manifest_sha256"] = canonical_json_sha256(directory["files"])
    changed.pop("identity_sha256")
    changed["identity_sha256"] = canonical_json_sha256(changed)
    with pytest.raises(ValueError, match="resource"):
        toolchain_capture.publish_capture(cas, {"rustc": changed})
    raw = custody_cas.read_ref(reference, expected_root=cas)
    raw["toolchains"] = {"rustc": changed}
    if mutation == "parent-escape":
        # Forge the complete transport independently: the production projector
        # now refuses this coordinate before the receiver could observe it.
        header = process_image_capture._image_path_key(
            tmp_path / "include" / "header.h"
        )
        original = [
            row
            for row in raw["files"]
            if process_image_capture._image_path_key(Path(row["path"])) == header
        ]
        assert len(original) == 1
        forced = (tmp_path / "forced.h").read_bytes()
        replacement = {
            "path": directory["files"][0]["resolved_path"],
            "sha256": hashlib.sha256(forced).hexdigest(),
            "size": len(forced),
        }
        assert ".." in Path(replacement["path"]).parts
        assert not any(row["path"] == replacement["path"] for row in raw["files"])
        retained = [
            row
            for row in raw["files"]
            if process_image_capture._image_path_key(Path(row["path"])) != header
        ]
        assert any(
            process_image_capture._image_path_key(Path(row["path"]))
            == process_image_capture._image_path_key(tmp_path / "forced.h")
            for row in retained
        )
        raw["files"] = sorted([*retained, replacement], key=lambda row: row["path"])
    else:
        raw["files"] = [
            row.as_dict() for row in toolchain_capture.frozen_files(changed)
        ]
    forged = custody_cas.put_json(cas, raw).as_dict()
    with pytest.raises(ValueError, match="resource"):
        toolchain_capture.load_capture(forged, cas_root=cas)


@pytest.mark.parametrize("failure", ["nonzero", "unparseable", "missing-helper"])
def test_native_c_phase_failure_preserves_completed_probe_diagnostics(
    tmp_path, monkeypatch, failure
):
    driver = tmp_path / "cc"
    driver.write_bytes(b"independent compiler fixture")
    stderr = (
        ' "missing-native-helper" "unit.c"\n'
        if failure == "missing-helper"
        else "compiler diagnostic without a command"
    )

    def run(command, **kwargs):
        return subprocess.CompletedProcess(
            command, 17 if failure == "nonzero" else 0, "retained stdout", stderr
        )

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    with pytest.raises(toolchain_capture.RustLinkCaptureError) as caught:
        toolchain_capture.capture_native_compiler_process_images(
            [str(driver)],
            role="test-native-c",
            language="c",
            target="x86_64-unknown-linux-gnu",
            cwd=tmp_path,
            env={"PATH": str(tmp_path)},
        )
    diagnostic = caught.value.diagnostic
    assert diagnostic["unit"] == "test-native-c"
    assert diagnostic["phase"] == "native-compile-c"
    assert len(diagnostic["probes"]) == 1
    probe = diagnostic["probes"][0]
    assert probe["argv"][0:2] == [str(driver), "-###"]
    assert probe["stdout"] == "retained stdout" and probe["stderr"] == stderr
    assert probe["returncode"] == (17 if failure == "nonzero" else 0)


def test_native_c_cross_target_requires_actual_effective_command(tmp_path, monkeypatch):
    _identity, tools, env, command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    calls.clear()
    env.update(
        CC_aarch64_unknown_linux_gnu=str(tools["selected-gcc"]),
        AR_aarch64_unknown_linux_gnu=str(tools["selected-ar"]),
    )
    with pytest.raises(ValueError, match="effective cc-rs compiler command"):
        toolchain_capture.capture_rust_link_process_images(
            rustc=tools["rustc"],
            cargo=tools["cargo"],
            cwd=tmp_path,
            env=env,
            target="aarch64-unknown-linux-gnu",
            command_argv=command,
            native_units={"target": ["c"]},
            rustc_version="rustc 1.99.0\nhost: x86_64-unknown-linux-gnu\n",
        )
    assert not any("-###" in argv for argv in calls)


def test_directory_resource_producer_receiver_roundtrip_with_unicode(tmp_path):
    from tools.proof_queue_pkg import command_identity

    root = tmp_path / "répertoire"
    root.mkdir()
    (root / "entête.h").write_text("#define UNICODE 1\n", encoding="utf-8")
    captured = command_identity._directory_manifest_identity(
        root, label="Unicode fixture"
    )
    independent = hashlib.sha256(
        json.dumps(
            captured["files"], ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
    ).hexdigest()
    assert captured["manifest_sha256"] == independent
    command_identity._validate_directory_manifest_identity(captured, selected_root=root)
    command_identity._revalidate_directory_manifest_identity(
        captured, selected_root=root, label="Unicode fixture"
    )
    (root / "ajouté.h").write_text("changed membership", encoding="utf-8")
    with pytest.raises(ValueError, match="membership or content changed"):
        command_identity._revalidate_directory_manifest_identity(
            captured, selected_root=root, label="Unicode fixture"
        )


@pytest.mark.parametrize(
    "crate_types,links",
    [
        ("lib", False),
        ("rlib", False),
        ("staticlib", False),
        ("cdylib", True),
        ("staticlib,cdylib", True),
        ("proc-macro", True),
        ("rlib,rlib", False),
        ("cdylib,cdylib", True),
    ],
)
def test_cargo_capture_preserves_explicit_library_artifacts_and_host(
    tmp_path, monkeypatch, crate_types, links
):
    tools = {}
    for name in ("rustc", "cargo", "target-linker", "host-linker"):
        path = tmp_path / (name + (".exe" if os.name == "nt" else ""))
        path.write_bytes(("image:" + name).encode())
        path.chmod(0o755)
        tools[name] = path
    observed = []

    def run(command, **kwargs):
        assert command[1] not in {"metadata", "pkgid"}, (
            "exact Cargo override must not rediscover manifest kinds"
        )
        metadata = _rust_metadata_probe(command, tmp_path)
        if metadata is not None:
            return metadata
        manifest = Path(command[command.index("--manifest-path") + 1])
        text = manifest.read_text(encoding="utf-8")
        source = (
            manifest.parent / ("host.rs" if "proc-macro=true" in text else "main.rs")
        ).read_text(encoding="utf-8")
        observed.append((list(command), text, source))
        assert "[lib]" in text and "[[bin]]" not in text
        assert "#![no_std]" not in source
        host = "proc-macro=true" in text
        assert "--lib" in command and "--bin" not in command
        if host:
            assert "--crate-type" not in command
            assert source == "extern crate proc_macro;\n"
        else:
            assert source == "fn main() {}\n#[test]\nfn proof_link_test() {}\n"
            assert command.index("--crate-type") < command.index("--")
            assert command[command.index("--crate-type") + 1] == crate_types
        selected = tools["host-linker" if host else "target-linker"]
        output = (
            json.dumps(str(selected)) + ' "std-library-probe.o"\n'
            if host or links
            else ""
        )
        return subprocess.CompletedProcess(command, 0, output, "")

    monkeypatch.setattr(toolchain_capture, "_COMMANDS", SimpleNamespace(run=run))
    command = ["cargo", "rustc", "--lib", "--crate-type", crate_types]
    images, selection = toolchain_capture.capture_rust_link_process_images(
        rustc=tools["rustc"],
        cargo=tools["cargo"],
        cwd=tmp_path,
        env={"PATH": str(tmp_path)},
        target="wasm32-wasip1",
        command_argv=command,
    )
    assert len(observed) == 2
    target_unit, host_unit = selection["units"]
    assert target_unit["artifact_selection"] == {
        "cargo_crate_types": crate_types.split(","),
        "rustc_crate_types": [],
        "manifest_crate_types": None,
        "link_required": links,
    }
    assert host_unit["artifact_selection"] == {
        "cargo_crate_types": None,
        "rustc_crate_types": ["proc-macro"],
        "manifest_crate_types": None,
        "link_required": True,
    }
    assert target_unit["selected_process_count"] == int(links)
    assert {row["path"] for row in images} == {
        toolchain_capture._image_path_key(tools["host-linker"]),
        *([toolchain_capture._image_path_key(tools["target-linker"])] if links else []),
    }
    identity = {"process_images": images, "link_selection": selection}
    toolchain_capture.validate_rust_link_selection(identity, full_capture=True)
    _, reference, _ = toolchain_capture.publish_capture(
        tmp_path / "cas", {"rustc": identity}
    )
    loaded = toolchain_capture.load_capture(reference, cas_root=tmp_path / "cas")
    assert loaded["toolchains"]["rustc"] == identity
    if links:
        incomplete = __import__("copy").deepcopy(identity)
        incomplete["link_selection"]["units"][0]["process_resolution"] = []
        with pytest.raises(ValueError, match="target linker resolution"):
            toolchain_capture.validate_rust_link_selection(
                incomplete, full_capture=True
            )
        incomplete["link_selection"]["units"][0]["process_image_refs"] = []
        with pytest.raises(ValueError, match="image membership"):
            toolchain_capture.validate_rust_link_selection(
                incomplete, full_capture=True
            )
    for mutation in ("omit", "invert"):
        altered = __import__("copy").deepcopy(loaded)
        artifact = altered["toolchains"]["rustc"]["link_selection"]["units"][0]
        if mutation == "omit":
            del artifact["artifact_selection"]
        else:
            artifact["artifact_selection"]["link_required"] = not links
        with pytest.raises(ValueError, match="artifact selection"):
            toolchain_capture.publish_capture(tmp_path / "cas", altered["toolchains"])
        raw = custody_cas.put_json(tmp_path / "cas", altered).as_dict()
        with pytest.raises(ValueError, match="artifact selection"):
            toolchain_capture.load_capture(raw, cas_root=tmp_path / "cas")
    # An archive-shaped record cannot erase an actual cdylib obligation even
    # if its command digest and all optional counts are resealed coherently.
    if not links:
        altered = __import__("copy").deepcopy(identity)
        altered["link_selection"]["admitted_command"] = [
            "cargo",
            "rustc",
            "--lib",
            "--crate-type",
            "cdylib",
        ]
        altered["link_selection"]["command_semantics_sha256"] = canonical_json_sha256(
            altered["link_selection"]["admitted_command"]
        )
        with pytest.raises(ValueError, match="producer command|artifact selection"):
            toolchain_capture.validate_rust_link_selection(altered, full_capture=True)


@pytest.mark.parametrize("owner", ["cargo", "delegated-cargo", "rustc"])
def test_rust_artifact_capture_retains_admitted_cargo_role_after_custom_binding(
    tmp_path, monkeypatch, owner
):
    from tools import proof_plan
    from tools.proof_queue_pkg import command_admission, command_identity

    _identity, tools, env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, required=False
    )
    selected_name = "custom-cargo" if owner != "rustc" else "rustc"
    selected = tmp_path / (selected_name + (".exe" if os.name == "nt" else ""))
    selected.write_bytes(b"independently selected Cargo executable")
    selected.chmod(0o755)
    payload = (
        ["cargo", "rustc", "--lib", "--crate-type", "staticlib"]
        if owner != "rustc"
        else [str(selected), "--crate-type", "staticlib"]
    )
    admitted = (
        [sys.executable, "tools/guarded_exec.py", "--", *payload]
        if owner == "delegated-cargo"
        else payload
    )
    envelope = command_admission.envelope_for_command(admitted)
    selected_env = {**env, "CARGO": str(selected)}
    exact = command_identity._exact_command(
        envelope, cwd=proof_plan.ROOT, env=selected_env
    )
    command_identity._bind_delegated_command(
        envelope, exact, cwd=proof_plan.ROOT, env=selected_env
    )
    producer = command_admission._nested_command(exact) or exact
    assert producer == [str(selected), *payload[1:]]
    _calls.clear()
    images, selection = toolchain_capture.capture_rust_link_process_images(
        rustc=tools["rustc"] if owner != "rustc" else selected,
        cargo=selected if owner != "rustc" else None,
        cwd=tmp_path,
        env=selected_env,
        target=None,
        command_argv=producer,
        admitted_command=admitted,
        rustc_version="rustc 1.99.0\nhost: x86_64-unknown-linux-gnu\n",
    )
    link_probes = [argv for argv in _calls if "link-args" in argv]
    assert len(link_probes) == (2 if owner != "rustc" else 1)
    assert link_probes[0][link_probes[0].index("--crate-type") + 1] == "staticlib"
    if owner != "rustc":
        assert "--crate-type" not in link_probes[1]
        # The temporary file is retired after capture; the host command itself
        # independently identifies its proc-macro library probe.
        assert "--lib" in link_probes[1]
    assert selection["producer_command"] == producer
    assert selection["units"][0]["artifact_selection"] == {
        "cargo_crate_types": ["staticlib"] if owner != "rustc" else None,
        "rustc_crate_types": [] if owner != "rustc" else ["staticlib"],
        "manifest_crate_types": None,
        "link_required": False,
    }
    assert selection["units"][0]["selected_process_count"] == 0
    if owner != "rustc":
        assert selection["units"][1]["artifact_selection"]["link_required"] is True
    else:
        assert len(selection["units"]) == 1
    retained = {"process_images": images, "link_selection": selection}
    assert (
        toolchain_capture.validate_rust_link_selection(retained, command_argv=producer)
        == selection
    )
    with pytest.raises(ValueError, match="actual admission"):
        toolchain_capture.validate_rust_link_selection(
            retained, command_argv=[str(selected), "check"]
        )


@pytest.mark.parametrize("same_bytes", [False, True])
@pytest.mark.parametrize("coordinate", ["resolution", "native-helper"])
def test_parent_traversal_cannot_borrow_another_captured_image(
    tmp_path, monkeypatch, same_bytes, coordinate
):
    identity, tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch
    )
    original = tools["linker" if coordinate == "resolution" else "cc1"]
    foreign = tmp_path / "foreign"
    (foreign / "deep").mkdir(parents=True)
    other = foreign / original.name
    other.write_bytes(original.read_bytes() if same_bytes else b"foreign image")
    hop = original.parent / "hop"
    try:
        hop.symlink_to(foreign / "deep", target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlink capability unavailable: {exc}")
    witness = hop / ".." / original.name
    # Win32 normalizes the lexical parent before following the directory
    # link; POSIX follows the link first. Both coordinates remain unsupported
    # by proof custody, including when the two executable byte strings match.
    expected, excluded = (
        (original, other) if sys.platform == "win32" else (other, original)
    )
    assert witness.samefile(expected) and not witness.samefile(excluded)
    if coordinate == "resolution":
        identity["link_selection"]["units"][0]["process_resolution"][0][
            "content_path"
        ] = str(witness)
    else:
        compiler = identity["link_selection"]["native_build"][0]["compilers"]["c"]
        helper = compiler["phases"][0]["helpers"][0]
        helper["path"] = str(witness)
        helper["command"][0] = str(witness)
        compiler["probes"][0]["stderr"] = compiler["probes"][0]["stderr"].replace(
            json.dumps(str(original)), json.dumps(str(witness))
        )
    with pytest.raises(ValueError, match="parent traversal"):
        toolchain_capture.validate_rust_link_selection(identity, full_capture=True)


def test_image_membership_diagnostic_bounds_untrusted_coordinates():
    visited = 0

    def captured():
        nonlocal visited
        for _ in range(100):
            visited += 1
            yield "r" * 10000, "p" * 10000

    error = toolchain_capture._image_membership_error(
        "missing image",
        unit="u" * 10000,
        role="r" * 10000,
        selected="s" * 10000,
        content="c" * 10000,
        images=captured(),
    )
    detail = json.loads(str(error).split("; image_edge=", 1)[1])
    assert visited == 9
    assert len(detail["captured_images"]) == 8
    assert detail["captured_images_truncated"] is True
    assert len(str(error)) < 12000
    assert detail["content_path"] == "c" * 512


@pytest.mark.parametrize(
    "operation,requirements,drivers",
    [
        ("c++", {"host": ["c++"]}, {"selected-g++"}),
        ("both", {"target": ["c", "c++"]}, {"selected-gcc", "selected-g++"}),
    ],
)
def test_native_cpp_capture_keeps_driver_helpers_language_and_build_role(
    tmp_path, monkeypatch, operation, requirements, drivers
):
    identity, tools, _env, command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, operation=operation
    )
    selection = toolchain_capture.validate_rust_link_selection(
        identity,
        required_native_units=requirements,
        full_capture=True,
    )
    native = selection["native_build"]
    assert len(native) == 1
    assert native[0]["selection"]["units"] == requirements
    assert {str(tools[name]) for name in drivers} == {
        call[0] for call in calls if "-###" in call
    }
    assert sum("-###" in call for call in calls) == 2 * len(drivers)
    assert native[0]["compilers"]["c++"]["language"] == "c++"
    # Captured helper and frozen-file paths are custody records (HF-154).
    assert native[0]["compilers"]["c++"]["phases"][0]["helpers"][0][
        "path"
    ] == custody_spelling(tools["cc1plus"])
    frozen = {str(row.path) for row in toolchain_capture.frozen_files(identity)}
    assert custody_spelling(tools["cc1plus"]) in frozen
    assert custody_spelling(tools["selected-g++"]) in frozen
    assert (
        sum(
            row["role"].startswith("rust-build-native-archiver-")
            for row in identity["process_images"]
        )
        == 1
    )
    before = list(calls)
    toolchain_capture.revalidate_rust_link_process_images(
        identity,
        target=None,
        command_argv=command,
        required_native_units=requirements,
    )
    assert calls == before


@pytest.mark.parametrize(
    "mutation",
    ["language", "role", "compiler", "helper", "extra-language", "retired-schema"],
)
def test_native_cpp_receipt_refuses_resealed_language_role_and_helper_omissions(
    tmp_path, monkeypatch, mutation
):
    import copy

    identity, tools, _env, _command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, operation="c++"
    )
    changed = copy.deepcopy(identity)
    native = changed["link_selection"]["native_build"][0]
    if mutation == "retired-schema":
        changed["link_selection"]["schema"] = (
            "molt.proof-rust-link-selection-telemetry.v4"
        )
    elif mutation == "language":
        native["compilers"]["c++"]["language"] = "c"
    elif mutation == "role":
        native["selection"]["units"] = {"target": ["c++"]}
    elif mutation == "compiler":
        del native["selection"]["compilers"]["c++"]
    elif mutation == "extra-language":
        native["selection"]["compilers"]["c"] = [str(tools["selected-gcc"])]
    else:
        changed["process_images"] = [
            row
            for row in changed["process_images"]
            if row["path"] != str(tools["cc1plus"])
        ]
        changed["link_selection"]["selected_process_count"] -= 1
    changed.pop("identity_sha256")
    changed["identity_sha256"] = canonical_json_sha256(changed)
    with pytest.raises(ValueError):
        toolchain_capture.publish_capture(tmp_path / "cas", {"rustc": changed})


def test_native_c_and_cpp_share_one_armed_resource_inventory(tmp_path, monkeypatch):
    from tools.proof_queue_pkg import command_identity

    observed = []
    real = command_identity._directory_manifest_identity

    def directory(path, **kwargs):
        observed.append(path)
        return real(path, **kwargs)

    monkeypatch.setattr(command_identity, "_directory_manifest_identity", directory)
    identity, _tools, _env, command, _calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, operation="both", resources=True, armed=False
    )
    assert observed == []
    _images, selection = toolchain_capture.revalidate_rust_link_process_images(
        identity,
        target=None,
        command_argv=command,
        required_native_units={"target": ["c", "c++"]},
    )
    assert observed == [tmp_path / "include"]
    resources = selection["native_build"][0]["resources"]
    assert {row["selected_root"] for row in resources} == {
        str(tmp_path / "include"),
        str(tmp_path / "forced.h"),
    }


@pytest.mark.parametrize("required", [{}, {"target": ["c++"]}, {"host": ["c"]}])
def test_native_build_requirement_mismatch_refuses_before_any_probe(
    tmp_path, monkeypatch, required
):
    _identity, tools, env, command, calls = _native_build_capture_fixture(
        tmp_path, monkeypatch, operation="c++"
    )
    calls.clear()
    with pytest.raises(ValueError, match="requirements differ from admitted command"):
        toolchain_capture.capture_rust_link_process_images(
            rustc=tools["rustc"],
            cargo=tools["cargo"],
            cwd=tmp_path,
            env=env,
            target=None,
            command_argv=command,
            native_units=required,
        )
    assert calls == [], "rejected authority must not launch metadata or compiler probes"
