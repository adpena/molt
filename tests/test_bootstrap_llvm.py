from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import threading
import time
from types import SimpleNamespace
import tomllib
import uuid

import pytest

from molt.llvm_toolchain import (
    llvm_bootstrap_command,
    llvm_release,
    load_llvm_architecture_contract,
    managed_llvm_paths,
)
from tools import bootstrap_llvm


ROOT = Path(__file__).resolve().parents[1]


def _unique_publication_staging(destination: Path) -> Path:
    return bootstrap_llvm._publication_staging(destination, uuid.uuid4().hex)


@pytest.mark.parametrize(
    "command",
    [
        [sys.executable, str(ROOT / "tools" / "bootstrap_llvm.py"), "--help"],
        [sys.executable, "-m", "tools.bootstrap_llvm", "--help"],
    ],
)
def test_bootstrap_entry_paths_share_module_safe_authority(command: list[str]) -> None:
    result = subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "Build and install a complete LLVM dev prefix for Molt." in result.stdout


def test_bootstrap_command_projects_the_module_authority() -> None:
    pin = bootstrap_llvm.required_llvm_backend_pin(ROOT)
    assert pin is not None
    command = llvm_bootstrap_command(pin, python="py")
    assert command == f"py -m tools.bootstrap_llvm --version {pin.default_release}"


def test_default_llvm_targets_follow_host_architecture() -> None:
    assert bootstrap_llvm._default_llvm_targets("AMD64") == "X86;WebAssembly"
    assert bootstrap_llvm._default_llvm_targets("x86_64") == "X86;WebAssembly"
    assert bootstrap_llvm._default_llvm_targets("ARM64") == "AArch64;WebAssembly"
    assert bootstrap_llvm._default_llvm_targets("aarch64") == "AArch64;WebAssembly"


@pytest.mark.parametrize(
    ("machine", "target"),
    [
        ("i686", "X86"),
        ("armv7l", "ARM"),
        ("riscv64", "RISCV"),
        ("ppc64le", "PowerPC"),
        ("s390x", "SystemZ"),
        ("loongarch64", "LoongArch"),
        ("mips64el", "Mips"),
        ("sparc64", "Sparc"),
    ],
)
def test_default_llvm_targets_cover_supported_host_families(
    machine: str, target: str
) -> None:
    assert bootstrap_llvm._default_llvm_targets(machine) == f"{target};WebAssembly"


def test_default_llvm_targets_fail_closed_for_unknown_architecture() -> None:
    with pytest.raises(SystemExit, match="unsupported LLVM host architecture"):
        bootstrap_llvm._default_llvm_targets("mystery-cpu")


def test_explicit_targets_parse_before_unknown_host_default(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(bootstrap_llvm.platform, "machine", lambda: "mystery-cpu")
    monkeypatch.setattr(
        bootstrap_llvm,
        "verify_llvm_toolchain_prefix",
        lambda *_args, **_kwargs: SimpleNamespace(
            llvm_config=tmp_path / "llvm-config",
            prefix=tmp_path,
            version="22.1.8",
        ),
    )
    monkeypatch.setattr(
        bootstrap_llvm,
        "project_llvm_toolchain_environment",
        lambda *_args, **_kwargs: {
            "MOLT_LLVM_PREFIX": str(tmp_path),
            "LLVM_SYS_221_PREFIX": str(tmp_path),
            "MLIR_SYS_220_PREFIX": str(tmp_path),
            "TABLEGEN_220_PREFIX": str(tmp_path),
            "LLVM_CONFIG_PATH": str(tmp_path / "llvm-config"),
        },
    )

    assert (
        bootstrap_llvm.main(
            ["--check", "--prefix", str(tmp_path), "--targets", "WebAssembly"]
        )
        == 0
    )


def test_managed_paths_share_checkout_family_custody() -> None:
    pin = bootstrap_llvm.required_llvm_backend_pin(ROOT)
    assert pin is not None
    paths = managed_llvm_paths(ROOT, pin)
    worktree_paths = managed_llvm_paths(
        ROOT.parent / "independent-checkout",
        pin,
    )

    assert paths == worktree_paths
    assert paths.root.name == "toolchains"
    assert paths.root.parent.name == "target-root"
    assert ROOT not in paths.prefix.parents


def test_native_backend_inkwell_mapping_matches_arch_contract_exactly() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    manifest = tomllib.loads(
        (ROOT / "runtime" / "molt-backend-native" / "Cargo.toml").read_text(
            encoding="utf-8"
        )
    )
    actual: dict[str, str] = {}
    for cfg_key, target_table in manifest["target"].items():
        features = (
            target_table.get("dependencies", {}).get("inkwell", {}).get("features", [])
        )
        target_features = [
            feature for feature in features if feature.startswith("target-")
        ]
        assert len(target_features) == 1, cfg_key
        actual[cfg_key] = target_features[0]

    expected = {
        f"cfg({row.rust_cfg})": row.inkwell_feature for row in contract.architectures
    }
    assert actual == expected


def test_native_backend_cranelift_mapping_matches_arch_contract_exactly() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    manifest = tomllib.loads(
        (ROOT / "runtime" / "molt-backend-native" / "Cargo.toml").read_text(
            encoding="utf-8"
        )
    )
    actual: dict[str, str] = {}
    for cfg_key, target_table in manifest["target"].items():
        features = (
            target_table.get("dependencies", {})
            .get("cranelift-codegen", {})
            .get("features", [])
        )
        backend_features = [
            feature
            for feature in features
            if feature in {"x86", "arm64", "riscv64", "s390x"}
        ]
        if backend_features:
            assert len(backend_features) == 1, cfg_key
            actual[cfg_key] = backend_features[0]

    expected = {
        f"cfg({row.rust_cfg})": row.cranelift_feature
        for row in contract.architectures
        if row.cranelift_feature is not None
    }
    assert actual == expected


def test_native_backend_fails_closed_outside_cranelift_contract() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    source = (ROOT / "runtime" / "molt-backend-native" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )
    guard = source.split("compile_error!", maxsplit=1)[0]
    actual = set(re.findall(r'target_arch = "([^"]+)"', guard))
    expected = {
        target_arch
        for row in contract.architectures
        if row.cranelift_feature is not None
        for target_arch in re.findall(r'target_arch = "([^"]+)"', row.rust_cfg)
    }
    assert actual == expected


def test_cranelift_contract_names_supported_upstream_architecture_arms() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    actual = {
        row.id: (row.cranelift_architecture, row.cranelift_feature)
        for row in contract.architectures
        if row.cranelift_architecture is not None
    }
    assert actual == {
        "x86_64": ("X86_64", "x86"),
        "aarch64": ("Aarch64", "arm64"),
        "riscv64": ("Riscv64", "riscv64"),
        "systemz": ("S390x", "s390x"),
    }
    assert (
        next(row for row in contract.architectures if row.id == "x86").cranelift_feature
        is None
    )


def test_release_source_checksum_is_pinned_to_official_llvm_provenance() -> None:
    assert bootstrap_llvm._release_source_sha256("22.1.8") == (
        "922f1817a0df7b1489272d18134ee0087a8b068828f87ac63b9861b1a9965888"
    )
    assert bootstrap_llvm._release_source_sha256("99.0.0-dev") is None
    release = llvm_release("22.1.8", ROOT)
    assert release is not None
    assert release.url.endswith("/llvm-project-22.1.8.src.tar.xz")
    assert release.size == 167061596
    assert release.provenance_url.endswith("/releases/tags/llvmorg-22.1.8")
    assert re.fullmatch(r"[0-9a-f]{64}", release.record_sha256)


def test_unpinned_release_requires_explicit_development_checksum() -> None:
    with pytest.raises(SystemExit, match="has no canonical source checksum"):
        bootstrap_llvm._source_sha256("99.0.0-dev", None)
    development = "a" * 64
    assert bootstrap_llvm._source_sha256("99.0.0-dev", development) == development
    with pytest.raises(SystemExit, match="cannot override"):
        bootstrap_llvm._source_sha256("22.1.8", development)


def test_download_replaces_corrupt_cache_atomically(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive = tmp_path / "llvm.tar.xz"
    archive.write_bytes(b"corrupt")
    payload = b"verified llvm source"
    digest = hashlib.sha256(payload).hexdigest()
    monkeypatch.setattr(
        bootstrap_llvm.urllib.request,
        "urlopen",
        lambda _url: io.BytesIO(payload),
    )

    bootstrap_llvm._download(
        "https://llvm.example/source", archive, expected_sha256=digest
    )

    assert archive.read_bytes() == payload
    assert not tuple(tmp_path.glob("*.partial"))


def test_download_rejects_corrupt_response_without_publishing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive = tmp_path / "llvm.tar.xz"
    monkeypatch.setattr(
        bootstrap_llvm.urllib.request,
        "urlopen",
        lambda _url: io.BytesIO(b"corrupt"),
    )

    with pytest.raises(SystemExit, match="checksum mismatch"):
        bootstrap_llvm._download(
            "https://llvm.example/source",
            archive,
            expected_sha256=hashlib.sha256(b"expected").hexdigest(),
        )

    assert not archive.exists()
    assert not tuple(tmp_path.glob("*.partial"))


def _write_test_tar(archive: Path, *, unsafe_link: bool = False) -> str:
    with tarfile.open(archive, "w:xz") as bundle:
        payload = b"project(LLVM)\n"
        source = tarfile.TarInfo("llvm-project-llvmorg-test/llvm/CMakeLists.txt")
        source.size = len(payload)
        bundle.addfile(source, io.BytesIO(payload))
        if unsafe_link:
            link = tarfile.TarInfo("llvm-project-llvmorg-test/llvm/escape")
            link.type = tarfile.SYMTYPE
            link.linkname = "../../../../outside"
            bundle.addfile(link)
    return hashlib.sha256(archive.read_bytes()).hexdigest()


def test_source_path_never_authorizes_unattested_partial_tree_reset(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    destination.mkdir()
    (destination / "partial.txt").write_text("partial", encoding="utf-8")

    with pytest.raises(SystemExit, match="unattested LLVM source"):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
        )

    assert (destination / "partial.txt").is_file()
    assert not (destination / bootstrap_llvm.LLVM_SOURCE_MARKER).exists()


def test_extraction_rejects_escaping_link(tmp_path: Path) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive, unsafe_link=True)
    destination = tmp_path / "source"

    with pytest.raises((SystemExit, tarfile.FilterError)):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
        )

    assert not destination.exists()


def test_source_reuse_rehashes_tree_and_repairs_same_size_mutation(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    first = bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"release": "test"},
    )
    source = destination / "llvm-project-llvmorg-test" / "llvm" / "CMakeLists.txt"
    original = source.stat()
    source.write_bytes(b"project(XYZZ)\n")
    source.touch()
    source_stat = source.stat()
    source.touch()
    # Restore the archive's source through a second extraction even when the
    # marker still names the same archive and contract.
    second = bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"release": "test"},
    )

    assert first == second
    assert source.read_bytes() == b"project(LLVM)\n"
    assert original.st_size == source.stat().st_size
    assert source_stat.st_size == source.stat().st_size


@pytest.mark.skipif(
    os.name != "nt", reason="NTFS ChangeTime policy is Windows-specific"
)
def test_source_hash_repeats_when_ntfs_change_time_is_unavailable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.cpp"
    source.write_text("int value;\n", encoding="utf-8")
    calls = 0
    real_sha256 = bootstrap_llvm._sha256

    def counted(path: Path) -> str:
        nonlocal calls
        calls += 1
        return real_sha256(path)

    monkeypatch.setattr(bootstrap_llvm, "_windows_change_time_ns", lambda _path: None)
    monkeypatch.setattr(bootstrap_llvm, "_sha256", counted)

    bootstrap_llvm._stable_file_sha256(source)
    assert calls == 2


def test_source_contract_change_invalidates_extracted_tree(tmp_path: Path) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"record_sha256": "a" * 64},
    )
    marker = destination / bootstrap_llvm.LLVM_SOURCE_MARKER
    first = json.loads(marker.read_text(encoding="utf-8"))
    bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"record_sha256": "b" * 64},
    )
    second = json.loads(marker.read_text(encoding="utf-8"))

    assert first["source_tree"] == second["source_tree"]
    assert second["source_contract"]["record_sha256"] == "b" * 64


@pytest.mark.parametrize(
    "phase", ["prepared", "old-renamed", "old-moved", "new-renamed", "new-moved"]
)
def test_extracted_source_publication_recovers_through_canonical_transaction(
    tmp_path: Path, phase: str
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"record_sha256": "a" * 64},
    )

    with pytest.raises(bootstrap_llvm._SimulatedPublicationCrash):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
            source_contract={"record_sha256": "b" * 64},
            simulate_publication_crash_after=phase,
        )
    bootstrap_llvm._recover_publication(destination)
    recovered = json.loads(
        (destination / bootstrap_llvm.LLVM_SOURCE_MARKER).read_text(encoding="utf-8")
    )
    assert recovered["source_contract"]["record_sha256"] == "a" * 64

    published = bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"record_sha256": "b" * 64},
    )
    assert published["source_contract"]["record_sha256"] == "b" * 64
    assert not bootstrap_llvm._publication_journal(destination).exists()


def test_extracted_source_recovers_before_rejecting_corrupt_archive(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"record_sha256": "a" * 64},
    )
    with pytest.raises(bootstrap_llvm._SimulatedPublicationCrash):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
            source_contract={"record_sha256": "b" * 64},
            simulate_publication_crash_after="new-renamed",
        )
    archive.write_bytes(b"corrupt")

    with pytest.raises(SystemExit, match="archive changed before extraction"):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
            source_contract={"record_sha256": "b" * 64},
        )
    recovered = json.loads(
        (destination / bootstrap_llvm.LLVM_SOURCE_MARKER).read_text(encoding="utf-8")
    )
    assert recovered["source_contract"]["record_sha256"] == "a" * 64


def test_development_source_refuses_unattested_directory_replacement(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    destination.mkdir()
    (destination / "owned-by-user").write_text("preserve", encoding="utf-8")

    with pytest.raises(SystemExit, match="unattested LLVM source"):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
        )
    assert (destination / "owned-by-user").is_file()


def test_development_source_refuses_corrupt_marker_as_deletion_authority(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    destination.mkdir()
    (destination / bootstrap_llvm.LLVM_SOURCE_MARKER).write_text("{}", encoding="utf-8")
    owned = destination / "owned-by-user"
    owned.write_text("preserve", encoding="utf-8")

    with pytest.raises(SystemExit, match="unattested LLVM source"):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
        )
    assert owned.is_file()


def test_development_source_refuses_forged_marker_as_deletion_authority(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "source.tar.xz"
    digest = _write_test_tar(archive)
    destination = tmp_path / "source"
    destination.mkdir()
    (destination / bootstrap_llvm.LLVM_SOURCE_MARKER).write_text(
        json.dumps(
            {
                "schema": bootstrap_llvm.LLVM_SOURCE_SCHEMA,
                "archive_sha256": digest,
                "source_contract": {"record_sha256": "forged"},
                "source_tree": {
                    "digest": "0" * 64,
                    "file_count": 1,
                    "total_bytes": 1,
                },
            }
        ),
        encoding="utf-8",
    )
    owned = destination / "owned-by-user"
    owned.write_text("preserve", encoding="utf-8")

    with pytest.raises(SystemExit, match="unattested LLVM source"):
        bootstrap_llvm._safe_extract_tar_xz(
            archive,
            destination,
            archive_sha256=digest,
            source_contract={"record_sha256": "new"},
        )
    assert owned.is_file()


def test_build_cache_is_bound_to_source_release_and_config(tmp_path: Path) -> None:
    build = tmp_path / "build"
    first = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "b" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir",
        build_type="Release",
    )
    bootstrap_llvm._prepare_build_cache(build, first)
    stale = build / "stale-object.o"
    stale.write_text("stale", encoding="utf-8")
    second = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "d" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir",
        build_type="Release",
    )
    bootstrap_llvm._prepare_build_cache(build, second)

    assert first["digest"] != second["digest"]
    assert re.fullmatch(r"[0-9a-f]{64}", str(second["inputs"]["config_digest"]))
    assert not stale.exists()
    assert (
        json.loads(
            (build / bootstrap_llvm.LLVM_BUILD_MARKER).read_text(encoding="utf-8")
        )
        == second
    )

    third = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "d" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="AArch64;WebAssembly",
        projects="clang;lld;mlir",
        build_type="Release",
    )
    assert second["digest"] != third["digest"]


def test_development_build_refuses_unattested_directory_deletion(
    tmp_path: Path,
) -> None:
    build = tmp_path / "build"
    build.mkdir()
    owned = build / "owned-by-user"
    owned.write_text("preserve", encoding="utf-8")
    identity = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "b" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir",
        build_type="Release",
    )

    with pytest.raises(SystemExit, match="unattested LLVM build"):
        bootstrap_llvm._prepare_build_cache(build, identity)
    assert owned.is_file()


def test_development_build_refuses_forged_marker_as_deletion_authority(
    tmp_path: Path,
) -> None:
    build = tmp_path / "build"
    build.mkdir()
    owned = build / "owned-by-user"
    owned.write_text("preserve", encoding="utf-8")
    (build / bootstrap_llvm.LLVM_BUILD_MARKER).write_text(
        json.dumps(
            {
                "schema": bootstrap_llvm.LLVM_BUILD_SCHEMA,
                "digest": "0" * 64,
                "inputs": {"forged": True},
            }
        ),
        encoding="utf-8",
    )
    identity = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "b" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir",
        build_type="Release",
    )

    with pytest.raises(SystemExit, match="unattested LLVM build"):
        bootstrap_llvm._prepare_build_cache(build, identity)
    assert owned.is_file()


def test_bootstrap_authority_topology_rejects_nested_destructive_roots(
    tmp_path: Path,
) -> None:
    prefix = tmp_path / "llvm"
    with pytest.raises(SystemExit, match="must be disjoint"):
        bootstrap_llvm._validate_bootstrap_path_topology(
            prefix=prefix,
            archive=tmp_path / "source.tar.xz",
            source_root=prefix / "source",
            build_dir=tmp_path / "build",
        )


def test_failed_staged_publication_restores_last_known_good(tmp_path: Path) -> None:
    destination = tmp_path / "llvm"
    staging = _unique_publication_staging(destination)
    destination.mkdir()
    staging.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    (staging / "identity").write_text("new", encoding="utf-8")

    def reject(_path: Path) -> None:
        raise RuntimeError("invalid staged prefix")

    with pytest.raises(RuntimeError, match="invalid staged prefix"):
        bootstrap_llvm._publish_staged_prefix(
            staging,
            destination,
            validate=reject,
        )

    assert (destination / "identity").read_text(encoding="utf-8") == "old"
    assert not staging.exists()
    assert not tuple(tmp_path.glob("*.rollback"))


def test_successful_staged_publication_prunes_rollback(tmp_path: Path) -> None:
    destination = tmp_path / "llvm"
    staging = _unique_publication_staging(destination)
    destination.mkdir()
    staging.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    (staging / "identity").write_text("new", encoding="utf-8")

    bootstrap_llvm._publish_staged_prefix(
        staging,
        destination,
        validate=lambda path: (path / "identity").read_text(encoding="utf-8"),
    )

    assert (destination / "identity").read_text(encoding="utf-8") == "new"
    assert not staging.exists()
    assert not tuple(tmp_path.glob("*.rollback"))


@pytest.mark.parametrize(
    "phase", ["prepared", "old-renamed", "old-moved", "new-renamed", "new-moved"]
)
def test_publication_startup_recovery_rolls_back_every_crash_phase(
    tmp_path: Path, phase: str
) -> None:
    destination = tmp_path / "llvm"
    staging = _unique_publication_staging(destination)
    destination.mkdir()
    staging.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    (staging / "identity").write_text("new", encoding="utf-8")

    with pytest.raises(bootstrap_llvm._SimulatedPublicationCrash):
        bootstrap_llvm._publish_staged_prefix(
            staging,
            destination,
            validate=lambda _path: None,
            simulate_crash_after=phase,
        )
    bootstrap_llvm._recover_publication(destination)

    assert (destination / "identity").read_text(encoding="utf-8") == "old"
    assert not bootstrap_llvm._publication_journal(destination).exists()
    assert not tuple(tmp_path.glob("*.rollback"))


def test_publication_startup_recovery_keeps_durably_validated_prefix(
    tmp_path: Path,
) -> None:
    destination = tmp_path / "llvm"
    staging = _unique_publication_staging(destination)
    destination.mkdir()
    staging.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    (staging / "identity").write_text("new", encoding="utf-8")

    with pytest.raises(bootstrap_llvm._SimulatedPublicationCrash):
        bootstrap_llvm._publish_staged_prefix(
            staging,
            destination,
            validate=lambda _path: None,
            simulate_crash_after="validated",
        )
    bootstrap_llvm._recover_publication(destination)

    assert (destination / "identity").read_text(encoding="utf-8") == "new"
    assert not bootstrap_llvm._publication_journal(destination).exists()
    assert not tuple(tmp_path.glob("*.rollback"))


@pytest.mark.parametrize("phase", ["prepared", "old-moved", "new-renamed", "new-moved"])
def test_fresh_publication_recovery_never_admits_unvalidated_prefix(
    tmp_path: Path, phase: str
) -> None:
    destination = tmp_path / "llvm"
    staging = _unique_publication_staging(destination)
    staging.mkdir()
    (staging / "identity").write_text("new", encoding="utf-8")

    with pytest.raises(bootstrap_llvm._SimulatedPublicationCrash):
        bootstrap_llvm._publish_staged_prefix(
            staging,
            destination,
            validate=lambda _path: None,
            simulate_crash_after=phase,
        )
    bootstrap_llvm._recover_publication(destination)

    assert not destination.exists()
    assert not bootstrap_llvm._publication_journal(destination).exists()


def test_publication_recovery_rejects_same_parent_cleanup_path_forgery(
    tmp_path: Path,
) -> None:
    destination = tmp_path / "llvm"
    destination.mkdir()
    protected = tmp_path / "protected"
    protected.mkdir()
    transaction = uuid.uuid4().hex
    bootstrap_llvm._atomic_json(
        bootstrap_llvm._publication_journal(destination),
        {
            "schema": bootstrap_llvm.LLVM_PUBLICATION_SCHEMA,
            "transaction": transaction,
            "destination": str(destination.resolve()),
            "staging": str(protected.resolve()),
            "backup": str(
                bootstrap_llvm._publication_backup(destination, transaction).resolve()
            ),
            "phase": "prepared",
        },
    )

    with pytest.raises(SystemExit, match="do not match its transaction"):
        bootstrap_llvm._recover_publication(destination)
    assert protected.is_dir()


def test_publication_lock_serializes_concurrent_publishers(tmp_path: Path) -> None:
    destination = tmp_path / "llvm"
    destination.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    active = 0
    peak = 0
    state_lock = threading.Lock()
    entered = threading.Event()
    errors: list[BaseException] = []

    def publish(label: str) -> None:
        nonlocal active, peak
        staging = _unique_publication_staging(destination)
        staging.mkdir()
        (staging / "identity").write_text(label, encoding="utf-8")

        def validate(_path: Path) -> None:
            nonlocal active, peak
            with state_lock:
                active += 1
                peak = max(peak, active)
                entered.set()
            time.sleep(0.05)
            with state_lock:
                active -= 1

        try:
            bootstrap_llvm._publish_staged_prefix(
                staging, destination, validate=validate
            )
        except BaseException as exc:  # pragma: no cover - asserted below
            errors.append(exc)

    first = threading.Thread(target=publish, args=("first",))
    second = threading.Thread(target=publish, args=("second",))
    first.start()
    assert entered.wait(timeout=2)
    second.start()
    first.join(timeout=3)
    second.join(timeout=3)

    assert errors == []
    assert peak == 1
    assert (destination / "identity").read_text(encoding="utf-8") == "second"
    assert not bootstrap_llvm._publication_journal(destination).exists()


def test_publication_lock_serializes_cross_process_publishers(tmp_path: Path) -> None:
    destination = tmp_path / "llvm"
    destination.mkdir()
    (destination / "identity").write_text("old", encoding="utf-8")
    events = tmp_path / "events.jsonl"
    worker = """
import json
import os
from pathlib import Path
import time
from tools import bootstrap_llvm

destination = Path(os.environ["MOLT_TEST_DESTINATION"])
staging = Path(os.environ["MOLT_TEST_STAGING"])
events = Path(os.environ["MOLT_TEST_EVENTS"])
label = os.environ["MOLT_TEST_LABEL"]
staging.mkdir()
(staging / "identity").write_text(label, encoding="utf-8")

def record(kind):
    row = json.dumps({"label": label, "kind": kind, "time": time.monotonic_ns()}) + "\\n"
    with events.open("a", encoding="utf-8") as handle:
        handle.write(row)
        handle.flush()
        os.fsync(handle.fileno())

def validate(_path):
    record("enter")
    time.sleep(0.15)
    record("exit")

bootstrap_llvm._publish_staged_prefix(staging, destination, validate=validate)
"""

    processes: list[subprocess.Popen[str]] = []
    for label in ("first", "second"):
        staging = _unique_publication_staging(destination)
        env = os.environ.copy()
        env.update(
            {
                "MOLT_TEST_DESTINATION": str(destination),
                "MOLT_TEST_STAGING": str(staging),
                "MOLT_TEST_EVENTS": str(events),
                "MOLT_TEST_LABEL": label,
            }
        )
        processes.append(
            subprocess.Popen(
                [sys.executable, "-c", worker],
                cwd=ROOT,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        )
    failures = []
    for process in processes:
        stdout, stderr = process.communicate(timeout=10)
        if process.returncode != 0:
            failures.append((process.returncode, stdout, stderr))

    assert failures == []
    rows = [
        json.loads(line) for line in events.read_text(encoding="utf-8").splitlines()
    ]
    assert [row["kind"] for row in rows] == ["enter", "exit", "enter", "exit"]
    assert (destination / "identity").read_text(encoding="utf-8") in {"first", "second"}
    assert not bootstrap_llvm._publication_journal(destination).exists()


def test_development_release_requires_explicit_noncanonical_custody(
    tmp_path: Path,
) -> None:
    with pytest.raises(SystemExit, match="explicit noncanonical --prefix"):
        bootstrap_llvm.main(["--version", "99.0.0-dev"])
    with pytest.raises(SystemExit, match="development-source-url"):
        bootstrap_llvm.main(
            [
                "--version",
                "99.0.0-dev",
                "--prefix",
                str(tmp_path / "llvm-dev"),
                "--development-source-sha256",
                "a" * 64,
            ]
        )

    canonical = managed_llvm_paths(ROOT).prefix
    with pytest.raises(SystemExit, match="disjoint from canonical managed custody"):
        bootstrap_llvm.main(
            [
                "--version",
                "99.0.0-dev",
                "--prefix",
                str(canonical),
                "--development-source-url",
                "https://llvm.example/development.tar.xz",
                "--development-source-sha256",
                "a" * 64,
            ]
        )


def test_development_paths_are_derived_from_explicit_noncanonical_prefix(
    tmp_path: Path,
) -> None:
    prefix = tmp_path / "llvm-dev"
    paths = bootstrap_llvm._development_llvm_paths(prefix, "99.0.0-dev")
    assert paths.prefix == prefix
    assert paths.root == tmp_path / ".llvm-dev.development-custody"
    assert paths.archive.is_relative_to(paths.root)
    assert paths.source_root.is_relative_to(paths.root)
    assert paths.build_dir.is_relative_to(paths.root)


@pytest.mark.parametrize(
    ("option", "value"),
    (
        ("--projects", "clang;lld;mlir;bolt"),
        ("--targets", f"{bootstrap_llvm._default_llvm_targets()};BPF"),
        ("--build-type", "Debug"),
    ),
)
def test_canonical_bootstrap_requires_exact_manifest_build_configuration(
    option: str, value: str
) -> None:
    with pytest.raises(
        SystemExit, match="projects expected=.*targets expected=.*build type"
    ):
        bootstrap_llvm.main([option, value])


def test_bootstrap_rejects_d_drive_for_every_explicit_custody_path() -> None:
    for poisoned in (r"D:\poison", r"d:/poison", r"\\?\D:\poison"):
        for option in ("--prefix", "--archive", "--source-root", "--build-dir"):
            with pytest.raises(Exception, match="retired D: canonical custody"):
                bootstrap_llvm.main([option, poisoned])


def test_bootstrap_rejects_d_drive_for_every_prefix_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pin = bootstrap_llvm.required_llvm_backend_pin(ROOT)
    assert pin is not None
    for name in (
        "MOLT_TARGET_ROOT",
        "MOLT_LLVM_PREFIX",
        pin.env_var,
        f"MLIR_SYS_{pin.major * 10}_PREFIX",
        f"TABLEGEN_{pin.major * 10}_PREFIX",
        "LLVM_CONFIG_PATH",
    ):
        monkeypatch.setenv(name, r"\\?\D:\poison")
        with pytest.raises(Exception, match="retired D: canonical custody"):
            bootstrap_llvm.main(["--check"])
        monkeypatch.delenv(name)


def test_arch_contract_windows_rows_are_complete() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    windows_rows = [row for row in contract.architectures if row.windows_component]
    assert {row.id for row in windows_rows} == {"x86", "x86_64", "aarch64"}
    assert all(
        row.windows_target_arch and row.windows_host_arch for row in windows_rows
    )


def test_windows_arm64_activation_uses_contract_arches(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    vsdevcmd = tmp_path / "Common7" / "Tools" / "VsDevCmd.bat"
    vsdevcmd.parent.mkdir(parents=True)
    vsdevcmd.write_text("", encoding="utf-8")
    observed: list[str] = []

    def run(command, **_kwargs):
        observed.extend(command)
        return SimpleNamespace(
            returncode=0,
            stdout=(
                "PATH=activated\nVSCMD_ARG_TGT_ARCH=arm64\nVSCMD_ARG_HOST_ARCH=arm64\n"
            ),
            stderr="",
        )

    monkeypatch.setattr(bootstrap_llvm.platform, "system", lambda: "Windows")
    monkeypatch.setattr(
        bootstrap_llvm, "_visual_studio_installation", lambda _component: tmp_path
    )
    monkeypatch.setattr(bootstrap_llvm.subprocess, "run", run)
    monkeypatch.setattr(
        bootstrap_llvm.shutil,
        "which",
        lambda name, path=None: (
            "cl.exe" if name == "cl" and path == "activated" else None
        ),
    )

    env = bootstrap_llvm._windows_msvc_env({"PATH": "base"}, machine="ARM64")

    assert env["VSCMD_ARG_TGT_ARCH"] == "arm64"
    assert env["VSCMD_ARG_HOST_ARCH"] == "arm64"
    assert any("-arch=arm64 -host_arch=arm64" in part for part in observed)


def test_resource_preflight_rejects_insufficient_disk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        bootstrap_llvm.shutil,
        "disk_usage",
        lambda _path: SimpleNamespace(free=10 * 1024**3),
    )

    with pytest.raises(SystemExit, match="only 10.0 GiB is available"):
        bootstrap_llvm._preflight_resources(
            tmp_path,
            required_free_gb=40.0,
            required_memory_gb=8.0,
        )


def test_resource_preflight_rejects_insufficient_memory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        bootstrap_llvm.shutil,
        "disk_usage",
        lambda _path: SimpleNamespace(free=100 * 1024**3),
    )
    monkeypatch.setattr(
        bootstrap_llvm,
        "plan_resource_pressure",
        lambda **_kwargs: SimpleNamespace(available_gb=4.0, physical_gb=4.0),
    )

    with pytest.raises(SystemExit, match="reports 4.0 GiB"):
        bootstrap_llvm._preflight_resources(
            tmp_path,
            required_free_gb=40.0,
            required_memory_gb=8.0,
        )
