"""Installed runtime cells: exact selection, receipt admission and retention."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil

import pytest

from molt import compiler_distribution as distribution
from molt._wasm_runtime_exports import (
    _is_cpython_abi_link_import,
    wasm_cpython_abi_distribution_export_names,
)
from molt.cli import (
    installed_runtime,
    native_symbol_inspection,
    runtime_build,
    runtime_callable_symbols,
    runtime_native_build,
)
from molt.cli import runtime_wasm_pair_build
from molt.cli.models import _RuntimeArtifactState
from molt.cli.native_link_manifest import native_link_dependency_manifest_path
from molt.cli.runtime_paths import _runtime_lib_archive_name
from molt.cli.runtime_wasm_build_support import (
    _wasm_runtime_codegen_flags,
    wasm_runtime_simd_enabled,
)
from molt.cli.runtime_wasm_generation import (
    bind_runtime_wasm_codegen,
    publish_runtime_wasm_generation,
)
from molt.verified_subset import current_host_coordinate
from tests.cli.native_link_test_support import (
    write_test_native_link_manifest,
    write_test_static_archive,
)
from tests.runtime_build_identity_helper import runtime_build_identity

_SOURCE_FILES = (
    "Cargo.lock",
    "Cargo.toml",
    "pyproject.toml",
    "runtime/molt-backend/Cargo.toml",
    "runtime/molt-runtime/Cargo.toml",
    "src/molt/cli/__init__.py",
    "uv.lock",
)
_GIT = {"object_format": "sha1", "commit": "a" * 40, "tree": "b" * 40}


def _digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


_PROJECTION_SYMBOLS = ("molt_len", "molt_test_intrinsic")


_CUSTODY_INPUT = b"custodied static dependency object"


def _native_cell(
    tmp_path: Path,
    *,
    projection_archive_sha256: str | None = None,
    custody_input: bool = False,
) -> tuple[dict, dict[str, Path]]:
    # The producer directory name is the Cargo profile the manifest records.
    producer = tmp_path / "producer" / "dev-fast"
    producer.mkdir(parents=True)
    archive = producer / _runtime_lib_archive_name("micro", None)
    write_test_static_archive(archive)
    native_arguments = "-lc"
    if custody_input:
        # One real absolute static input, as rustc's native-static-libs note
        # reports it; the canonical manifest writer publishes its custody.
        dependency = tmp_path / "producer" / "native-input" / "custodied_dependency.o"
        dependency.parent.mkdir(parents=True)
        dependency.write_bytes(_CUSTODY_INPUT)
        native_arguments = f'"{dependency}" -lc'
    identity = write_test_native_link_manifest(
        archive, native_arguments=native_arguments
    )
    config = identity.payload["family"]["compile"]["common_config"]
    key = {
        "target_triple": "native",
        "cargo_profile": "dev-fast",
        "stdlib_profile": "micro",
        "runtime_features": sorted(set(config["runtime_features"])),
    }
    manifest = native_link_dependency_manifest_path(archive)
    content = runtime_callable_symbols._runtime_callable_projection_content(
        _PROJECTION_SYMBOLS
    )
    projection = archive.with_name(
        runtime_callable_symbols._runtime_callable_projection_name(
            archive.name,
            archive_sha256=projection_archive_sha256 or _digest(archive.read_bytes()),
            projection_sha256=_digest(content),
        )
    )
    projection.write_bytes(content)
    members = {
        archive.name: (archive, "runtime_archive"),
        manifest.name: (manifest, "native_link_manifest"),
        projection.name: (projection, distribution.NATIVE_CALLABLE_PROJECTION_ROLE),
    }
    if custody_input:
        from molt.cli.native_link_custody import native_link_custody_archive_path

        custody = native_link_custody_archive_path(
            archive, json.loads(manifest.read_text("utf-8"))["custody"]
        )
        assert custody is not None and custody.is_file()
        members[custody.name] = (custody, "native_link_custody_archive")
    files = [
        {
            "role": role,
            "name": name,
            "sha256": _digest(path.read_bytes()),
            "size": path.stat().st_size,
        }
        for name, (path, role) in sorted(members.items())
    ]
    cell = {
        "id": distribution.runtime_cell_id(
            distribution.NATIVE_RUNTIME_CELL, key, files
        ),
        "kind": distribution.NATIVE_RUNTIME_CELL,
        "key": key,
        "files": files,
    }
    return cell, {name: path for name, (path, _role) in members.items()}


@pytest.fixture
def bundle(tmp_path: Path, monkeypatch) -> Path:
    return _installed_bundle(tmp_path, monkeypatch)


@pytest.fixture
def custody_bundle(tmp_path: Path, monkeypatch) -> Path:
    """The same release bundle, with a native cell that ships custody."""
    return _installed_bundle(tmp_path, monkeypatch, custody_input=True)


def _installed_bundle(
    tmp_path: Path, monkeypatch, *, custody_input: bool = False
) -> Path:
    monkeypatch.setenv("MOLT_HOME", str(tmp_path / "home"))
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)
    monkeypatch.delenv("MOLT_BUNDLE_ROOT", raising=False)
    root = tmp_path / "bundle"
    source = root / "source"
    records = []
    for name in _SOURCE_FILES:
        path = source / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"source")
        records.append(
            {
                "path": name,
                "mode": 0o100644,
                "blob_oid": "c" * 40,
                "size": 6,
                "sha256": _digest(b"source"),
            }
        )
    system, arch = current_host_coordinate()
    windows = system == "windows"
    binaries = {}
    for name, data in (("molt-backend", b"compiler"), ("molt", b"launcher")):
        path = root / "bin" / (name + (".exe" if windows else ""))
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        binaries[name] = path
    cell, members = _native_cell(tmp_path, custody_input=custody_input)
    for name, path in members.items():
        destination = root / distribution.RUNTIME_ROOT / cell["id"] / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
    payload = {
        "schema": distribution.MANIFEST_SCHEMA,
        "git": dict(_GIT),
        "files": records,
        "compiler": {
            "path": "bin/" + binaries["molt-backend"].name,
            "size": 8,
            "sha256": _digest(b"compiler"),
            "profile": "release",
            "features": list(distribution.PRODUCTION_COMPILER_FEATURES),
            "platform": system,
            "arch": arch,
        },
        "launcher": {
            "path": "bin/" + binaries["molt"].name,
            "size": 8,
            "sha256": _digest(b"launcher"),
            "platform": system,
            "arch": arch,
        },
        "runtime": {
            "schema": distribution.RUNTIME_INVENTORY_SCHEMA,
            "platform": system,
            "arch": arch,
            "source": dict(_GIT),
            "cells": [cell],
        },
    }
    (source / distribution.MANIFEST_NAME).write_text(json.dumps(payload), "utf-8")
    return root


def _cell(bundle: Path) -> installed_runtime.InstalledRuntimeCell:
    installed = distribution.installed_compiler(bundle / "source")
    assert installed is not None
    record = installed.runtime["cells"][0]
    return installed_runtime.select_installed_runtime_cell(
        installed, kind=record["kind"], key=record["key"]
    )


def _forbid_cargo(monkeypatch) -> None:
    def fail(*_args, **_kwargs):
        pytest.fail("installed runtime entered a source Cargo plan")

    monkeypatch.setattr(runtime_native_build, "_prepare_native_runtime_build", fail)
    monkeypatch.setattr(runtime_native_build, "resolve_runtime_cargo_plan", fail)
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_prepare_runtime_wasm_pair_build", fail
    )
    monkeypatch.setattr(runtime_build, "_ensure_runtime_wasm_both", fail)


def test_installed_native_cell_is_admitted_and_retained_outside_bundle(
    bundle, tmp_path, monkeypatch
):
    _forbid_cargo(monkeypatch)
    cell = _cell(bundle)
    admission = installed_runtime.admit_installed_native_runtime(cell)
    runtime_lib, identity = admission.runtime_lib, admission.build_identity
    assert runtime_lib == cell.runtime_lib
    assert runtime_lib.is_relative_to(tmp_path / "home" / "installed-runtime")
    shipped = bundle / distribution.RUNTIME_ROOT / cell.id / runtime_lib.name
    assert runtime_lib.read_bytes() == shipped.read_bytes()
    # The admission fences exactly the signed members it hashed.
    for role, member in admission.members():
        assert member.sha256 == cell.file_record(role)["sha256"]
    # Re-admission names the same generation; the bundle closure stays exact.
    again = installed_runtime.admit_installed_native_runtime(cell)
    assert (again.runtime_lib, again.build_identity) == (runtime_lib, identity)
    assert installed_runtime.installed_native_runtime_identity(cell, runtime_lib) == (
        identity
    )
    cell.installed.verify_runtime()


def test_receipt_identity_must_agree_with_the_cell_key(bundle):
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    cell = payload["runtime"]["cells"][0]
    # A consistent, re-addressed record whose key claims another feature set.
    old = bundle / distribution.RUNTIME_ROOT / cell["id"]
    cell["key"]["runtime_features"] = sorted(
        {*cell["key"]["runtime_features"], "stdlib_full"}
    )
    cell["id"] = distribution.runtime_cell_id(cell["kind"], cell["key"], cell["files"])
    manifest.write_text(json.dumps(payload), "utf-8")
    old.rename(bundle / distribution.RUNTIME_ROOT / cell["id"])
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="features"):
        installed_runtime.admit_installed_native_runtime(_cell(bundle))


def test_retained_generation_damage_fails_readmission(bundle):
    cell = _cell(bundle)
    runtime_lib = installed_runtime.admit_installed_native_runtime(cell).runtime_lib
    runtime_lib.write_bytes(runtime_lib.read_bytes() + b"x")
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="differs"):
        installed_runtime.installed_native_runtime_identity(cell, runtime_lib)


def test_readmission_rejects_a_different_runtime_path(bundle, tmp_path):
    cell = _cell(bundle)
    installed_runtime.admit_installed_native_runtime(cell)
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="retained"):
        installed_runtime.installed_native_runtime_identity(cell, tmp_path / "x.a")


@pytest.mark.parametrize(
    "role",
    [
        "runtime_archive",
        "native_link_manifest",
        distribution.NATIVE_CALLABLE_PROJECTION_ROLE,
    ],
)
def test_damaged_shipped_member_fails_before_retention(bundle, tmp_path, role):
    cell = _cell(bundle)
    shipped = bundle / distribution.RUNTIME_ROOT / cell.id / cell.file_name(role)
    shipped.write_bytes(shipped.read_bytes() + b" ")
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="damaged"):
        installed_runtime.admit_installed_native_runtime(cell)
    assert not (tmp_path / "home" / "installed-runtime" / cell.id).exists()


def test_unshipped_cell_fails_with_the_shipped_matrix(bundle):
    with pytest.raises(installed_runtime.InstalledRuntimeError) as error:
        installed_runtime.select_installed_native_runtime(
            bundle / "source",
            target_triple=None,
            cargo_profile="release-output",
            stdlib_profile="full",
            extra_runtime_features=(),
        )
    assert "does not ship" in str(error.value)
    assert "cargo_profile=dev-fast" in str(error.value)


def test_installed_native_build_never_plans_cargo(bundle, tmp_path, monkeypatch):
    _forbid_cargo(monkeypatch)
    state = _RuntimeArtifactState(runtime_lib=tmp_path / "unshipped.a")
    assert not runtime_native_build._ensure_runtime_lib(
        state.runtime_lib,
        None,
        True,
        "release-output",
        bundle / "source",
        None,
        stdlib_profile="full",
        runtime_state=state,
    )
    failure = state.native_runtime_build_failure
    assert failure is not None and failure.stage == "installed-runtime-selection"
    assert state.native_runtime_build_identity is None


def test_installed_wasm_build_never_plans_cargo(bundle, monkeypatch):
    _forbid_cargo(monkeypatch)
    state = _RuntimeArtifactState()
    assert not runtime_wasm_pair_build._ensure_runtime_wasm_both(
        state,
        json_output=True,
        cargo_profile="release-output",
        cargo_timeout=None,
        project_root=bundle / "source",
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        bind_for_codegen=True,
    )
    failure = state.runtime_wasm_build_failure
    assert failure is not None and failure.stage == "installed-runtime-selection"
    assert state.runtime_wasm_codegen_binding is None


def test_installed_state_has_no_ambient_wasm_coordinates(bundle, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "wasm").mkdir()
    state = runtime_build._initialize_runtime_artifact_state(
        is_rust_transpile=False,
        is_wasm=True,
        emit_mode="wasm",
        molt_root=bundle / "source",
        runtime_cargo_profile="release-output",
        target_triple=None,
        stdlib_profile="micro",
    )
    assert state.runtime_wasm is None and state.runtime_reloc_wasm is None


def test_internal_runtime_wasm_build_requires_a_source_checkout(bundle, monkeypatch):
    _forbid_cargo(monkeypatch)
    assert (
        runtime_build._prebuild_runtime_wasm(
            project_root=bundle / "source",
            kind="both",
            json_output=True,
            build_profile="release",
            cargo_timeout=None,
        )
        == 1
    )


def test_source_runtime_selector_is_rejected_for_installed_molt(bundle, monkeypatch):
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(bundle))
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="source"):
        installed_runtime.installed_runtime_active(bundle / "source")


def test_retention_store_inside_the_installation_is_rejected(bundle, monkeypatch):
    monkeypatch.setenv("MOLT_HOME", str(bundle / "state"))
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="outside"):
        installed_runtime.admit_installed_native_runtime(_cell(bundle))
    assert not (bundle / "state").exists()


def test_source_checkout_has_no_installed_cells(tmp_path):
    assert (
        installed_runtime.select_installed_native_runtime(
            tmp_path,
            target_triple=None,
            cargo_profile="dev-fast",
            stdlib_profile="micro",
            extra_runtime_features=(),
        )
        is None
    )


@pytest.mark.parametrize("damage", ["extra-file", "extra-dir", "missing"])
def test_runtime_tree_closure_is_exact(bundle, damage):
    installed = distribution.installed_compiler(bundle / "source")
    assert installed is not None
    cell_root = bundle / distribution.RUNTIME_ROOT / installed.runtime["cells"][0]["id"]
    if damage == "extra-file":
        (cell_root / "unowned.a").write_bytes(b"x")
    elif damage == "extra-dir":
        (cell_root.parent / ("f" * 64)).mkdir()
    else:
        next(cell_root.iterdir()).unlink()
    with pytest.raises(ValueError):
        installed.verify_runtime()


@pytest.mark.parametrize("field", ["id", "key", "files", "source"])
def test_manifest_rejects_noncanonical_cells(bundle, field):
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    cell = payload["runtime"]["cells"][0]
    if field == "id":
        cell["id"] = "0" * 64
    elif field == "key":
        cell["key"]["cargo_profile"] = "release-output"
    elif field == "files":
        cell["files"][0]["name"] = "../escape.a"
    else:
        # Runtime cells from another commit are not this compiler's runtime.
        payload["runtime"]["source"]["commit"] = "d" * 40
    manifest.write_text(json.dumps(payload), "utf-8")
    with pytest.raises(ValueError):
        distribution.installed_compiler(bundle / "source")


def test_manifest_rejects_duplicate_cell_keys(bundle):
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    cell = dict(payload["runtime"]["cells"][0])
    other = dict(cell, files=[dict(entry) for entry in cell["files"]])
    other["files"][0]["size"] += 1
    other["id"] = distribution.runtime_cell_id(
        other["kind"], other["key"], other["files"]
    )
    payload["runtime"]["cells"] = sorted([cell, other], key=lambda item: item["id"])
    manifest.write_text(json.dumps(payload), "utf-8")
    with pytest.raises(ValueError, match="duplicate"):
        distribution.installed_compiler(bundle / "source")


def test_wasm_receipts_must_carry_distribution_semantics(bundle):
    """A pair without SIMD/C-API distribution facts cannot satisfy any WASM key."""
    installed = distribution.installed_compiler(bundle / "source")
    assert installed is not None
    record = {
        "id": "0" * 64,
        "kind": distribution.WASM_RUNTIME_CELL,
        "key": {
            "target_triple": "wasm32-wasip1",
            "cargo_profile": "release",
            "stdlib_profile": "micro",
            "runtime_features": [],
            "simd": True,
            "freestanding": False,
        },
        "files": [],
    }
    cell = installed_runtime.InstalledRuntimeCell(
        installed, record, installed.runtime_root / record["id"]
    )
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="SIMD"):
        installed_runtime._require_wasm_semantics(
            cell, runtime_build_identity("shared")
        )


@pytest.mark.parametrize("simd", [True, False])
@pytest.mark.parametrize("freestanding", [True, False])
def test_simd_reader_agrees_with_the_codegen_flag_policy(simd, freestanding):
    for base in (
        (),
        ("-C", "target-feature=+bulk-memory"),
        ("-Ctarget-feature=+simd128",),
    ):
        flags = _wasm_runtime_codegen_flags(
            base, simd_enabled=simd, freestanding=freestanding
        )
        expected = simd if not base else "+simd128" in "".join(base)
        assert wasm_runtime_simd_enabled(flags) is expected


def test_distribution_export_surface_is_the_complete_canonical_selector():
    names = wasm_cpython_abi_distribution_export_names()
    assert names == tuple(sorted(set(names)))
    assert names and all(_is_cpython_abi_link_import(name) for name in names)
    assert not any(name.startswith("molt_") for name in names)


def test_release_policy_is_derived_from_existing_authorities(monkeypatch):
    from molt.cli.config_resolution import RUNTIME_STDLIB_PROFILE_TIERS
    from tools.release import runtime_cells

    requests = runtime_cells.declared_runtime_cells()
    kinds = {request.kind for request in requests}
    assert kinds == {distribution.NATIVE_RUNTIME_CELL, distribution.WASM_RUNTIME_CELL}
    for kind in kinds:
        subset = [request for request in requests if request.kind == kind]
        assert {request.guest_profile for request in subset} == {"dev", "release"}
        assert {request.stdlib_profile for request in subset} == set(
            RUNTIME_STDLIB_PROFILE_TIERS
        )
    assert (
        len(set(requests)) == len(requests) == 4 * 2 * len(RUNTIME_STDLIB_PROFILE_TIERS)
    )
    monkeypatch.setenv("MOLT_RUNTIME_GPU_CUDA", "1")
    with pytest.raises(ValueError, match="MOLT_RUNTIME_GPU_CUDA"):
        runtime_cells.require_release_policy_environment()


def _forbid_symbol_reader(monkeypatch) -> None:
    def fail(*_args, **_kwargs):
        pytest.fail("installed native codegen ran a symbol reader")

    monkeypatch.setattr(native_symbol_inspection, "_native_symbol_reader", fail)
    monkeypatch.setattr(native_symbol_inspection, "_nm_candidate_binaries", fail)
    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", fail
    )


def _stage_installed_codegen(bundle: Path, monkeypatch, cell, *, damage=None):
    """Bind installed codegen after the integrated retained-cell admission."""
    state = _RuntimeArtifactState(runtime_lib=cell.runtime_lib)
    admission = installed_runtime.admit_installed_native_runtime(cell)
    assert admission.runtime_lib == cell.runtime_lib
    if damage is not None:
        damage(admission.callable_projection.path)

    def ready(runtime_state, **_kwargs):
        runtime_state.installed_native_admission = admission
        runtime_state.native_runtime_build_identity = admission.build_identity
        return True

    monkeypatch.setattr(
        runtime_callable_symbols, "_ensure_native_runtime_lib_ready_for_codegen", ready
    )
    monkeypatch.setattr(
        runtime_callable_symbols,
        "select_installed_native_runtime",
        lambda *_args, **_kwargs: cell,
    )
    stage = runtime_callable_symbols._stage_runtime_callable_symbols_for_native_codegen
    return state, stage(
        state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        molt_root=bundle / "source",
        cargo_timeout=None,
        stdlib_profile="micro",
    )


def test_installed_codegen_binds_the_shipped_projection_without_a_reader(
    bundle, monkeypatch
):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(bundle)
    state, (digest, failure) = _stage_installed_codegen(bundle, monkeypatch, cell)
    assert failure is None
    binding = state.native_runtime_codegen_binding
    assert binding is not None
    record = cell.file_record(distribution.NATIVE_CALLABLE_PROJECTION_ROLE)
    assert binding.callable_symbols.path.name == record["name"]
    assert binding.callable_symbols.path.parent == cell.runtime_lib.parent
    assert binding.callable_symbols.sha256 == record["sha256"]
    assert binding.archive.sha256 == cell.file_record("runtime_archive")["sha256"]
    assert binding.archive == state.installed_native_admission.archive
    assert digest == binding.semantic_digest
    assert digest == runtime_callable_symbols._runtime_callable_symbols_digest(
        _PROJECTION_SYMBOLS
    )
    binding.verify()


def _replace_projection(path: Path) -> None:
    path.write_bytes(b"molt_hostile\n")


@pytest.mark.parametrize(
    "damage", [_replace_projection, Path.unlink], ids=["replaced", "missing"]
)
def test_installed_codegen_rejects_a_damaged_projection_without_fallback(
    bundle, monkeypatch, damage
):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    state, (digest, failure) = _stage_installed_codegen(
        bundle, monkeypatch, _cell(bundle), damage=damage
    )
    assert failure is not None and not digest
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None
    assert state.installed_native_admission is None


def test_cell_receipts_admit_the_canonical_projection(tmp_path):
    record, members = _native_cell(tmp_path)
    root = next(iter(members.values())).parent
    assert installed_runtime.admit_runtime_cell_receipts(record, root)


def test_cell_receipts_bind_the_projection_to_the_shipped_archive(tmp_path):
    record, members = _native_cell(tmp_path, projection_archive_sha256="0" * 64)
    root = next(iter(members.values())).parent
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="named"):
        installed_runtime.admit_runtime_cell_receipts(record, root)


def test_manifest_requires_the_native_callable_projection(bundle):
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    cell = payload["runtime"]["cells"][0]
    cell["files"] = [
        entry
        for entry in cell["files"]
        if entry["role"] != distribution.NATIVE_CALLABLE_PROJECTION_ROLE
    ]
    cell["id"] = distribution.runtime_cell_id(cell["kind"], cell["key"], cell["files"])
    manifest.write_text(json.dumps(payload), "utf-8")
    with pytest.raises(ValueError, match="roles"):
        distribution.installed_compiler(bundle / "source")


def test_release_native_cell_stages_the_canonical_projection(tmp_path, monkeypatch):
    from tools.release import runtime_cells

    for name in runtime_cells.POLICY_OVERRIDE_ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    built: dict[str, object] = {}

    def ensure(runtime_lib, _target, _json, cargo_profile, _root, _timeout, **kwargs):
        # The build selects a retained generation, leaving its Cargo coordinate
        # absent. Release cells must consume that selected path.
        runtime_lib = tmp_path / "retained" / cargo_profile / runtime_lib.name
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        write_test_static_archive(runtime_lib)
        identity = write_test_native_link_manifest(runtime_lib)
        kwargs["runtime_state"].runtime_lib = runtime_lib
        kwargs["runtime_state"].native_runtime_build_identity = identity
        built.update(profile=cargo_profile, identity=identity)
        return True

    def facts(_path, *, identity, **_kwargs):
        symbols = frozenset(_PROJECTION_SYMBOLS)
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        )

    def key(_request):
        identity = built["identity"]
        config = identity.payload["family"]["compile"]["common_config"]
        return {
            "target_triple": "native",
            "cargo_profile": built["profile"],
            "stdlib_profile": "micro",
            "runtime_features": sorted(set(config["runtime_features"])),
        }

    monkeypatch.setattr(runtime_native_build, "_ensure_runtime_lib", ensure)
    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", facts
    )
    monkeypatch.setattr(runtime_cells, "runtime_cell_key", key)
    (tmp_path / "source").mkdir()
    output = tmp_path / "cells"
    output.mkdir()
    record = runtime_cells._produce_native(
        runtime_cells.RuntimeCellRequest(
            distribution.NATIVE_RUNTIME_CELL, "dev", "micro"
        ),
        tmp_path / "source",
        output,
        None,
    )
    role = distribution.NATIVE_CALLABLE_PROJECTION_ROLE
    roles = {entry["role"]: entry for entry in record["files"]}
    assert set(roles) == {"runtime_archive", "native_link_manifest", role}
    cell_root = output / record["id"]
    archive = cell_root / roles["runtime_archive"]["name"]
    projection = cell_root / roles[role]["name"]
    assert projection.read_bytes() == (
        runtime_callable_symbols._runtime_callable_projection_content(
            _PROJECTION_SYMBOLS
        )
    )
    expected_name = runtime_callable_symbols._runtime_callable_projection_name(
        archive.name,
        archive_sha256=_digest(archive.read_bytes()),
        projection_sha256=_digest(projection.read_bytes()),
    )
    assert projection.name == expected_name
    assert installed_runtime.admit_runtime_cell_receipts(record, cell_root)


def test_runtime_cell_output_is_bound_before_the_private_cwd(tmp_path, monkeypatch):
    from types import SimpleNamespace

    import molt.toolchain_identity as toolchain_identity
    import molt.verified_subset as verified_subset
    from tools.release import compiler_payload, git_source_snapshot, runtime_cells

    for name in runtime_cells.POLICY_OVERRIDE_ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(
        compiler_payload, "source_snapshot", lambda *_a: SimpleNamespace(files=())
    )
    monkeypatch.setattr(compiler_payload, "source_environment", lambda: {})
    monkeypatch.setattr(
        toolchain_identity, "resolve_executable", lambda *_a, **_k: Path("git")
    )
    monkeypatch.setattr(
        git_source_snapshot,
        "materialize_git_source_snapshot",
        lambda _snapshot, root, **_kwargs: root,
    )
    monkeypatch.setattr(
        verified_subset, "current_host_coordinate", lambda: ("linux", "x86_64")
    )
    seen: list[tuple[Path, Path]] = []

    class Stop(Exception):
        pass

    def produce(_request, _source_root, output, _timeout):
        seen.append((output, Path.cwd()))
        raise Stop

    monkeypatch.setattr(runtime_cells, "_produce_native", produce)
    monkeypatch.setattr(runtime_cells, "_produce_wasm", produce)
    with pytest.raises(Stop):
        runtime_cells.produce_runtime_cells(
            tmp_path,
            Path("cells"),
            source_sha="a" * 40,
            platform="linux",
            arch="x86_64",
        )
    [(output, cwd)] = seen
    assert output.is_absolute() and output.name == "publish"
    assert output.parent.parent.resolve() == tmp_path.resolve()
    assert cwd.resolve() != tmp_path.resolve()
    assert not (tmp_path / "cells").exists()
    assert not output.parent.exists()


def _stage_native_codegen(state, bundle: Path):
    stage = runtime_callable_symbols._stage_runtime_callable_symbols_for_native_codegen
    return stage(
        state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        molt_root=bundle / "source",
        cargo_timeout=None,
        stdlib_profile="micro",
    )


def _installed_native_operation(bundle: Path, monkeypatch, cell):
    """Admit and bind codegen through the real installed native authorities."""
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(bundle.parent / "state"))
    for module in (runtime_native_build, runtime_callable_symbols):
        monkeypatch.setattr(
            module, "select_installed_native_runtime", lambda *_a, **_k: cell
        )
    admissions: list[str] = []
    admit = runtime_native_build.admit_installed_native_runtime

    def counted(selected):
        admissions.append(selected.id)
        return admit(selected)

    monkeypatch.setattr(runtime_native_build, "admit_installed_native_runtime", counted)
    state = _RuntimeArtifactState(runtime_lib=cell.runtime_lib)
    digest, failure = _stage_native_codegen(state, bundle)
    assert failure is None and digest
    return state, admissions


def _link_admission(state, bundle: Path) -> bool:
    return runtime_native_build._ensure_native_runtime_lib_ready_before_link(
        state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        molt_root=bundle / "source",
        cargo_timeout=None,
        diagnostics_enabled=False,
        phase_starts={},
        stdlib_profile="micro",
    )


def _forbid_readmission(monkeypatch) -> None:
    def fail(*_args, **_kwargs):
        pytest.fail("installed link admission re-read or re-admitted the cell")

    for module, names in (
        (
            runtime_native_build,
            (
                "admit_installed_native_runtime",
                "installed_native_runtime_identity",
                "current_native_runtime_build_identity",
                "read_native_link_dependency_manifest",
            ),
        ),
        (
            installed_runtime,
            (
                "verify_runtime_member",
                "artifact_content_identity",
                "stable_regular_file_identity",
            ),
        ),
    ):
        for name in names:
            monkeypatch.setattr(module, name, fail)


def _mutate(path: Path, mutation: str) -> None:
    """Change one file generation while restoring its size and mtime."""
    original = path.read_bytes()
    metadata = path.stat()
    if mutation == "replace":
        # Identical bytes in a new file are still not the admitted generation.
        replacement = path.with_name(path.name + ".replacement")
        replacement.write_bytes(original)
        replacement.replace(path)
    else:
        path.write_bytes(bytes([original[0] ^ 1]) + original[1:])
    os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))


def test_installed_operation_admits_once_and_links_that_generation(bundle, monkeypatch):
    from molt.cli import native_link_manifest

    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)

    def no_second_hash(*_args, **_kwargs):
        pytest.fail("installed codegen re-hashed its admitted archive")

    monkeypatch.setattr(
        native_symbol_inspection, "_native_symbol_artifact_identity", no_second_hash
    )
    cell = _cell(bundle)
    state, admissions = _installed_native_operation(bundle, monkeypatch, cell)
    assert admissions == [cell.id]
    admission = state.installed_native_admission
    binding = state.native_runtime_codegen_binding
    assert admission is not None and binding is not None
    assert binding.archive == admission.archive
    assert binding.build_identity == admission.build_identity
    _forbid_readmission(monkeypatch)
    monkeypatch.setattr(native_link_manifest, "_runtime_identity", no_second_hash)
    monkeypatch.setattr(
        native_link_manifest,
        "read_native_link_dependency_manifest_payload",
        no_second_hash,
    )
    assert _link_admission(state, bundle)
    assert state.native_runtime_codegen_binding is binding
    assert state.installed_native_admission is admission
    # Actual link flags consume the admitted semantic facts while retaining
    # the exact receipt fence and custody membership/content policy.
    list(
        native_link_manifest.read_native_link_flags(
            binding.runtime_lib,
            target_triple=None,
            object_format=native_link_manifest._object_format_for_identity(
                binding.build_identity
            ),
            runtime_build_identity=binding.build_identity,
            runtime_codegen_binding=binding,
        ).flags
    )
    binding.verify()


@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
@pytest.mark.parametrize(
    "role",
    [
        "runtime_archive",
        "native_link_manifest",
        distribution.NATIVE_CALLABLE_PROJECTION_ROLE,
    ],
)
def test_installed_link_fences_every_admitted_member(
    bundle, monkeypatch, role, mutation
):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(bundle)
    state, _admissions = _installed_native_operation(bundle, monkeypatch, cell)
    admission = state.installed_native_admission
    assert admission is not None
    # The manifest is fenced only by the admission, not by the codegen binding.
    _mutate(dict(admission.members())[role].path, mutation)
    _forbid_readmission(monkeypatch)
    assert not _link_admission(state, bundle)
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None
    assert state.installed_native_admission is None
    failure = state.native_runtime_build_failure
    assert failure is not None and failure.stage == "codegen-link-admission"


@pytest.mark.parametrize("selection", ["other-cell", "source-checkout", "unshipped"])
def test_installed_link_rejects_a_changed_selection_after_codegen(
    bundle, monkeypatch, selection
):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(bundle)
    state, _admissions = _installed_native_operation(bundle, monkeypatch, cell)

    def select(*_args, **_kwargs):
        if selection == "source-checkout":
            return None
        if selection == "unshipped":
            raise installed_runtime.InstalledRuntimeError("does not ship")
        return installed_runtime.InstalledRuntimeCell(
            cell.installed, {**cell.record, "id": "f" * 64}, cell.members_root
        )

    monkeypatch.setattr(runtime_native_build, "select_installed_native_runtime", select)
    # A changed selection neither re-admits nor falls back to a source identity.
    _forbid_readmission(monkeypatch)
    assert not _link_admission(state, bundle)
    assert state.native_runtime_codegen_binding is None
    assert state.installed_native_admission is None
    failure = state.native_runtime_build_failure
    assert failure is not None and failure.stage == "codegen-link-admission"


def test_failed_installed_link_admission_revokes_without_repair(bundle, monkeypatch):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(bundle)
    state, admissions = _installed_native_operation(bundle, monkeypatch, cell)
    runtime_lib = cell.runtime_lib
    _mutate(runtime_lib, "rewrite")
    damaged = runtime_lib.read_bytes()
    assert not _link_admission(state, bundle)
    assert state.installed_native_admission is None
    # Nothing left in this operation can authorize a link.
    assert not _link_admission(state, bundle)
    failure = state.native_runtime_build_failure
    assert failure is not None and "no admitted codegen generation" in failure.summary
    # A fresh admission fails closed on the damaged store. It neither repairs
    # nor quarantines retained bytes, and binds nothing.
    digest, stage_failure = _stage_native_codegen(state, bundle)
    assert stage_failure is not None and not digest
    assert admissions == [cell.id, cell.id]
    assert state.native_runtime_codegen_binding is None
    assert state.installed_native_admission is None
    assert runtime_lib.read_bytes() == damaged


def test_installed_codegen_requires_the_operation_admission(
    bundle, monkeypatch, capsys
):
    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(bundle)
    identity = installed_runtime.admit_installed_native_runtime(cell).build_identity

    def ready(runtime_state, **_kwargs):
        # A readiness path that records an identity but no admission.
        runtime_state.native_runtime_build_identity = identity
        return True

    monkeypatch.setattr(
        runtime_callable_symbols, "_ensure_native_runtime_lib_ready_for_codegen", ready
    )
    monkeypatch.setattr(
        runtime_callable_symbols,
        "select_installed_native_runtime",
        lambda *_args, **_kwargs: cell,
    )
    state = _RuntimeArtifactState(runtime_lib=cell.runtime_lib)
    digest, failure = _stage_native_codegen(state, bundle)
    assert failure is not None and not digest
    assert "no admitted generation" in capsys.readouterr().out
    assert state.native_runtime_codegen_binding is None


_WASM_MEMBERS = {
    "wasm_shared_member": b"shared runtime fixture",
    "wasm_reloc_member": b"reloc runtime fixture",
}


def _installed_wasm_cell(bundle: Path, cell_id: str):
    installed = distribution.installed_compiler(bundle / "source")
    assert installed is not None
    record = {
        "id": cell_id,
        "kind": distribution.WASM_RUNTIME_CELL,
        "key": {
            "target_triple": "wasm32-wasip1",
            "cargo_profile": "release",
            "stdlib_profile": "micro",
            "runtime_features": [],
            "simd": True,
            "freestanding": False,
        },
        "files": [
            {
                "role": role,
                "name": f"{role}.wasm",
                "sha256": _digest(data),
                "size": len(data),
            }
            for role, data in sorted(_WASM_MEMBERS.items())
        ],
    }
    return installed_runtime.InstalledRuntimeCell(
        installed, record, installed.runtime_root / cell_id
    )


def _bound_installed_wasm(bundle: Path, tmp_path: Path, monkeypatch):
    """Retain and bind a real generation of fixture bytes, as setup does."""
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "state"))
    cell = _installed_wasm_cell(bundle, "e" * 64)
    sources = {}
    for role, data in _WASM_MEMBERS.items():
        sources[role] = tmp_path / f"{role}.source"
        sources[role].write_bytes(data)
    root = cell.retained_root
    root.mkdir(parents=True)
    generation = publish_runtime_wasm_generation(
        root / "molt_runtime.wasm",
        root / "molt_runtime_reloc.wasm",
        shared_identity=runtime_build_identity("shared"),
        reloc_identity=runtime_build_identity("reloc"),
        source_shared=sources["wasm_shared_member"],
        source_reloc=sources["wasm_reloc_member"],
    )
    binding = bind_runtime_wasm_codegen(generation, None)
    state = _RuntimeArtifactState(
        runtime_wasm_codegen_binding=binding,
        runtime_wasm_generation=binding.generation.manifest,
    )
    return cell, state, binding


def _ensure_installed_wasm_after_codegen(state, cell, bundle: Path, **overrides):
    arguments = {
        "project_root": bundle / "source",
        "required_link_features": frozenset(),
        "required_exports": frozenset({"molt_len"}),
        "planned_exports": None,
        "bind_for_codegen": False,
        **overrides,
    }
    return runtime_wasm_pair_build._ensure_installed_runtime_wasm(
        state, cell, **arguments
    )


def _forbid_wasm_readmission(monkeypatch) -> list:
    """Forbid re-admission; record the separately tested export admission."""

    def fail(*_args, **_kwargs):
        pytest.fail("installed WASM link admission re-admitted the cell")

    monkeypatch.setattr(runtime_wasm_pair_build, "admit_installed_wasm_runtime", fail)
    monkeypatch.setattr(installed_runtime, "stable_regular_file_identity", fail)
    admitted = []

    def admits(generation, exports):
        from molt.cli.runtime_wasm_validation import RuntimeWasmAdmissionReport

        admitted.append((generation, exports))
        return RuntimeWasmAdmissionReport()

    monkeypatch.setattr(
        runtime_wasm_pair_build, "runtime_wasm_generation_admission", admits
    )
    return admitted


def test_installed_wasm_link_reuses_the_bound_generation(bundle, tmp_path, monkeypatch):
    _forbid_cargo(monkeypatch)
    cell, state, binding = _bound_installed_wasm(bundle, tmp_path, monkeypatch)
    admitted = _forbid_wasm_readmission(monkeypatch)
    assert _ensure_installed_wasm_after_codegen(state, cell, bundle)
    # Emitted imports are admitted against the exact pair codegen consumed.
    assert admitted == [(binding.generation, frozenset({"molt_len"}))]
    assert state.runtime_wasm_codegen_binding is binding
    assert state.runtime_wasm_generation == binding.generation.manifest
    assert state.runtime_wasm_selected == binding.generation.shared
    assert state.runtime_reloc_wasm_selected == binding.generation.reloc
    assert state.runtime_wasm_expected_identity is not None
    assert state.runtime_wasm_expected_identity.is_file()


@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
@pytest.mark.parametrize("member", ["manifest", "shared", "reloc"])
def test_installed_wasm_link_fences_the_bound_members(
    bundle, tmp_path, monkeypatch, member, mutation
):
    _forbid_cargo(monkeypatch)
    cell, state, binding = _bound_installed_wasm(bundle, tmp_path, monkeypatch)
    admitted = _forbid_wasm_readmission(monkeypatch)
    _mutate(getattr(binding.generation, member), mutation)
    assert not _ensure_installed_wasm_after_codegen(state, cell, bundle)
    assert admitted == []
    assert state.runtime_wasm_codegen_binding is None
    failure = state.runtime_wasm_build_failure
    assert failure is not None and failure.stage == "codegen-identity-stability"


@pytest.mark.parametrize("change", ["other-cell", "unshipped-feature"])
def test_installed_wasm_link_rejects_a_changed_selection(
    bundle, tmp_path, monkeypatch, change
):
    _forbid_cargo(monkeypatch)
    cell, state, _binding = _bound_installed_wasm(bundle, tmp_path, monkeypatch)
    admitted = _forbid_wasm_readmission(monkeypatch)
    overrides = {}
    if change == "other-cell":
        # The same signed bytes under another content address name another
        # retained generation; the bound pair is never reselected.
        cell = installed_runtime.InstalledRuntimeCell(
            cell.installed, {**cell.record, "id": "d" * 64}, cell.members_root
        )
    else:
        overrides["required_link_features"] = frozenset({"stdlib_full"})
    assert not _ensure_installed_wasm_after_codegen(state, cell, bundle, **overrides)
    assert admitted == []
    assert state.runtime_wasm_codegen_binding is None


# --- installed native custody: retained, then admitted by content at link ---


def _custodied_link_flags(binding) -> list[str]:
    """The final native-link consumer of the bound generation's receipt."""
    from molt.cli import native_link_manifest

    return list(
        native_link_manifest.read_native_link_flags(
            binding.runtime_lib,
            target_triple=None,
            object_format=native_link_manifest._object_format_for_identity(
                binding.build_identity
            ),
            runtime_build_identity=binding.build_identity,
            runtime_codegen_binding=binding,
        ).flags
    )


@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
@pytest.mark.parametrize("target", ["archive", "extracted"])
def test_installed_custody_is_retained_and_admitted_by_content_at_link(
    custody_bundle, monkeypatch, target, mutation
):
    """Retained custody is content-addressed and the final link reader owns it.

    The operation's codegen fences (archive, manifest, projection) reject even
    an identical-byte replacement. Custody is different: the final
    ``read_native_link_flags`` validates the retained custody archive and its
    extracted closure by content on every read. Identical bytes in a new file
    are therefore the same custody. Changed bytes cannot authorize a link, and
    a fresh admission does not repair them.
    """
    from molt.cli.native_link_manifest import NativeLinkDependencyManifestError

    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(custody_bundle)
    name = cell.file_name("native_link_custody_archive")
    assert name is not None
    state, admissions = _installed_native_operation(custody_bundle, monkeypatch, cell)
    assert admissions == [cell.id]
    binding = state.native_runtime_codegen_binding
    assert binding is not None
    # Retention copied the signed custody archive beside the retained archive.
    retained = cell.runtime_lib.with_name(name)
    shipped = cell.bundle_file("native_link_custody_archive").path
    assert retained.read_bytes() == shipped.read_bytes()
    assert (
        _digest(retained.read_bytes())
        == (cell.file_record("native_link_custody_archive")["sha256"])
    )
    assert not os.path.samefile(retained, shipped)
    assert _link_admission(state, custody_bundle)
    flags = _custodied_link_flags(binding)
    [extracted] = [
        Path(flag) for flag in flags if Path(flag).name == "custodied_dependency.o"
    ]
    assert extracted.is_relative_to(cell.retained_root)
    assert extracted.read_bytes() == _CUSTODY_INPUT
    damaged = retained if target == "archive" else extracted
    _mutate(damaged, mutation)
    if mutation == "replace":
        assert _custodied_link_flags(binding) == flags
        return
    changed = damaged.read_bytes()
    with pytest.raises(NativeLinkDependencyManifestError, match="native-link custody"):
        _custodied_link_flags(binding)
    with pytest.raises(installed_runtime.InstalledRuntimeError, match="custody"):
        installed_runtime.admit_installed_native_runtime(cell)
    assert damaged.read_bytes() == changed


# --- installed WASM first admission: real receipts, retention and hydration ---

# Real WASM modules: the header plus one named custom section each. Export
# admission is tested separately. These fixtures prove receipt semantics,
# retention and the retained-generation policy.
_WASM_FIXTURE_BYTES = {
    "shared": b"\x00asm\x01\x00\x00\x00\x00\x07\x06shared",
    "reloc": b"\x00asm\x01\x00\x00\x00\x00\x06\x05reloc",
}
_INSTALLED_WASM_REQUEST = {
    "cargo_profile": "release",
    "stdlib_profile": "micro",
    "simd_enabled": True,
    "freestanding": False,
}


def _distribution_wasm_identities(key, **facts):
    """Build a canonical shared/reloc receipt pair carrying a cell's distribution facts.

    The production family constructor builds the pair. ``facts`` replaces one
    receipt fact, to model a shipped cell whose receipt disagrees with its key.
    """
    from molt.cli.runtime_build_identity import (
        RuntimeBuildMemberPlan,
        _resolve_runtime_build_family_identities,
    )
    from tests.runtime_build_identity_helper import (
        runtime_toolchain_content_manifest,
    )

    family = runtime_build_identity("shared", "installed-wasm-family").to_dict()[
        "payload"
    ]["family"]
    compilation = family["compile"]
    target = facts.get("target_triple", key["target_triple"])
    config = dict(compilation["common_config"])
    config.update(
        target_triple=target,
        cargo_profile=facts.get("cargo_profile", key["cargo_profile"]),
        runtime_features=facts.get("runtime_features", list(key["runtime_features"])),
        base_rustflags=list(
            _wasm_runtime_codegen_flags(
                (),
                simd_enabled=facts.get("simd", key["simd"]),
                freestanding=key["freestanding"],
            )
        ),
    )
    config["build_script_environment"] = {
        **config["build_script_environment"],
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": list(
            facts.get("exports", wasm_cpython_abi_distribution_export_names())
        ),
    }
    members = family["members"]
    return _resolve_runtime_build_family_identities(
        sources=compilation["sources"],
        toolchain_manifest=runtime_toolchain_content_manifest(
            "installed-wasm", target_triple=target
        ),
        target_triple=target,
        common_config=config,
        publication_authority=family["publication_authority"],
        members=tuple(
            RuntimeBuildMemberPlan(
                kind=kind,
                resolved_rustflags=tuple(members[kind]["resolved_rustflags"]),
                link_args=tuple(members[kind]["link_args"]),
                publication_transform=members[kind]["publication_transform"],
                preserve_debug=members[kind]["preserve_debug"],
            )
            for kind in ("shared", "reloc")
        ),
    )


def _ship_installed_wasm_cell(bundle: Path, tmp_path: Path, **facts) -> dict:
    """Ship one WASM cell laid out as the release producer lays it out.

    The canonical generation publisher writes the content-named members and
    their receipt. The cell record, the bundle member tree and the signed
    manifest entry then carry those bytes.
    """
    from molt.wasm_artifact import inspect_wasm_binary

    key = installed_runtime.wasm_runtime_cell_key(**_INSTALLED_WASM_REQUEST)
    shared_identity, reloc_identity = _distribution_wasm_identities(key, **facts)
    producer = tmp_path / "wasm-producer"
    producer.mkdir()
    sources = {}
    for kind, data in _WASM_FIXTURE_BYTES.items():
        sources[kind] = producer / f"{kind}.source.wasm"
        sources[kind].write_bytes(data)
        assert inspect_wasm_binary(sources[kind]) == "valid"
    generation = publish_runtime_wasm_generation(
        producer / "molt_runtime.wasm",
        producer / "molt_runtime_reloc.wasm",
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=sources["shared"],
        source_reloc=sources["reloc"],
    )
    members = (
        (generation.manifest, "wasm_generation_manifest"),
        (generation.shared, "wasm_shared_member"),
        (generation.reloc, "wasm_reloc_member"),
    )
    files = sorted(
        (
            {
                "role": role,
                "name": path.name,
                "sha256": _digest(path.read_bytes()),
                "size": path.stat().st_size,
            }
            for path, role in members
        ),
        key=lambda entry: entry["name"],
    )
    record = {
        "id": distribution.runtime_cell_id(distribution.WASM_RUNTIME_CELL, key, files),
        "kind": distribution.WASM_RUNTIME_CELL,
        "key": key,
        "files": files,
    }
    cell_root = bundle / distribution.RUNTIME_ROOT / record["id"]
    cell_root.mkdir()
    for path, _role in members:
        shutil.copyfile(path, cell_root / path.name)
    manifest = bundle / "source" / distribution.MANIFEST_NAME
    payload = json.loads(manifest.read_text("utf-8"))
    payload["runtime"]["cells"] = sorted(
        [*payload["runtime"]["cells"], record], key=lambda cell: cell["id"]
    )
    manifest.write_text(json.dumps(payload), "utf-8")
    return record


def _select_installed_wasm(bundle: Path) -> installed_runtime.InstalledRuntimeCell:
    cell = installed_runtime.select_installed_wasm_runtime(
        bundle / "source", **_INSTALLED_WASM_REQUEST
    )
    assert cell is not None
    return cell


def _record_wasm_hydrations(monkeypatch) -> list[Path]:
    """Observe the real retained-pair hydration; it still runs unchanged."""
    hydrations: list[Path] = []
    hydrate = installed_runtime.hydrate_runtime_wasm_generation

    def observed(**kwargs):
        hydrations.append(kwargs["dest_shared"])
        return hydrate(**kwargs)

    monkeypatch.setattr(installed_runtime, "hydrate_runtime_wasm_generation", observed)
    return hydrations


def test_installed_wasm_first_admission_hydrates_then_reuses_the_retained_pair(
    bundle, tmp_path, monkeypatch
):
    from molt.toolchain_identity import stable_regular_file_identity
    from molt.wasm_artifact import inspect_wasm_binary

    _forbid_cargo(monkeypatch)
    record = _ship_installed_wasm_cell(bundle, tmp_path)
    cell = _select_installed_wasm(bundle)
    assert cell.id == record["id"]
    cell.installed.verify_runtime()
    hydrations = _record_wasm_hydrations(monkeypatch)
    assert not cell.retained_root.exists()
    first = installed_runtime.admit_installed_wasm_runtime(cell)
    # First admission hydrates one retained generation outside the bundle.
    assert hydrations == [cell.retained_root / "molt_runtime.wasm"]
    assert first.manifest == cell.retained_root / "molt_runtime.generation.json"
    for kind, member in (
        ("shared", first.shared_member_identity),
        ("reloc", first.reloc_member_identity),
    ):
        role = f"wasm_{kind}_member"
        entry = cell.file_record(role)
        assert member.path.parent == cell.retained_root
        assert (member.path.name, member.sha256, member.size) == (
            entry["name"],
            entry["sha256"],
            entry["size"],
        )
        assert member.path.read_bytes() == _WASM_FIXTURE_BYTES[kind]
        assert inspect_wasm_binary(member.path) == "valid"
        assert not os.path.samefile(member.path, cell.bundle_file(role).path)
    receipt = stable_regular_file_identity(first.manifest, label="retained receipt")
    # Warm admission reuses the retained generation and hydrates nothing again.
    second = installed_runtime.admit_installed_wasm_runtime(cell)
    assert len(hydrations) == 1
    assert (
        stable_regular_file_identity(first.manifest, label="retained receipt")
        == receipt
    )
    assert (second.shared_member_identity, second.reloc_member_identity) == (
        first.shared_member_identity,
        first.reloc_member_identity,
    )
    assert (second.shared_identity, second.reloc_identity) == (
        first.shared_identity,
        first.reloc_identity,
    )
    # Codegen pins this admitted pair, and the re-derived selection reuses it.
    binding = bind_runtime_wasm_codegen(second, None)
    installed_runtime.reuse_installed_wasm_generation(cell, binding.generation)
    cell.installed.verify_runtime()


_WASM_RECEIPT_DEFECTS = {
    "target": ({"target_triple": "wasm32-unknown-unknown"}, "target/profile"),
    "profile": ({"cargo_profile": "fixture-other-profile"}, "target/profile"),
    "features": (
        {"runtime_features": ["molt_fixture_unshipped_feature"]},
        "build features",
    ),
    "simd": ({"simd": False}, "SIMD"),
    "exports": ({"exports": ()}, "C-API export surface"),
}


@pytest.mark.parametrize("defect", [*_WASM_RECEIPT_DEFECTS, "required-feature"])
def test_installed_wasm_admission_requires_receipt_distribution_semantics(
    bundle, tmp_path, monkeypatch, defect
):
    _forbid_cargo(monkeypatch)
    facts, message = _WASM_RECEIPT_DEFECTS.get(
        defect, ({}, "lacks required runtime features")
    )
    record = _ship_installed_wasm_cell(bundle, tmp_path, **facts)
    cell = _select_installed_wasm(bundle)
    assert cell.id == record["id"]
    required = (
        frozenset({"molt_fixture_unshipped_feature"})
        if defect == "required-feature"
        else frozenset()
    )
    with pytest.raises(installed_runtime.InstalledRuntimeError, match=message):
        installed_runtime.admit_installed_wasm_runtime(
            cell, required_link_features=required
        )
    # Rejection precedes retention: nothing is hydrated under MOLT_HOME.
    assert not cell.retained_root.exists()


@pytest.mark.parametrize(
    "damage",
    [
        "receipt",
        "nested-receipt",
        "missing-member",
        "identical-member",
        "rewritten-member",
    ],
)
def test_installed_wasm_retained_generation_follows_the_content_addressed_rule(
    bundle, tmp_path, monkeypatch, damage
):
    """Retained WASM damage follows the current content-addressed generation rule.

    Admission reads the mutable retained receipt. If the receipt is invalid or a
    member is missing, admission rehydrates the pair from the admitted shipped
    cell. It never overwrites a content-named member whose bytes no longer match
    its name: admission fails and leaves those bytes in place. Identical bytes
    in a new file have the same content address, so a fresh admission accepts
    them. The pair already pinned for code generation rejects every member
    replacement through its file fences.
    """
    from molt.cli.runtime_wasm_generation import read_runtime_wasm_generation

    _forbid_cargo(monkeypatch)
    _ship_installed_wasm_cell(bundle, tmp_path)
    cell = _select_installed_wasm(bundle)
    first = installed_runtime.admit_installed_wasm_runtime(cell)
    binding = bind_runtime_wasm_codegen(first, None)
    hydrations = _record_wasm_hydrations(monkeypatch)
    member = first.shared
    if damage == "receipt":
        first.manifest.write_text("{}\n", encoding="utf-8")
    elif damage == "nested-receipt":
        first.manifest.write_text("[" * 100_000 + "]" * 100_000, encoding="utf-8")
    elif damage == "missing-member":
        member.unlink()
    else:
        _mutate(member, "replace" if damage == "identical-member" else "rewrite")
    if damage in {"receipt", "nested-receipt"}:
        # The pinned codegen receipt and its members are untouched.
        installed_runtime.reuse_installed_wasm_generation(cell, binding.generation)
    else:
        with pytest.raises(
            installed_runtime.InstalledRuntimeError, match="changed after admission"
        ):
            installed_runtime.reuse_installed_wasm_generation(cell, binding.generation)
    if damage == "rewritten-member":
        changed = member.read_bytes()
        with pytest.raises(installed_runtime.InstalledRuntimeError, match="corrupt"):
            installed_runtime.admit_installed_wasm_runtime(cell)
        # Rehydration was attempted and refused to replace the named member.
        assert hydrations == [cell.retained_root / "molt_runtime.wasm"]
        assert member.read_bytes() == changed
        return
    again = installed_runtime.admit_installed_wasm_runtime(cell)
    assert (again.shared, again.reloc) == (first.shared, first.reloc)
    assert again.shared.read_bytes() == _WASM_FIXTURE_BYTES["shared"]
    assert hydrations == (
        []
        if damage == "identical-member"
        else [cell.retained_root / "molt_runtime.wasm"]
    )
    assert (
        read_runtime_wasm_generation(
            first.manifest,
            expected_shared_identity=first.shared_identity,
            expected_reloc_identity=first.reloc_identity,
        )
        is not None
    )
    if damage == "identical-member":
        assert again.shared_member_identity != first.shared_member_identity


@pytest.mark.parametrize("kind", ["native", "wasm"])
def test_signed_receipt_damage_is_rejected_before_json_parsing(
    bundle, tmp_path, monkeypatch, kind
):
    if kind == "wasm":
        _ship_installed_wasm_cell(bundle, tmp_path)
        cell = _select_installed_wasm(bundle)
        role = "wasm_generation_manifest"
    else:
        cell = _cell(bundle)
        role = "native_link_manifest"
    # Capture the signed record before replacing its bytes with a parser bomb.
    # A digest mismatch must never enter the JSON parser.
    path = cell.members_root / cell.file_record(role)["name"]
    path.write_bytes(b"[" * 100_000 + b"]" * 100_000)
    parsed = []

    def parser(*args, **kwargs):
        parsed.append(True)
        pytest.fail("signed digest mismatch entered the JSON parser")

    monkeypatch.setattr(installed_runtime, "loads_exact", parser)
    with pytest.raises(
        installed_runtime.InstalledRuntimeError, match="missing or damaged"
    ):
        cell.bundle_json(role)
    assert parsed == []


@pytest.mark.parametrize("target", ["archive", "extracted"])
@pytest.mark.parametrize("cache_hit", [False, True])
def test_installed_native_final_link_rechecks_custody(
    custody_bundle, tmp_path, monkeypatch, capsys, target, cache_hit
):
    import contextlib
    import subprocess
    from molt.capability_manifest import CapabilityManifest
    from molt.cli import link_pipeline, native_link_command
    from molt.cli import link_fingerprints
    from tests.cli.native_link_test_support import write_test_static_archive

    _forbid_cargo(monkeypatch)
    _forbid_symbol_reader(monkeypatch)
    cell = _cell(custody_bundle)
    state, _ = _installed_native_operation(custody_bundle, monkeypatch, cell)
    binding = state.native_runtime_codegen_binding
    assert binding is not None
    app = tmp_path / "app.a"
    write_test_static_archive(app)
    binary = tmp_path / "app.exe"
    binary.write_bytes(b"previous user output")
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang"], None, None),
    )
    monkeypatch.setattr(link_pipeline, "native_link_cache_tool_facts", lambda _plan: [])

    def cache_match(**kwargs):
        if cache_hit:
            assert binding.custody is not None
            [extracted] = [
                path
                for path in binding.custody.paths().values()
                if path.name == "custodied_dependency.o"
            ]
            damaged = (
                cell.runtime_lib.with_name(
                    cell.file_name("native_link_custody_archive")
                )
                if target == "archive"
                else extracted
            )
            _mutate(damaged, "rewrite")
        return cache_hit

    monkeypatch.setattr(link_fingerprints, "_link_outputs_match", cache_match)
    monkeypatch.setattr(
        link_pipeline,
        "native_link_selection",
        lambda *_a, **_k: contextlib.nullcontext(None),
    )
    # The actual command builder and custody observation stay real.
    ran = []

    def link(*, link_cmd, **kwargs):
        [extracted] = [
            Path(flag)
            for flag in link_cmd
            if Path(flag).name == "custodied_dependency.o"
        ]
        damaged = (
            cell.runtime_lib.with_name(cell.file_name("native_link_custody_archive"))
            if target == "archive"
            else extracted
        )
        _mutate(damaged, "rewrite")
        candidate = Path(link_cmd[link_cmd.index("-o") + 1])
        candidate.write_bytes(b"unpublished candidate")
        ran.append(candidate)
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(link_pipeline, "_run_native_link_command", link)
    prepared, error = link_pipeline._prepare_native_link(
        output_artifact=app,
        resolved_capability_policy=CapabilityManifest().resolve(),
        artifacts_root=tmp_path,
        json_output=True,
        output_binary=binary,
        runtime_codegen_binding=binding,
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
    )
    assert len(ran) == (0 if cache_hit else 1)
    assert prepared is None and error == 2
    assert "Native runtime changed during final linking" in capsys.readouterr().out
    assert binary.read_bytes() == b"previous user output"
    assert all(not path.exists() for path in ran)


def test_cold_native_retention_reuses_staged_custody_validation(
    custody_bundle, monkeypatch
):
    from molt.cli import native_link_custody as custody

    _forbid_cargo(monkeypatch)
    cell = _cell(custody_bundle)
    validations = []
    archive_reads = []
    hashes = []
    validating = []
    validate = custody._validate_archive_file
    open_tar = custody.tarfile.open
    identity = custody._file_identity

    def scan(path, **kwargs):
        validating.append(path)
        try:
            return validate(path, **kwargs)
        finally:
            validating.pop()

    def tar(*args, **kwargs):
        if validating:
            archive_reads.append(validating[-1])
        return open_tar(*args, **kwargs)

    def hash_file(path, *args, **kwargs):
        hashes.append(path)
        return identity(path, *args, **kwargs)

    monkeypatch.setattr(custody, "_validate_archive_file", scan)
    monkeypatch.setattr(custody.tarfile, "open", tar)
    monkeypatch.setattr(custody, "_file_identity", hash_file)
    copy = installed_runtime.copy_native_link_custody_archive

    def retain(*args, **kwargs):
        observation = copy(*args, **kwargs)
        validations.append(observation)
        return observation

    monkeypatch.setattr(installed_runtime, "copy_native_link_custody_archive", retain)
    admission = installed_runtime.admit_installed_native_runtime(cell)
    assert len(validations) == 1
    staged = validations[0].archive
    assert staged is not None
    assert archive_reads.count(staged.path) == 1
    assert hashes.count(staged.path) == 1
    assert admission.custody.files
    assert all(
        member.path.is_relative_to(cell.retained_root)
        for _, member in admission.custody.files
    )
