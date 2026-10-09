from __future__ import annotations
from tests.process_guard_common import install_module_view, run_guarded_test_process

import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import threading
import time
from types import SimpleNamespace
import tomllib
import uuid
import platform

import pytest
from tests.process_guard_common import start_owned_test_process

from molt.llvm_toolchain import (
    llvm_bootstrap_command,
    llvm_release,
    load_llvm_architecture_contract,
    managed_llvm_paths,
)
from tools import bootstrap_llvm


ROOT = Path(__file__).resolve().parents[1]
CMAKE_TOOL = bootstrap_llvm._BuildTool(path="/tools/cmake", version="4.0.0")
NINJA_TOOL = bootstrap_llvm._BuildTool(path="/tools/ninja", version="1.13.0")


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
    result = run_guarded_test_process(
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
    install_module_view(
        monkeypatch, "platform", platform, bootstrap_llvm, machine=lambda: "mystery-cpu"
    )
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


@pytest.mark.usefixtures("developer_host_context")
def test_managed_paths_share_checkout_family_custody(tmp_path: Path) -> None:
    # A checkout family: the main checkout and a worktree beside it. Where CI
    # checks this repository out is not such a family.
    family = tmp_path / "Molt"
    main_checkout = family / "molt-src"
    worktree = family / "worktrees" / "feature"
    main_checkout.mkdir(parents=True)
    worktree.mkdir(parents=True)
    pin = bootstrap_llvm.required_llvm_backend_pin(ROOT)
    assert pin is not None

    paths = managed_llvm_paths(main_checkout, pin)
    worktree_paths = managed_llvm_paths(worktree, pin)

    independent = tmp_path / "independent-checkout"
    independent.mkdir()
    assert managed_llvm_paths(independent, pin) != paths
    assert paths == worktree_paths
    assert paths.root == family.resolve() / "target-root" / "toolchains"
    assert main_checkout.resolve() not in paths.prefix.parents


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


def test_native_backend_cranelift_features_cover_host_and_cross_isa_contract() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    manifest = tomllib.loads(
        (ROOT / "runtime" / "molt-backend-native" / "Cargo.toml").read_text(
            encoding="utf-8"
        )
    )
    dependency = manifest["dependencies"]["cranelift-codegen"]
    features = set(dependency["features"])
    contract_features = {
        row.cranelift_feature
        for row in contract.architectures
        if row.cranelift_feature is not None
    }
    # Cross-compilation between the two shipped primary families is available
    # on every host. `host-arch` admits the remaining contract hosts without
    # duplicating architecture tables in this manifest.
    assert features == {"arm64", "host-arch", "std", "unwind", "x86"}
    assert {"arm64", "x86"} <= contract_features
    assert all(
        "cranelift-codegen" not in table.get("dependencies", {})
        for table in manifest["target"].values()
    )


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
    assert release.minimum_cmake == "3.20.0"
    assert re.fullmatch(r"[0-9a-f]{64}", release.record_sha256)


def test_unpinned_release_requires_explicit_development_checksum() -> None:
    with pytest.raises(SystemExit, match="has no canonical source checksum"):
        bootstrap_llvm._source_sha256("99.0.0-dev", None)
    development = "a" * 64
    assert bootstrap_llvm._source_sha256("99.0.0-dev", development) == development
    with pytest.raises(SystemExit, match="cannot override"):
        bootstrap_llvm._source_sha256("22.1.8", development)


def test_cmake_selection_bypasses_incompatible_earlier_path_entry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    paths = ("C:/old/cmake.exe", "C:/current/cmake.exe")
    versions = {paths[0]: "3.18.1", paths[1]: "4.4.0"}
    monkeypatch.setattr(bootstrap_llvm, "_executable_candidates", lambda _name: paths)
    monkeypatch.setattr(
        bootstrap_llvm,
        "_tool_version",
        lambda path, *, role: versions[path],
    )

    selected = bootstrap_llvm._compatible_cmake("3.20.0")

    assert selected == bootstrap_llvm._BuildTool(paths[1], "4.4.0")


def test_cmake_selection_fails_before_source_work_with_observed_versions(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = "C:/old/cmake.exe"
    monkeypatch.setattr(bootstrap_llvm, "_executable_candidates", lambda _name: (path,))
    monkeypatch.setattr(
        bootstrap_llvm, "_tool_version", lambda _path, *, role: "3.18.1"
    )

    with pytest.raises(SystemExit, match=r"requires CMake >= 3\.20\.0.*3\.18\.1"):
        bootstrap_llvm._compatible_cmake("3.20.0")


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


def test_source_reuse_projection_detects_and_repairs_same_size_mutation(
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

    assert first["source_tree"] == second["source_tree"]
    assert first["source_contract"] == second["source_contract"]
    assert source.read_bytes() == b"project(LLVM)\n"
    assert original.st_size == source.stat().st_size
    assert source_stat.st_size == source.stat().st_size


def test_source_reuse_trusted_projection_avoids_content_rehash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
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

    def unexpected_content_hash(_destination: Path) -> dict[str, object]:
        raise AssertionError(
            "trusted unchanged source projection must avoid content rehash"
        )

    monkeypatch.setattr(
        bootstrap_llvm, "_source_tree_identity", unexpected_content_hash
    )
    second = bootstrap_llvm._safe_extract_tar_xz(
        archive,
        destination,
        archive_sha256=digest,
        source_contract={"release": "test"},
    )

    assert first == second


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
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=CMAKE_TOOL,
        ninja=NINJA_TOOL,
    )
    bootstrap_llvm._prepare_build_cache(build, first)
    stale = build / "stale-object.o"
    stale.write_text("stale", encoding="utf-8")
    second = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "d" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=CMAKE_TOOL,
        ninja=NINJA_TOOL,
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
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=CMAKE_TOOL,
        ninja=NINJA_TOOL,
    )
    assert second["digest"] != third["digest"]

    newer_cmake = bootstrap_llvm._build_cache_identity(
        release_identity={"record_sha256": "a" * 64},
        source_identity={"source_tree": {"digest": "d" * 64}},
        architecture_contract_sha256="c" * 64,
        targets="X86;WebAssembly",
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=bootstrap_llvm._BuildTool(path="/tools/cmake", version="4.1.0"),
        ninja=NINJA_TOOL,
    )
    assert second["digest"] != newer_cmake["digest"]


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
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=CMAKE_TOOL,
        ninja=NINJA_TOOL,
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
        projects="clang;lld;mlir;polly",
        build_type="Release",
        cmake=CMAKE_TOOL,
        ninja=NINJA_TOOL,
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
            start_owned_test_process(
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
                "--development-minimum-cmake",
                "3.20.0",
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
                "--development-minimum-cmake",
                "3.20.0",
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
        ("--build-type", "Debug"),
    ),
)
def test_canonical_bootstrap_requires_exact_projects_and_build_type(
    option: str, value: str
) -> None:
    with pytest.raises(
        SystemExit, match="projects expected=.*required targets=.*build type"
    ):
        bootstrap_llvm.main([option, value])


def test_arch_contract_windows_rows_are_complete() -> None:
    contract = load_llvm_architecture_contract(ROOT)
    windows_rows = [row for row in contract.architectures if row.windows_component]
    assert {row.id for row in windows_rows} == {"x86", "x86_64", "aarch64"}
    assert all(
        row.windows_target_arch and row.windows_host_arch for row in windows_rows
    )


def test_llvm_alone_requires_atl_after_shared_msvc_activation(tmp_path):
    include = tmp_path / "include"
    include.mkdir()
    env = {"INCLUDE": str(include)}
    with pytest.raises(SystemExit, match="Microsoft.VisualStudio.Component.VC.ATL"):
        bootstrap_llvm._require_windows_atl(env, tmp_path)
    (include / "atlbase.h").write_text("", encoding="utf-8")
    bootstrap_llvm._require_windows_atl(env, tmp_path)


def test_resource_preflight_rejects_insufficient_disk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        bootstrap_llvm,
        disk_usage=lambda _path: SimpleNamespace(free=10 * 1024**3),
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
    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        bootstrap_llvm,
        disk_usage=lambda _path: SimpleNamespace(free=100 * 1024**3),
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


def test_canonical_configure_failure_removes_transaction_staging(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    prefix = tmp_path / "llvm"
    build_dir = tmp_path / "build"
    source = tmp_path / "source"
    source.mkdir()

    def missing_prefix(*_args, **_kwargs):
        raise bootstrap_llvm.LlvmToolchainConfigError("missing")

    def failed_configure(*_args, **_kwargs):
        raise subprocess.CalledProcessError(1, "cmake")

    monkeypatch.setattr(bootstrap_llvm, "verify_llvm_toolchain_prefix", missing_prefix)
    monkeypatch.setattr(bootstrap_llvm, "_run", failed_configure)

    with pytest.raises(subprocess.CalledProcessError):
        bootstrap_llvm._build_and_publish(
            SimpleNamespace(
                version="22.1.8",
                build_type="Release",
                projects="clang;lld;mlir;polly",
                configure_only=False,
                jobs=1,
            ),
            prefix=prefix,
            build_dir=build_dir,
            llvm_source=source,
            targets="X86;WebAssembly",
            required_targets={"X86", "WebAssembly"},
            project_set={"clang", "lld", "mlir", "polly"},
            env={},
            is_canonical=True,
            build_identity={"schema": bootstrap_llvm.LLVM_BUILD_SCHEMA},
            cmake=CMAKE_TOOL,
            ninja=NINJA_TOOL,
        )

    assert not tuple(tmp_path.glob(".llvm.*.staging"))


def _reattest_fixture(monkeypatch, tmp_path, *, live_digest: str):
    prefix = tmp_path / "llvm"
    prefix.mkdir()
    (prefix / ".molt-llvm-toolchain.json").write_text(
        json.dumps({"content_digest": "a" * 64, "version": "22.1.8", "release": None}),
        encoding="utf-8",
    )
    verification = SimpleNamespace(
        content_digest=live_digest, version="22.1.8", release=None
    )
    written = []
    monkeypatch.setattr(
        bootstrap_llvm, "verify_llvm_toolchain_prefix", lambda *a, **k: verification
    )
    monkeypatch.setattr(
        bootstrap_llvm,
        "write_llvm_toolchain_attestation",
        lambda root, verified, **kwargs: (
            written.append((verified, kwargs)) or prefix / ".molt-llvm-toolchain.json"
        ),
    )
    return prefix, verification, written


def test_reattest_rewrites_an_unchanged_prefix(monkeypatch, tmp_path) -> None:
    prefix, verification, written = _reattest_fixture(
        monkeypatch, tmp_path, live_digest="a" * 64
    )
    bootstrap_llvm._reattest_prefix(
        prefix,
        version="22.1.8",
        expected_targets=("X86",),
        projects=("clang",),
        build_type="Release",
    )
    assert written == [
        (verification, {"projects": ("clang",), "build_type": "Release"})
    ]


def test_reattest_refuses_a_prefix_whose_content_changed(monkeypatch, tmp_path) -> None:
    prefix, _verification, written = _reattest_fixture(
        monkeypatch, tmp_path, live_digest="b" * 64
    )
    with pytest.raises(SystemExit, match="changed since its attestation"):
        bootstrap_llvm._reattest_prefix(
            prefix,
            version="22.1.8",
            expected_targets=("X86",),
            projects=("clang",),
            build_type="Release",
        )
    assert written == []


def test_managed_builds_exclude_host_optional_libraries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A Homebrew zstd found at configure time became a dynamic dependency of
    # the managed clang; the configure command pins it off.
    source = tmp_path / "source"
    source.mkdir()
    commands: list[list[str]] = []

    def missing_prefix(*_args, **_kwargs):
        raise bootstrap_llvm.LlvmToolchainConfigError("missing")

    def capture(command, *_args, **_kwargs):
        commands.append(list(command))
        raise subprocess.CalledProcessError(1, "cmake")

    monkeypatch.setattr(bootstrap_llvm, "verify_llvm_toolchain_prefix", missing_prefix)
    monkeypatch.setattr(bootstrap_llvm, "_run", capture)
    with pytest.raises(subprocess.CalledProcessError):
        bootstrap_llvm._build_and_publish(
            SimpleNamespace(
                version="22.1.8",
                build_type="Release",
                projects="clang;lld;mlir;polly",
                configure_only=True,
                jobs=1,
            ),
            prefix=tmp_path / "llvm",
            build_dir=tmp_path / "build",
            llvm_source=source,
            targets="X86;WebAssembly",
            required_targets={"X86", "WebAssembly"},
            project_set={"clang", "lld", "mlir", "polly"},
            env={},
            is_canonical=True,
            build_identity={"schema": bootstrap_llvm.LLVM_BUILD_SCHEMA},
            cmake=CMAKE_TOOL,
            ninja=NINJA_TOOL,
        )
    assert "-DLLVM_ENABLE_ZSTD=OFF" in commands[0]


@pytest.mark.parametrize("name", ["ordinary", "OneDrive - archive cache"])
def test_cached_archive_uses_bytes_not_directory_brand(tmp_path, monkeypatch, name):
    archive = tmp_path / name / "llvm.tar.xz"
    archive.parent.mkdir()
    archive.write_bytes(b"owned LLVM archive")
    monkeypatch.setattr(
        bootstrap_llvm.urllib.request,
        "urlopen",
        lambda *a, **k: pytest.fail("valid cache was downloaded"),
    )
    bootstrap_llvm._download(
        "https://example.invalid/llvm",
        archive,
        expected_sha256=hashlib.sha256(b"owned LLVM archive").hexdigest(),
        expected_size=len(b"owned LLVM archive"),
    )
    assert archive.read_bytes() == b"owned LLVM archive"
