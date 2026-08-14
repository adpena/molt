from __future__ import annotations

import copy
import hashlib
import json
import shutil
import threading
from contextlib import contextmanager
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any

import pytest

from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.source_extension_manifest_codec import (
    _compact_source_extension_manifest,
    _manifest_dependencies,
    _manifest_sequence,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_reproducibility import (
    _canonicalize_locations,
    _canonicalize_location_string_ordered,
    _require_location_neutral,
    _residual_producer_paths,
)
from molt.cli.source_extension_set_identity import (
    SOURCE_EXTENSION_SET_SCHEMA_VERSION,
    _source_extension_reproduction_comparison,
    _source_extension_set_identity,
)
from molt.cli.source_extension_publication import (
    _source_extension_publication_custody,
    publish_source_extension_candidate,
    recover_source_extension_publication,
)
from molt.cli.source_package_seal import SourcePackageInput, stage_source_package_seal
from molt.cli.source_package_seal import SourcePackageSealVerificationError


@contextmanager
def _held_publication_custody(destination: Path):
    lock_path = destination.parent / f".{destination.name}.producer.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message=f"cannot acquire fixture publication lock {lock_path}",
    )
    try:
        yield _source_extension_publication_custody(destination, handle)
    finally:
        _release_file_lock(handle)


def _publish_candidate(**kwargs: Any) -> dict[str, Any]:
    destination = kwargs["destination"]
    assert isinstance(destination, Path)
    with _held_publication_custody(destination) as custody:
        return publish_source_extension_candidate(custody=custody, **kwargs)


def _recover_publication(
    destination: Path, transaction_root: Path
) -> dict[str, Any] | None:
    with _held_publication_custody(destination) as custody:
        return recover_source_extension_publication(transaction_root, custody=custody)


def _manifest(object_count: int = 132) -> dict[str, object]:
    shared_dependencies = [
        {"path": f"../../inputs/header-{index}.h", "sha256": f"{index:064x}"}
        for index in range(64)
    ]
    objects = []
    for index in range(object_count):
        objects.append(
            {
                "source": f"../../inputs/source-{index}.c",
                "object": f"{index}.o",
                "source_sha256": hashlib.sha256(f"source-{index}".encode()).hexdigest(),
                "object_sha256": hashlib.sha256(f"object-{index}".encode()).hexdigest(),
                "defined_symbols": ["shared_defined", f"defined_{index}"],
                "undefined_symbols": ["shared_undefined"],
                "compile_command": [
                    "clang",
                    "-c",
                    f"../../inputs/source-{index}.c",
                    "-o",
                    f"@object-root/{index}.o",
                    "-MF",
                    f"@object-root/{index}.d",
                    "-MT",
                    f"{index}.o",
                    "-Xclang",
                    "-fsemantic-order-matters",
                    "-Xclang",
                    "-fsemantic-order-matters",
                ],
                "symbol_command": ["llvm-nm", "--defined-only"],
                "dependencies": copy.deepcopy(shared_dependencies),
            }
        )
    return {
        "module": "pkg._native",
        "target_triple": "wasm32-wasip1",
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "build": {
            "compiler": ["clang"],
            "extra_compile_args": ["-DVALUE=1", "-DVALUE=2", "-DVALUE=1"],
            "include_dirs": ["@source/include", "@source/include", "@build/include"],
        },
        "object_closure": {
            "schema_version": 1,
            "root_symbol": "PyInit__native",
            "runtime_symbols": [],
            "objects": objects,
        },
    }


def test_132_unit_manifest_compaction_reconstructs_exact_commands_and_content() -> None:
    manifest = _manifest()
    original = copy.deepcopy(manifest)
    original_bytes = len(json.dumps(original, sort_keys=True, indent=2).encode())

    compact = _compact_source_extension_manifest(manifest)
    _validate_compact_source_extension_manifest(compact)

    compact_bytes = len(json.dumps(compact, sort_keys=True, indent=2).encode())
    assert compact_bytes < original_bytes * 0.4
    build = compact["build"]
    assert isinstance(build, dict)
    assert _manifest_sequence(compact, build, "extra_compile_args") == [
        "-DVALUE=1",
        "-DVALUE=2",
        "-DVALUE=1",
    ]
    assert _manifest_sequence(compact, build, "include_dirs") == [
        "@source/include",
        "@source/include",
        "@build/include",
    ]
    compact_objects = compact["object_closure"]["objects"]
    original_objects = original["object_closure"]["objects"]
    for current, before in zip(compact_objects, original_objects, strict=True):
        assert (
            _manifest_sequence(compact, current, "compile_command")
            == before["compile_command"]
        )
        assert (
            _manifest_sequence(compact, current, "symbol_command")
            == before["symbol_command"]
        )
        assert _manifest_dependencies(compact, current) == before["dependencies"]


def test_compact_unit_identity_detects_per_unit_operand_divergence() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=2))
    first = compact["object_closure"]["objects"][0]
    operand = next(
        item
        for item in first["compile_command_operands"]
        if str(item["value"]).endswith(".o")
    )
    operand["value"] = "@object-root/diverged.o"
    with pytest.raises(ValueError, match="unit identity is false"):
        _validate_compact_source_extension_manifest(compact)


def test_command_template_roundtrip_preserves_literal_placeholder_tokens() -> None:
    manifest = _manifest(object_count=1)
    command = manifest["object_closure"]["objects"][0]["compile_command"]
    command.extend(["-DPLACEHOLDER=%{operand}", "/Fo%{source}"])
    original = list(command)
    compact = _compact_source_extension_manifest(manifest)
    item = compact["object_closure"]["objects"][0]
    assert _manifest_sequence(compact, item, "compile_command") == original
    _validate_compact_source_extension_manifest(compact)


def test_compaction_rejects_dependency_metadata_outside_canonical_pair() -> None:
    manifest = _manifest(object_count=1)
    manifest["object_closure"]["objects"][0]["dependencies"][0]["ambient"] = "drift"
    with pytest.raises(ValueError, match="dependencies is invalid"):
        _compact_source_extension_manifest(manifest)


def test_compact_manifest_rejects_unused_string_authority() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=1))
    compact["build_authorities"]["strings"].append("zzzz-unused-authority")
    with pytest.raises(ValueError, match="unused string authority"):
        _validate_compact_source_extension_manifest(compact)


def test_compact_manifest_rejects_unused_sequence_authority() -> None:
    compact = _compact_source_extension_manifest(_manifest(object_count=1))
    strings = compact["build_authorities"]["strings"]
    digest = hashlib.sha256(
        json.dumps([strings[0]], separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    compact["build_authorities"]["sequences"][digest] = [0]
    with pytest.raises(ValueError, match="unused or dangling sequence authority"):
        _validate_compact_source_extension_manifest(compact)


def test_path_canonicalization_handles_joined_flags_double_slashes_and_urls() -> None:
    root = Path("C:/Molt/target-root")
    payload = {
        "argv": [
            "-IC://Molt//target-root//include",
            "-L" + str(root / "lib"),
            "/LIBPATH:C:/Molt/target-root/lib",
            "--sysroot=C://Molt//target-root//sysroot",
            "@C:/Molt/target-root/response.rsp",
        ],
        "url": "https://example.invalid/C:/Molt/target-root/include",
    }
    canonical = _canonicalize_locations(payload, ((root, "@target"),))
    assert canonical["argv"] == [
        "-I@target/include",
        "-L@target/lib",
        "/LIBPATH:@target/lib",
        "--sysroot=@target/sysroot",
        "@@target/response.rsp",
    ]
    assert canonical["url"] == payload["url"]
    _require_location_neutral(canonical, authority="test manifest")


@pytest.mark.parametrize(
    "residual",
    [
        "-I/usr/local/include",
        "/usr",
        "-isystem /opt/sdk/include",
        "@/tmp/compiler.rsp",
        "file:///usr/local/include",
        "~/sdk/include",
        "$HOME/sdk/include",
        "${HOME}/sdk/include",
        "%USERPROFILE%/sdk/include",
        r"/LIBPATH:C:\sdk\lib",
        r"\\server\share\sdk\include",
    ],
)
def test_nested_residual_path_gate_rejects_every_compiler_path_form(
    residual: str,
) -> None:
    findings = _residual_producer_paths(
        {"outer": [{"extension": {"build": {"argv": [residual]}}}]}
    )
    assert findings
    assert "$.outer[0].extension.build.argv[0]" in findings[0]


def test_location_projection_is_invariant_across_windows_linux_and_macos() -> None:
    windows = [
        _canonicalize_location_string_ordered(
            value, ((PureWindowsPath("C:/work/repo"), "@repo"),)
        )
        for value in (r"C:\work\repo\src\module.c", r"-IC:\work\repo\include")
    ]
    linux = [
        _canonicalize_location_string_ordered(
            value, ((PurePosixPath("/home/agent/repo"), "@repo"),)
        )
        for value in ("/home/agent/repo/src/module.c", "-I/home/agent/repo/include")
    ]
    macos = [
        _canonicalize_location_string_ordered(
            value, ((PurePosixPath("/Users/agent/repo"), "@repo"),)
        )
        for value in ("/Users/agent/repo/src/module.c", "-I/Users/agent/repo/include")
    ]
    assert windows == linux == macos == ["@repo/src/module.c", "-I@repo/include"]


def _write_identity_fixture(
    root: Path, *, producer_root: str, artifact: str
) -> dict[str, str]:
    source = root / "pkg/__init__.py"
    source.parent.mkdir(parents=True)
    source.write_text("VALUE = 1\n", encoding="utf-8")
    artifact_path = root / "pkg/_native.molt.wasm"
    artifact_path.write_bytes(artifact.encode("ascii"))
    artifact_sha256 = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    payload = {
        "schema_version": 1,
        "version": "1.0.0",
        "module": "pkg._native",
        "extension_sha256": artifact_sha256,
        "wheel_sha256": "b" * 64,
        "python_tag": "py3",
        "target_python": "py312",
        "abi_tier": "cpython-abi",
        "target_triple": "wasm32-wasip1",
        "artifact_kind": "wasm_relocatable_object",
        "capabilities": ["module.extension.exec"],
        "python_exports": ["pkg"],
        "provided_capsules": [],
        "link_requirements": {
            "target_triple": "wasm32-wasip1",
            "items": [],
            "retained_symbols": [],
        },
        "source_plan": {"target_selector": "_native"},
        "build": {"producer_root": producer_root},
        "object_closure": {
            "schema_version": 1,
            "root_symbol": "PyInit__native",
            "init_symbol_owner": "0.o",
            "runtime_symbols": [],
            "objects": [
                {
                    "source": "../inputs/native.c",
                    "object": "0.o",
                    "source_sha256": "c" * 64,
                    "object_sha256": "d" * 64,
                    "defined_symbols": ["PyInit__native"],
                    "undefined_symbols": [],
                    "dependencies": [],
                    "compile_command": ["clang", "-c", "../inputs/native.c"],
                    "symbol_command": ["llvm-nm"],
                }
            ],
        },
    }
    payload = _compact_source_extension_manifest(payload)
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    set_manifest = {
        "schema_version": SOURCE_EXTENSION_SET_SCHEMA_VERSION,
        "kind": "molt-source-extension-set",
        "package": "pkg",
        "package_version": "1.0.0",
        "name": "test",
        "seal_name": "pkg-test",
        "cpython": "3.12",
        "source_head": "e" * 40,
        "submodules": [],
        "target": "wasm",
        "target_triple": "wasm32-wasip1",
        "abi_tier": "cpython-abi",
        "installed_package_files": ["pkg/__init__.py"],
        "target_metadata": {
            "abi": {
                "tier": "cpython-abi",
                "python_header_sha256": "f" * 64,
                "include_surface": {"sha256": "1" * 64},
            }
        },
        "extensions": [
            {
                "module": "pkg._native",
                "target": "_native",
                "python_exports": ["pkg"],
                "capabilities": ["module.extension.exec"],
                "provided_capsules": [],
                "exclude_linked_static_libraries": [],
            }
        ],
    }
    (root / "extension_set_manifest.json").write_text(
        json.dumps(set_manifest), encoding="utf-8"
    )
    return {
        "pkg/__init__.py": hashlib.sha256(source.read_bytes()).hexdigest(),
        "pkg/_native.molt.wasm": artifact_sha256,
        "pkg/_native.molt.wasm.extension_manifest.json": hashlib.sha256(
            sidecar.read_bytes()
        ).hexdigest(),
    }


@pytest.mark.parametrize("suffix", [".molt.wasm", ".molt.a"])
def test_extension_identity_rejects_extra_raw_artifact(
    tmp_path: Path, suffix: str
) -> None:
    root = tmp_path / suffix.removeprefix(".")
    inventory = _write_identity_fixture(
        root, producer_root="/producer/host", artifact="a" * 64
    )
    extra = root / f"pkg/extra{suffix}"
    extra.write_bytes(b"unregistered")
    inventory[extra.relative_to(root).as_posix()] = hashlib.sha256(
        extra.read_bytes()
    ).hexdigest()

    with pytest.raises(ValueError, match="artifact inventory differs from typed set"):
        _source_extension_set_identity(root, inventory_sha256=inventory)


def test_extension_identity_rejects_artifact_sidecar_digest_drift(
    tmp_path: Path,
) -> None:
    root = tmp_path / "digest-drift"
    inventory = _write_identity_fixture(
        root, producer_root="/producer/host", artifact="a" * 64
    )
    artifact = root / "pkg/_native.molt.wasm"
    artifact.write_bytes(b"tampered")

    with pytest.raises(ValueError, match="artifact bytes differ from sidecar"):
        _source_extension_set_identity(root, inventory_sha256=inventory)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("target_python", "py313"),
        ("target_triple", "x86_64-unknown-linux-gnu"),
        ("abi_tier", "source-compat"),
        ("artifact_kind", "static_archive"),
        ("module", "pkg._other"),
    ],
)
def test_extension_identity_rejects_sidecar_variant_drift(
    tmp_path: Path, field: str, value: str
) -> None:
    root = tmp_path / field
    inventory = _write_identity_fixture(
        root, producer_root="/producer/host", artifact="a" * 64
    )
    sidecar = root / "pkg/_native.molt.wasm.extension_manifest.json"
    payload = json.loads(sidecar.read_text(encoding="utf-8"))
    payload[field] = value
    sidecar.write_text(json.dumps(payload), encoding="utf-8")
    inventory[sidecar.relative_to(root).as_posix()] = hashlib.sha256(
        sidecar.read_bytes()
    ).hexdigest()

    with pytest.raises(ValueError, match="extension sidecar"):
        _source_extension_set_identity(root, inventory_sha256=inventory)


def test_canonical_identity_is_cross_platform_while_attestation_remains_exact(
    tmp_path: Path,
) -> None:
    windows = tmp_path / "windows"
    linux = tmp_path / "linux"
    windows_inventory = _write_identity_fixture(
        windows, producer_root="C:/build/worker", artifact="a" * 64
    )
    linux_inventory = _write_identity_fixture(
        linux, producer_root="/home/worker/build", artifact="a" * 64
    )
    windows_identity = _source_extension_set_identity(
        windows, inventory_sha256=windows_inventory
    )
    linux_identity = _source_extension_set_identity(
        linux, inventory_sha256=linux_inventory
    )
    assert windows_identity["canonical_sha256"] == linux_identity["canonical_sha256"]
    assert (
        windows_identity["producer_attestation_sha256"]
        != linux_identity["producer_attestation_sha256"]
    )

    divergent = tmp_path / "divergent"
    divergent_inventory = _write_identity_fixture(
        divergent, producer_root="/Users/worker/build", artifact="9" * 64
    )
    divergent_identity = _source_extension_set_identity(
        divergent, inventory_sha256=divergent_inventory
    )
    comparison = _source_extension_reproduction_comparison(
        expected_incumbent_sha256=windows_identity["canonical_sha256"],
        expected_candidate_sha256=windows_identity["canonical_sha256"],
        incumbent_seal_sha256="2" * 64,
        incumbent_identity=windows_identity,
        candidate_seal_sha256="3" * 64,
        candidate_identity=divergent_identity,
    )
    assert comparison["reproduced"] is False


def test_producer_attestation_covers_complete_verified_inventory(
    tmp_path: Path,
) -> None:
    root = tmp_path / "inventory"
    inventory = _write_identity_fixture(
        root, producer_root="/producer/host", artifact="a" * 64
    )
    baseline = _source_extension_set_identity(root, inventory_sha256=inventory)
    extended_inventory = dict(inventory)
    extended_inventory["provenance/logs/full-command.json"] = "7" * 64
    extended = _source_extension_set_identity(root, inventory_sha256=extended_inventory)
    assert extended["canonical_sha256"] == baseline["canonical_sha256"]
    assert (
        extended["producer_attestation_sha256"]
        != baseline["producer_attestation_sha256"]
    )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("module", "pkg..._native"),
        ("module", "pkg.class"),
        ("module", "pkg/../../escape"),
        ("module", r"pkg.\\escape"),
        ("target", "../escape"),
        ("target", r"..\\escape"),
        ("target", "C:escape"),
        ("target", "CON"),
        ("target", "nul.txt"),
        ("target", "a?b"),
        ("target", "x."),
        ("target", "x "),
        ("target", "x\x1f"),
    ],
)
def test_extension_identity_rejects_sidecar_path_escape(
    tmp_path: Path, field: str, value: str
) -> None:
    root = tmp_path / f"escape-{field}-{len(value)}"
    inventory = _write_identity_fixture(
        root, producer_root="/producer/host", artifact="a" * 64
    )
    manifest_path = root / "extension_set_manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["extensions"][0][field] = value
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="module is not import syntax|safe filename"):
        _source_extension_set_identity(root, inventory_sha256=inventory)


def _stage_identity_fixture(
    tmp_path: Path, *, label: str, artifact: str
) -> tuple[object, dict[str, object]]:
    payload = tmp_path / f"payload-{label}"
    _write_identity_fixture(
        payload, producer_root=f"/producer/{label}", artifact=artifact
    )
    store = tmp_path / f"store-{label}"
    seal = stage_source_package_seal(
        store,
        [
            SourcePackageInput(
                path,
                path.relative_to(payload).as_posix(),
                "fixture",
            )
            for path in sorted(payload.rglob("*"))
            if path.is_file()
        ],
    )
    identity = _source_extension_set_identity(
        seal.payload_root,
        inventory_sha256={entry.relative_path: entry.sha256 for entry in seal.files},
    )
    return seal, identity


def test_publication_preserves_incumbent_on_divergent_candidate_expectation(
    tmp_path: Path,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="incumbent", artifact="a" * 64
    )
    candidate, _candidate_identity = _stage_identity_fixture(
        tmp_path, label="candidate", artifact="9" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.root, destination)

    with pytest.raises(ValueError, match="canonical identity mismatch"):
        _publish_candidate(
            destination=destination,
            candidate_seal=candidate,
            transaction_root=tmp_path / "transaction",
            expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
            expected_candidate_identity_sha256="0" * 64,
        )

    assert (destination / "source-package-seal.json").read_bytes() == (
        incumbent.root / "source-package-seal.json"
    ).read_bytes()


def test_publication_performs_declared_identity_upgrade_and_recovers_crash(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_publication as publication

    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="old", artifact="a" * 64
    )
    candidate, candidate_identity = _stage_identity_fixture(
        tmp_path, label="new", artifact="9" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.root, destination)
    transaction = tmp_path / "transaction"
    real_replace = publication.os.replace

    def crash_after_retire(source: Path, target: Path) -> None:
        if Path(source).name == "candidate" and Path(target) == destination:
            raise OSError("simulated crash after incumbent retirement")
        real_replace(source, target)

    monkeypatch.setattr(publication.os, "replace", crash_after_retire)
    with pytest.raises(OSError, match="simulated crash"):
        _publish_candidate(
            destination=destination,
            candidate_seal=candidate,
            transaction_root=transaction,
            expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
            expected_candidate_identity_sha256=candidate_identity["canonical_sha256"],
        )
    assert not destination.exists()

    monkeypatch.setattr(publication.os, "replace", real_replace)
    recovered = _recover_publication(destination, transaction)
    assert recovered is not None and recovered["state"] == "committed"
    assert (destination / "source-package-seal.json").read_bytes() == (
        candidate.root / "source-package-seal.json"
    ).read_bytes()


def test_publication_exact_identity_is_noop_despite_attestation_drift(
    tmp_path: Path,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="host-a", artifact="a" * 64
    )
    candidate, candidate_identity = _stage_identity_fixture(
        tmp_path, label="host-b", artifact="a" * 64
    )
    assert (
        incumbent_identity["canonical_sha256"] == candidate_identity["canonical_sha256"]
    )
    assert (
        incumbent_identity["producer_attestation_sha256"]
        != candidate_identity["producer_attestation_sha256"]
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.root, destination)

    result = _publish_candidate(
        destination=destination,
        candidate_seal=candidate,
        transaction_root=tmp_path / "transaction",
        expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
        expected_candidate_identity_sha256=candidate_identity["canonical_sha256"],
    )
    assert result["no_op"] is True
    assert (destination / "source-package-seal.json").read_bytes() == (
        incumbent.root / "source-package-seal.json"
    ).read_bytes()


def test_publication_detects_stale_incumbent_race_before_retirement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import source_extension_publication as publication
    from molt.cli.source_package_seal import SourcePackageSealVerificationError

    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="race-old", artifact="a" * 64
    )
    candidate, candidate_identity = _stage_identity_fixture(
        tmp_path, label="race-new", artifact="9" * 64
    )
    stale, _stale_identity = _stage_identity_fixture(
        tmp_path, label="race-stale", artifact="8" * 64
    )
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.root, destination)
    real_resume = publication._resume_source_extension_publication

    def race(record_path: Path, custody: object) -> dict[str, object]:
        shutil.rmtree(destination)
        shutil.copytree(stale.root, destination)
        return real_resume(record_path, custody)

    monkeypatch.setattr(publication, "_resume_source_extension_publication", race)
    with pytest.raises(SourcePackageSealVerificationError):
        _publish_candidate(
            destination=destination,
            candidate_seal=candidate,
            transaction_root=tmp_path / "transaction",
            expected_incumbent_identity_sha256=incumbent_identity["canonical_sha256"],
            expected_candidate_identity_sha256=candidate_identity["canonical_sha256"],
        )
    assert (destination / "source-package-seal.json").read_bytes() == (
        stale.root / "source-package-seal.json"
    ).read_bytes()


def test_publication_rejects_released_producer_lock_custody(tmp_path: Path) -> None:
    destination = tmp_path / "canonical"
    lock_path = destination.parent / f".{destination.name}.producer.lock"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=1.0,
        timeout_message="fixture lock unavailable",
    )
    custody = _source_extension_publication_custody(destination, handle)
    _release_file_lock(handle)
    with pytest.raises(SourcePackageSealVerificationError, match="live exclusive"):
        recover_source_extension_publication(tmp_path / "transaction", custody=custody)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing-field",
        "unknown-kind",
        "unknown-state",
        "bad-hash",
        "extra-authority",
        "relative-path",
        "invalid-path",
        "noncanonical-path",
    ],
)
def test_publication_recovery_rejects_malformed_record_without_mutation(
    tmp_path: Path, mutation: str
) -> None:
    destination = (tmp_path / "canonical").resolve()
    destination.mkdir()
    marker = destination / "incumbent.txt"
    marker.write_text("preserve\n", encoding="utf-8")
    transaction = (tmp_path / "transaction").resolve()
    transaction.mkdir()
    publication_root = transaction / "identity-publication"
    record: dict[str, object] = {
        "schema_version": 1,
        "kind": "source-extension-seal-compare-and-swap",
        "state": "prepared",
        "destination": str(destination),
        "candidate": str(publication_root / "candidate"),
        "retired": str(publication_root / "retired"),
        "incumbent_seal_sha256": "1" * 64,
        "candidate_seal_sha256": "2" * 64,
        "incumbent_identity_sha256": "3" * 64,
        "candidate_identity_sha256": "4" * 64,
    }
    if mutation == "missing-field":
        record.pop("candidate_identity_sha256")
    elif mutation == "unknown-kind":
        record["kind"] = "legacy-publication"
    elif mutation == "unknown-state":
        record["state"] = "mystery"
    elif mutation == "bad-hash":
        record["candidate_seal_sha256"] = "NOT-A-HASH"
    elif mutation == "extra-authority":
        record["ambient_destination"] = str(tmp_path / "escape")
    elif mutation == "relative-path":
        record["candidate"] = "identity-publication/candidate"
    elif mutation == "invalid-path":
        record["candidate"] = "C:\\invalid\0path"
    else:
        record["destination"] = str(
            destination.parent / "nonexistent" / ".." / destination.name
        )
    (transaction / "identity-publication.json").write_text(
        json.dumps(record), encoding="utf-8"
    )

    with _held_publication_custody(destination) as custody:
        with pytest.raises(SourcePackageSealVerificationError):
            recover_source_extension_publication(transaction, custody=custody)

    assert marker.read_text(encoding="utf-8") == "preserve\n"
    assert not publication_root.exists()


def test_destination_scoped_custody_serializes_competing_cas_publishers(
    tmp_path: Path,
) -> None:
    incumbent, incumbent_identity = _stage_identity_fixture(
        tmp_path, label="race-incumbent", artifact="a" * 64
    )
    candidates = [
        _stage_identity_fixture(tmp_path, label=label, artifact=artifact)
        for label, artifact in (("race-a", "8" * 64), ("race-b", "9" * 64))
    ]
    destination = tmp_path / "canonical"
    shutil.copytree(incumbent.root, destination)
    barrier = threading.Barrier(2)
    results: list[tuple[int, dict[str, Any]]] = []
    failures: list[tuple[int, BaseException]] = []

    def compete(index: int) -> None:
        candidate, candidate_identity = candidates[index]
        barrier.wait()
        try:
            with _held_publication_custody(destination) as custody:
                result = publish_source_extension_candidate(
                    custody=custody,
                    destination=destination,
                    candidate_seal=candidate,
                    transaction_root=tmp_path / f"transaction-{index}",
                    expected_incumbent_identity_sha256=(
                        incumbent_identity["canonical_sha256"]
                    ),
                    expected_candidate_identity_sha256=(
                        candidate_identity["canonical_sha256"]
                    ),
                )
                results.append((index, result))
        except BaseException as exc:
            failures.append((index, exc))

    threads = [threading.Thread(target=compete, args=(index,)) for index in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=10.0)
        assert not thread.is_alive()

    assert len(results) == 1
    assert len(failures) == 1
    assert "canonical identity mismatch" in str(failures[0][1])
    winner, result = results[0]
    assert result["upgraded"] is True
    assert (destination / "source-package-seal.json").read_bytes() == (
        candidates[winner][0].root / "source-package-seal.json"
    ).read_bytes()
