from __future__ import annotations

import os
import hashlib
import subprocess
from collections.abc import Sequence
from typing import Any
import json
import shutil
import sys
import threading
import time
from pathlib import Path

import pytest

from molt import toolchain_identity
from molt.cli import runtime_build_identity as identity
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.exact_json import canonical_json_bytes, canonical_json_sha256
from molt.wasi_sysroot import resolve_wasi_sysroot_layout
from tests.runtime_build_identity_helper import build_python_identity_fixture
from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan
from molt.cli import runtime_cargo_plan as cargo_plans
from molt.cli import runtime_identity_schema as schema
from molt.python_runtime_identity import validate_python_runtime_identity
from tests.python_environment_test_support import runtime_identity_manifest


@pytest.fixture(scope="module")
def runtime_receipt() -> dict[str, object]:
    return runtime_identity_manifest()


def _test_plan(root: Path, env: dict[str, str], *, response_path: Path | None = None):
    link_args = (
        ("--", "-C", "link-arg=@" + str(response_path))
        if response_path is not None
        else ()
    )
    return resolve_runtime_cargo_plan(
        root,
        env=env,
        cargo_command=(
            sys.executable,
            "rustc",
            "--target",
            "wasm32-wasip1",
            *link_args,
        ),
        requested_target="wasm32-wasip1",
        host_target="test-host",
    )


def _provision_toolchain(root: Path) -> identity.RuntimeToolchainContentManifest:
    archive_root = root / "archives"
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.update(
        {
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "MOLT_BUILD_PYTHON": sys.executable,
            "CC_wasm32-wasip1": sys.executable,
            "CXX_wasm32-wasip1": sys.executable,
            "AR_wasm32-wasip1": sys.executable,
            "RANLIB_wasm32-wasip1": sys.executable,
            "CARGO_HOME": str(root / "test-cargo-home"),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    plan = _test_plan(root, env)
    return identity.provision_wasm_runtime_toolchain_content_manifest(
        project_root=root,
        env=plan.environment,
        cargo_plan=plan,
        target_triple="wasm32-wasip1",
        wasi_sysroot=root / "wasi-sysroot",
        wasm_linker=Path(sys.executable),
        long_double_archive=archive_root / "libc-printscan-long-double.a",
        builtins_archive=archive_root / "libclang_rt.builtins-wasm32.a",
        wasi_libc_archive=archive_root / "libc.a",
        rust_builtins_archive=archive_root / "libcompiler_builtins.rlib",
    )


def _resolve(
    root: Path,
    *,
    kind: str,
    publication: str,
    profile: str = "release-output",
    base_rustflags: str = "-C panic=abort",
    response_path: Path | None = None,
    extra_env: dict[str, str] | None = None,
    artifact_selection: RuntimeArtifactSelection = RUNTIME_WASM_COMBINED_ARTIFACTS,
    publication_digest: str = "test-publication-authority",
    runtime_features: tuple[str, ...] = ("stdlib_micro",),
) -> identity.RuntimeBuildIdentity:
    archive_root = root / "archives"
    sysroot = root / "wasi-sysroot"
    archives = [
        archive_root / "libc.a",
        archive_root / "libcompiler_builtins.rlib",
        archive_root / "libc-printscan-long-double.a",
        archive_root / "libclang_rt.builtins-wasm32.a",
    ]
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.update(
        {
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "MOLT_BUILD_PYTHON": sys.executable,
            "CC_wasm32-wasip1": sys.executable,
            "CXX_wasm32-wasip1": sys.executable,
            "AR_wasm32-wasip1": sys.executable,
            "RANLIB_wasm32-wasip1": sys.executable,
            "CARGO_HOME": str(root / "test-cargo-home"),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    env.update(extra_env or {})
    env["RUSTFLAGS"] = base_rustflags
    plan = _test_plan(root, env, response_path=response_path)
    shared, reloc = identity.resolve_wasm_runtime_build_family_identities(
        root,
        env=plan.environment,
        cargo_plan=plan,
        cargo_profile=profile,
        target_triple="wasm32-wasip1",
        runtime_features=runtime_features,
        base_rustflags=base_rustflags,
        cargo_command=plan.command,
        producer_artifact_selection=artifact_selection,
        publication_authority={
            "schema": "molt.runtime-wasm-publication-authority.v1",
            "digest": canonical_json_sha256(publication_digest),
            "files": [],
        },
        members=(
            identity.RuntimeBuildMemberPlan(
                kind="shared",
                resolved_rustflags=(
                    "-C panic=abort --cfg shared"
                    + (f" -C link-arg=@{response_path}" if response_path else "")
                ),
                publication_transform=(
                    publication if kind == "shared" else "strip-final-link-metadata-v1"
                ),
                preserve_debug=False,
                link_args=("--export=shared",),
            ),
            identity.RuntimeBuildMemberPlan(
                kind="reloc",
                resolved_rustflags="-C panic=abort --cfg reloc",
                publication_transform=(
                    publication
                    if kind == "reloc"
                    else "relocatable-wasm-byte-identity-v1"
                ),
                preserve_debug=False,
                link_args=("--export=reloc",),
            ),
        ),
        wasi_sysroot=sysroot,
        wasm_linker=Path(sys.executable),
        long_double_archive=archives[2],
        builtins_archive=archives[3],
        wasi_libc_archive=archives[0],
        rust_builtins_archive=archives[1],
    )
    return shared if kind == "shared" else reloc


@pytest.fixture
def identity_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setattr(cargo_plans, "_rust_resource_roots", lambda *args, **kwargs: ())
    monkeypatch.setattr(
        identity,
        "_python_identity",
        lambda _env: build_python_identity_fixture(),
    )
    (tmp_path / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits = "release"\n[profile.dev-fast]\ninherits = "dev"\n',
        encoding="utf-8",
    )
    source = tmp_path / "runtime" / "src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("pub fn runtime() {}\n", encoding="utf-8")
    sysroot = tmp_path / "wasi-sysroot"
    (sysroot / "include").mkdir(parents=True)
    (sysroot / "include" / "errno.h").write_text("#define WASI_ERRNO 1\n")
    (sysroot / "include" / "stddef.h").write_text("typedef int size_t;\n")
    (sysroot / "lib" / "wasm32-wasip1").mkdir(parents=True)
    (sysroot / "lib" / "wasm32-wasip1" / "libwasi-emulated-signal.a").write_bytes(
        b"signal"
    )
    (sysroot / "VERSION").write_text("33\n", encoding="utf-8")
    archive_root = tmp_path / "archives"
    archive_root.mkdir()
    for name in (
        "libc.a",
        "libcompiler_builtins.rlib",
        "libc-printscan-long-double.a",
        "libclang_rt.builtins-wasm32.a",
    ):
        (archive_root / name).write_bytes(name.encode("ascii"))
    monkeypatch.setattr(
        identity,
        "runtime_source_paths",
        lambda root, _features=(): (root / "runtime" / "src",),
        raising=True,
    )
    monkeypatch.setattr(
        identity,
        "_rust_toolchain_resources",
        lambda **_kwargs: {
            "host_triple": "test-host",
            "selected_target": "wasm32-wasip1",
            "content": {
                "digest": "0" * 64,
                "file_count": 0,
                "total_size": 0,
                "roots": [],
                "missing": [],
            },
        },
        raising=True,
    )
    return tmp_path


def test_identity_roundtrips_and_shared_reloc_form_one_family(
    identity_root: Path,
) -> None:
    shared = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    reloc = _resolve(
        identity_root,
        kind="reloc",
        publication="relocatable-wasm-byte-identity-v1",
    )

    assert shared.digest != reloc.digest
    assert shared.family_digest == reloc.family_digest
    assert identity.RuntimeBuildIdentity.from_dict(shared.to_dict()) == shared


def test_runtime_features_change_exact_compile_and_member_identity(
    identity_root: Path,
) -> None:
    def resolve(features: tuple[str, ...]) -> identity.RuntimeBuildIdentity:
        return _resolve(
            identity_root,
            kind="shared",
            publication="strip-final-link-metadata-v1",
            runtime_features=features,
        )

    baseline = resolve(("stdlib_micro",))
    repeated = resolve(("stdlib_micro",))
    expanded = resolve(("stdlib_micro", "molt_tk_native"))
    assert repeated == baseline
    assert expanded.compile_digest != baseline.compile_digest
    assert expanded.family_digest != baseline.family_digest
    assert expanded.digest != baseline.digest


def test_identity_observes_source_and_sysroot_mutation_without_cache(
    identity_root: Path,
) -> None:
    before = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    (identity_root / "runtime" / "src" / "lib.rs").write_text(
        "pub fn runtime_changed() {}\n", encoding="utf-8"
    )
    after_source = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    assert after_source.digest != before.digest
    assert after_source.family_digest != before.family_digest

    (identity_root / "wasi-sysroot" / "include" / "stddef.h").write_text(
        "typedef unsigned size_t;\n", encoding="utf-8"
    )
    after_header = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    assert after_header.digest != after_source.digest
    assert after_header.family_digest != after_source.family_digest


def test_canonical_profile_and_publication_transform_are_identity_inputs(
    identity_root: Path,
) -> None:
    release = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    dev = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        profile="dev-fast",
    )
    unstripped = _resolve(
        identity_root,
        kind="shared",
        publication="unstripped-debug-v1",
    )

    assert len({release.digest, dev.digest, unstripped.digest}) == 3
    assert release.family_digest != dev.family_digest
    assert release.family_digest != unstripped.family_digest


@pytest.mark.parametrize("kind", ["shared", "reloc"])
def test_dependency_profile_fields_invalidate_compile_and_member_identity(
    identity_root: Path, kind: str
) -> None:
    manifest = identity_root / "Cargo.toml"
    original = manifest.read_text(encoding="utf-8")
    identities = []
    for debug, opt in ((0, 1), (1, 1), (1, 2)):
        manifest.write_text(
            original
            + f'\n[profile.dev.package."*"]\ndebug = {debug}\n'
            + f"[profile.dev.package.cranelift-codegen]\nopt-level = {opt}\n",
            encoding="utf-8",
        )
        identities.append(
            _resolve(
                identity_root,
                kind=kind,
                publication="profile-contract",
                profile="dev-fast",
            )
        )
    assert len({item.compile_digest for item in identities}) == 3
    assert len({item.family_digest for item in identities}) == 3
    assert len({item.digest for item in identities}) == 3


def test_publication_authority_content_is_family_identity_input(
    identity_root: Path,
) -> None:
    before = _resolve(
        identity_root,
        kind="reloc",
        publication="relocatable-runtime-publication-v2",
        publication_digest="a" * 64,
    )
    after = _resolve(
        identity_root,
        kind="reloc",
        publication="relocatable-runtime-publication-v2",
        publication_digest="b" * 64,
    )

    assert before.family_digest != after.family_digest
    assert before.digest != after.digest


def test_exact_producer_artifact_selection_is_family_identity_input(
    identity_root: Path,
) -> None:
    combined = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    staticlib_only = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
    )

    assert combined.family_digest != staticlib_only.family_digest
    assert combined.compile_digest != staticlib_only.compile_digest
    assert combined.digest != staticlib_only.digest
    assert (
        combined.payload["family"]["compile"]["common_config"][
            "producer_artifact_selection"
        ]
        == RUNTIME_WASM_COMBINED_ARTIFACTS.source_identity
    )


def test_deserializer_rejects_self_asserted_digest(identity_root: Path) -> None:
    value = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    ).to_dict()
    value["digest"] = "0" * 64
    with pytest.raises(ValueError, match="digest"):
        identity.RuntimeBuildIdentity.from_dict(value)


def test_identity_json_objects_reject_non_string_key_aliasing() -> None:
    with pytest.raises(TypeError, match="keys must be strings"):
        identity._freeze_json({1: "integer", "1": "string"})

    with pytest.raises(ValueError, match="incomplete"):
        identity.RuntimeBuildIdentity.from_dict(
            {
                "schema": "molt.runtime-build-member-identity.v3",
                "digest": "0" * 64,
                "compile_digest": "0" * 64,
                "family_digest": "0" * 64,
                "payload": {1: "not-json"},
            }
        )


def test_runtime_identity_uses_shared_exact_json_authority() -> None:
    payload = {"language": "λ", "values": [1, True, None]}

    assert (
        identity._digest(payload)
        == hashlib.sha256(canonical_json_bytes(payload)).hexdigest()
    )
    assert identity._digest(payload) == canonical_json_sha256(payload)
    with pytest.raises(ValueError, match="Out of range float values"):
        identity._digest({"invalid": float("nan")})


def test_identity_rejects_digest_valid_wrong_family_schema(
    identity_root: Path,
) -> None:
    value = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    ).to_dict()
    family = value["payload"]["family"]
    family["schema"] = "molt.runtime-build-family.v2"
    value["family_digest"] = identity._digest(family)
    value["digest"] = identity._digest(value["payload"])

    with pytest.raises(ValueError, match="family shape"):
        identity.RuntimeBuildIdentity.from_dict(value)


def _reseal_runtime_build_identity(value: dict[str, object]) -> None:
    payload = value["payload"]
    family = payload["family"]
    compile_payload = family["compile"]
    compile_digest = identity._digest(compile_payload)
    family["compile_digest"] = compile_digest
    value["compile_digest"] = compile_digest
    value["family_digest"] = identity._digest(family)
    value["digest"] = identity._digest(payload)


def test_identity_rejects_digest_valid_shape_and_type_drift(
    identity_root: Path,
) -> None:
    baseline = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    ).to_dict()

    outer_extra = json.loads(json.dumps(baseline))
    outer_extra["legacy"] = True
    with pytest.raises(ValueError, match="schema"):
        identity.RuntimeBuildIdentity.from_dict(outer_extra)

    compile_extra = json.loads(json.dumps(baseline))
    compile_extra["payload"]["family"]["compile"]["legacy"] = True
    _reseal_runtime_build_identity(compile_extra)
    with pytest.raises(ValueError, match="compile/family shape"):
        identity.RuntimeBuildIdentity.from_dict(compile_extra)

    member_type = json.loads(json.dumps(baseline))
    member_type["payload"]["family"]["members"]["shared"]["preserve_debug"] = 1
    _reseal_runtime_build_identity(member_type)
    with pytest.raises(ValueError, match="member shape"):
        identity.RuntimeBuildIdentity.from_dict(member_type)

    direct_payload = member_type["payload"]
    with pytest.raises(ValueError, match="member shape"):
        identity.RuntimeBuildIdentity(
            digest=member_type["digest"],
            compile_digest=member_type["compile_digest"],
            family_digest=member_type["family_digest"],
            payload=direct_payload,
        )


def test_runtime_tooling_authority_excludes_orthogonal_cli_files(
    tmp_path: Path,
) -> None:
    authority_paths = {
        path.relative_to(tmp_path).as_posix()
        for path in identity.runtime_build_tooling_paths(tmp_path)
    }
    assert "pyproject.toml" in authority_paths
    assert "src/molt/pyproject.toml" not in authority_paths
    for relative in authority_paths:
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"# {relative}\n", encoding="utf-8")
    unrelated = tmp_path / "src/molt/cli/frontend_pipeline.py"
    unrelated.write_text("# unrelated 1\n", encoding="utf-8")

    before = identity.runtime_build_tooling_authority(tmp_path)
    unrelated.write_text("# unrelated 2\n", encoding="utf-8")
    after_unrelated = identity.runtime_build_tooling_authority(tmp_path)
    owned = tmp_path / identity._RUNTIME_BUILD_TOOLING_RELPATHS[0]
    owned.write_text("# runtime authority changed\n", encoding="utf-8")
    after_owned = identity.runtime_build_tooling_authority(tmp_path)

    assert before["schema"] == "molt.runtime-build-tooling-authority.v2"
    assert before["file_count"] == len(authority_paths)
    assert after_unrelated == before
    assert after_owned["digest"] != before["digest"]


def test_ambient_c_and_cxx_flags_are_family_identity_inputs(
    identity_root: Path,
) -> None:
    baseline = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={"CFLAGS_wasm32-wasip1": "-O1", "CXXFLAGS": "-fno-rtti"},
    )
    changed = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={"CFLAGS_wasm32-wasip1": "-O2", "CXXFLAGS": "-fno-rtti"},
    )

    assert baseline.family_digest != changed.family_digest
    assert baseline.payload["family"]["compile"]["common_config"][
        "ambient_c_build_environment"
    ] == {
        cargo_plans._CargoEnvironment({}).canonical_key("CFLAGS_wasm32-wasip1"): (
            "-O1",
        ),
        "CXXFLAGS": ("-fno-rtti",),
    }


def test_build_script_environment_is_semantic_content_identity(
    identity_root: Path,
) -> None:
    pythonpath = identity_root / "pythonpath"
    pythonpath.mkdir()
    (pythonpath / "helper.py").write_text("VERSION = 1\n", encoding="utf-8")
    relocated = identity_root / "relocated-pythonpath"
    shutil.copytree(pythonpath, relocated)
    archives = identity_root / "archives"
    common = {
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": "Py_False, PyLong_Type; Py_False",
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": "Py_False",
        "MOLT_WASM_LONGDOUBLE_ARCHIVE": str(archives / "libc-printscan-long-double.a"),
        "MOLT_WASM_BUILTINS_ARCHIVE": str(archives / "libclang_rt.builtins-wasm32.a"),
    }
    baseline = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={**common, "PYTHONPATH": str(pythonpath)},
    )
    relocated_identity = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={
            **common,
            "PYTHONPATH": str(relocated),
            "MOLT_WASM_CPYTHON_ABI_EXPORTS": "PyLong_Type\nPy_False",
        },
    )
    assert relocated_identity == baseline

    build_script = baseline.payload["family"]["compile"]["common_config"][
        "build_script_environment"
    ]
    assert build_script["MOLT_WASM_CPYTHON_ABI_EXPORTS"] == (
        "PyLong_Type",
        "Py_False",
    )
    assert build_script["MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS"] == ("Py_False",)
    assert build_script["MOLT_WASM_LONGDOUBLE_ARCHIVE"]["state"] == "resolved"
    assert build_script["MOLT_WASM_BUILTINS_ARCHIVE"]["state"] == "resolved"
    assert str(identity_root) not in json.dumps(baseline.to_dict(), sort_keys=True)

    (relocated / "helper.py").write_text("VERSION = 2\n", encoding="utf-8")
    changed_pythonpath = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={**common, "PYTHONPATH": str(relocated)},
    )
    assert changed_pythonpath.compile_digest != baseline.compile_digest

    changed_exports = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={
            **common,
            "PYTHONPATH": str(pythonpath),
            "MOLT_WASM_CPYTHON_ABI_EXPORTS": "Py_False PyLong_Type Py_True",
        },
    )
    assert changed_exports.compile_digest != baseline.compile_digest


def test_build_script_environment_rejects_invalid_or_unowned_data_exports(
    identity_root: Path,
) -> None:
    with pytest.raises(ValueError, match="invalid C symbols"):
        _resolve(
            identity_root,
            kind="shared",
            publication="strip-final-link-metadata-v1",
            extra_env={"MOLT_WASM_CPYTHON_ABI_EXPORTS": "not-a-symbol"},
        )
    with pytest.raises(ValueError, match="data exports are not a subset"):
        _resolve(
            identity_root,
            kind="shared",
            publication="strip-final-link-metadata-v1",
            extra_env={
                "MOLT_WASM_CPYTHON_ABI_EXPORTS": "Py_False",
                "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": "Py_True",
            },
        )


def test_identity_serialization_is_detached_and_rejects_nested_mutation(
    identity_root: Path,
) -> None:
    resolved = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    detached = resolved.to_dict()
    detached["payload"]["family"]["compile"]["common_config"]["cargo_profile"] = (
        "poison"
    )
    assert (
        resolved.to_dict()["payload"]["family"]["compile"]["common_config"][
            "cargo_profile"
        ]
        == "release-output"
    )

    with pytest.raises(TypeError):
        resolved.payload["family"]["compile"]["common_config"]["cargo_profile"] = (
            "poison"
        )


def test_identity_is_location_independent_including_response_files(
    identity_root: Path, tmp_path: Path
) -> None:
    relocated = tmp_path / "relocated" / "tree"
    shutil.copytree(identity_root, relocated)
    first_response = identity_root / "runtime-link.rsp"
    second_response = relocated / "runtime-link.rsp"
    first_response.write_text("--export=PyLong_Type\n", encoding="utf-8")
    second_response.write_text("--export=PyLong_Type\n", encoding="utf-8")

    first = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        response_path=first_response,
        base_rustflags=f"-C panic=abort --sysroot={identity_root / 'wasi-sysroot'}",
    )
    second = _resolve(
        relocated,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        response_path=second_response,
        base_rustflags=f"-C panic=abort --sysroot={relocated / 'wasi-sysroot'}",
    )
    serialized = json.dumps(first.to_dict(), sort_keys=True)

    assert first == second
    assert str(identity_root) not in serialized
    assert str(relocated) not in serialized
    assert r"C:\\" not in serialized
    assert "D:/" not in serialized


def test_identity_rejects_unknown_absolute_flag_path(identity_root: Path) -> None:
    with pytest.raises(
        ValueError,
        match="runtime Rust -L all resource is missing or has the wrong kind",
    ):
        _resolve(
            identity_root,
            kind="shared",
            publication="strip-final-link-metadata-v1",
            base_rustflags=r"-L D:\poison\runtime",
        )


def test_flag_canonicalization_enforces_path_boundaries(identity_root: Path) -> None:
    sysroot = identity_root / "wasi-sysroot"
    logical = (("wasi-sysroot", sysroot),)
    assert (
        identity._canonical_flag_token(
            f"-I{sysroot / 'include'}", logical_paths=logical
        )
        == "-I${wasi-sysroot}/include"
    )
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token(f"--sysroot={sysroot}bar", logical_paths=logical)
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token(f"embedded{sysroot}", logical_paths=logical)
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token("-I/opt/poison", logical_paths=logical)


def test_tool_version_banner_never_serializes_installed_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = tmp_path / "clang.exe"
    tool.write_bytes(b"MZtool-bytes")
    monkeypatch.setattr(identity, "_command_path", lambda *_args, **_kwargs: tool)
    monkeypatch.setattr(
        toolchain_identity.subprocess,
        "run",
        lambda *_args, **_kwargs: type(
            "Completed",
            (),
            {
                "returncode": 0,
                "stdout": "clang version 22.1.7\nInstalledDir: D:\\poison\\LLVM\\bin\n",
                "stderr": "",
            },
        )(),
    )

    result = identity._executable_identity("cc", str(tool), env={})

    assert result["version"] == "22.1.7"
    assert "poison" not in json.dumps(result)


def test_tree_identity_rejects_logical_label_collision(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    with pytest.raises(ValueError, match="root label collision"):
        identity._tree_identity(
            (("runtime/input", first), ("runtime/input", second)),
            require_all=True,
        )


def test_tree_identity_is_deterministic_across_parallel_completion_order(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(12):
        (source / f"{index:02}.rs").write_bytes(bytes([index]) * (index + 1))
    original = identity._hash_tree_input_file
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)
    serial = identity._tree_identity((("runtime/source", source),), require_all=True)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 4)

    def forward(file: identity._TreeInputFile) -> str:
        time.sleep((11 - int(file.path.stem)) * 0.0005)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", forward)
    first = identity._tree_identity((("runtime/source", source),), require_all=True)

    def reverse(file: identity._TreeInputFile) -> str:
        time.sleep(int(file.path.stem) * 0.0005)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", reverse)
    second = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert serial == first == second
    assert first["file_count"] == 12


def test_tree_identity_parallel_scheduler_bounds_in_flight_futures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(20):
        (source / f"{index:02}.rs").write_text(
            f"pub const VALUE_{index}: usize = {index};\n", encoding="utf-8"
        )
    pending_sizes: list[int] = []
    original_wait = identity.wait

    def observed_wait(futures: object, **kwargs: object) -> object:
        pending_sizes.append(len(futures))  # type: ignore[arg-type]
        return original_wait(futures, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 3)
    monkeypatch.setattr(identity, "wait", observed_wait)

    result = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert result["file_count"] == 20
    assert pending_sizes
    assert max(pending_sizes) == 6


def test_tree_identity_workers_are_cpu_memory_file_and_policy_bounded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, int]] = []

    def resource_ceiling(**kwargs: int) -> int:
        calls.append(kwargs)
        return 7

    monkeypatch.setattr(identity, "_memory_bounded_worker_count", resource_ceiling)

    assert identity._tree_hash_worker_count(3) == 3
    assert identity._tree_hash_worker_count(100) == 7
    assert calls == [
        {
            "bytes_per_worker": identity._TREE_HASH_BYTES_PER_WORKER,
            "headroom_bytes": identity._TREE_HASH_MEMORY_HEADROOM_BYTES,
        },
        {
            "bytes_per_worker": identity._TREE_HASH_BYTES_PER_WORKER,
            "headroom_bytes": identity._TREE_HASH_MEMORY_HEADROOM_BYTES,
        },
    ]
    monkeypatch.setattr(
        identity, "_memory_bounded_worker_count", lambda **_kwargs: 10_000
    )
    assert identity._tree_hash_worker_count(100) == identity._TREE_HASH_MAX_WORKERS


def test_tree_identity_rejects_same_size_mutation_during_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"a" * (2 * 1024 * 1024))
    before = source.stat()
    original = identity._sha256_open_file

    def mutate_after_read(handle: object) -> str:
        digest = original(handle)
        with source.open("r+b", buffering=0) as writer:
            writer.write(b"b" * before.st_size)
            os.fsync(writer.fileno())
        os.utime(
            source,
            ns=(before.st_atime_ns, before.st_mtime_ns),
        )
        return digest

    monkeypatch.setattr(identity, "_sha256_open_file", mutate_after_read)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_mutation_after_enumeration_before_open(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"before")
    original = identity._hash_tree_input_file

    def mutate_before_open(file: identity._TreeInputFile) -> str:
        before = file.path.stat()
        file.path.write_bytes(b"after!")
        os.utime(
            file.path,
            ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000),
        )
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", mutate_before_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_mutation_after_open_before_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"before")
    original = identity._sha256_open_file

    def mutate_after_open(handle: object) -> str:
        before = source.stat()
        source.write_bytes(b"after!")
        os.utime(
            source,
            ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000),
        )
        return original(handle)

    monkeypatch.setattr(identity, "_sha256_open_file", mutate_after_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_fails_closed_on_hash_io_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    victim = source / "victim.rs"
    victim.write_text("pub fn victim() {}\n", encoding="utf-8")
    original = identity._hash_tree_input_file

    def delete_before_open(file: identity._TreeInputFile) -> str:
        file.path.unlink()
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", delete_before_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(OSError, match="runtime input hashing failed.*victim.rs"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_fails_closed_on_read_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"content")

    def fail_read(_handle: object) -> str:
        raise OSError("synthetic read fault")

    monkeypatch.setattr(identity, "_sha256_open_file", fail_read)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(OSError, match="runtime input hashing failed.*read fault"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_internal_file_alias(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    target = source / "target.rs"
    target.write_text("pub fn target() {}\n", encoding="utf-8")
    linked = source / "linked.rs"
    try:
        linked.symlink_to(target)
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="path alias"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_root_alias(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "target.rs").write_text("pub fn target() {}\n", encoding="utf-8")
    linked = tmp_path / "linked"
    try:
        linked.symlink_to(source, target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks are unavailable")

    with pytest.raises(ValueError, match="root alias"):
        identity._tree_identity((("runtime/source", linked),), require_all=True)


def test_tree_identity_rejects_file_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    outside = tmp_path / "outside.rs"
    outside.write_text("pub fn poison() {}\n", encoding="utf-8")
    linked = source / "linked.rs"
    try:
        linked.symlink_to(outside)
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_directory_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "poison.rs").write_text("pub fn poison() {}\n", encoding="utf-8")
    linked = source / "linked"
    try:
        linked.symlink_to(outside, target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_broken_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    linked = source / "broken.rs"
    try:
        linked.symlink_to(tmp_path.parent / "missing" / "poison.rs")
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_distro_wasi_layout_identities_only_target_content(tmp_path: Path) -> None:
    root = tmp_path / "usr"
    host_include = root / "include"
    target_include = host_include / "wasm32-wasi"
    target_lib = root / "lib" / "wasm32-wasi"
    target_include.mkdir(parents=True)
    target_lib.mkdir(parents=True)
    (target_include / "errno.h").write_text("#define WASI_ERRNO 1\n", encoding="utf-8")
    (target_lib / "libc.a").write_bytes(b"wasi-libc")
    outside = tmp_path / "host-ncurses.h"
    outside.write_text("host-only\n", encoding="utf-8")
    try:
        (host_include / "ncurses.h").symlink_to(outside)
    except OSError:
        pytest.skip("file symlinks are unavailable")

    layout = resolve_wasi_sysroot_layout(target_include)
    assert layout is not None
    assert layout.root == root.resolve(strict=False)
    assert layout.include_roots == (
        ("include/wasm32-wasi", target_include.resolve(strict=False)),
    )
    roots = tuple((f"wasi/{label}", path) for label, path in layout.content_roots())
    before = identity._tree_identity(roots, require_all=False)
    outside.write_text("changed host-only\n", encoding="utf-8")
    assert identity._tree_identity(roots, require_all=False) == before
    (target_include / "errno.h").write_text("#define WASI_ERRNO 2\n", encoding="utf-8")
    assert (
        identity._tree_identity(roots, require_all=False)["digest"] != before["digest"]
    )


@pytest.mark.parametrize(
    "relative_sysroot",
    (Path("usr/share/wasi-sysroot"), Path("arbitrary/wasi-sysroot")),
)
def test_wasi_sysroot_layout_does_not_include_unattested_parent_version(
    tmp_path: Path,
    relative_sysroot: Path,
) -> None:
    sysroot = tmp_path / relative_sysroot
    target_include = sysroot / "include" / "wasm32-wasip1"
    target_lib = sysroot / "lib" / "wasm32-wasip1"
    target_include.mkdir(parents=True)
    target_lib.mkdir(parents=True)
    (target_include / "errno.h").write_text("#define WASI_ERRNO 1\n", encoding="utf-8")
    (target_lib / "libc.a").write_bytes(b"wasi-libc")
    sdk_root = (
        sysroot.parent.parent if sysroot.parent.name == "share" else sysroot.parent
    )
    version = sdk_root / "VERSION"
    version.write_text("33.0+m\nllvm-version: 22.1.0\n", encoding="utf-8")

    layout = resolve_wasi_sysroot_layout(sysroot)
    assert layout is not None
    assert layout.version_file is None
    roots = tuple((f"wasi/{label}", path) for label, path in layout.content_roots())
    before = identity._tree_identity(roots, require_all=False)
    version.write_text("33.0+m\nllvm-version: 22.1.1\n", encoding="utf-8")

    assert identity._tree_identity(roots, require_all=False) == before


@pytest.mark.skipif(os.name == "nt", reason="POSIX linker entrypoint symlink contract")
def test_executable_identity_preserves_role_selecting_symlink(tmp_path: Path) -> None:
    generic = tmp_path / "lld"
    generic.write_text(
        "#!/bin/sh\n"
        'case "$0" in\n'
        '  *wasm-ld) echo "LLD 22.1.8" ;;\n'
        '  *) echo "lld is a generic driver" >&2; exit 1 ;;\n'
        "esac\n",
        encoding="utf-8",
    )
    generic.chmod(0o755)
    role = tmp_path / "wasm-ld"
    role.symlink_to(generic)

    tool = identity._executable_identity(
        "wasm-ld",
        "wasm-ld",
        env={"PATH": str(tmp_path)},
    )

    assert identity._command_path("wasm-ld", {"PATH": str(tmp_path)}) == role
    assert tool["version"] == "22.1.8"


@pytest.mark.parametrize("resource", ["archive", "sysroot", "python"])
def test_portable_manifest_is_evidence_not_live_capture_authority(
    identity_root: Path, monkeypatch: pytest.MonkeyPatch, resource: str
) -> None:
    before = _resolve(
        identity_root, kind="shared", publication="strip-final-link-metadata-v1"
    )
    manifest_path = identity_root / "toolchain.json"
    before.toolchain_manifest.write(manifest_path)
    restored = identity.RuntimeToolchainContentManifest.read(manifest_path)
    assert restored == before.toolchain_manifest
    if resource == "archive":
        (identity_root / "archives" / "libc.a").write_bytes(b"changed")
    elif resource == "sysroot":
        (identity_root / "wasi-sysroot" / "include" / "errno.h").write_text(
            "#define WASI_ERRNO 2\\n", encoding="utf-8"
        )
    else:
        python = build_python_identity_fixture()
        python["selected_executable"]["sha256"] = "1" * 64
        python["identity_sha256"] = canonical_json_sha256(
            {key: value for key, value in python.items() if key != "identity_sha256"}
        )
        monkeypatch.setattr(identity, "_python_identity", lambda _env: python)
    after = _resolve(
        identity_root, kind="shared", publication="strip-final-link-metadata-v1"
    )
    assert after.toolchain_manifest != restored
    assert after.compile_digest != before.compile_digest


def test_exact_toolchain_manifest_observes_changed_archive_byte(
    identity_root: Path,
) -> None:
    before = _provision_toolchain(identity_root)
    archive = identity_root / "archives" / "libc.a"
    original = archive.read_bytes()
    archive.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))

    after = _provision_toolchain(identity_root)

    assert after.digest != before.digest


def test_toolchain_manifest_is_relocatable_and_rejects_tampering(
    identity_root: Path, tmp_path: Path
) -> None:
    relocated = tmp_path / "relocated-toolchain"
    shutil.copytree(identity_root, relocated)
    first = _provision_toolchain(identity_root)
    second = _provision_toolchain(relocated)
    assert first == second

    value = first.to_dict()
    value["payload"]["target_triple"] = "wasm32-poison"
    with pytest.raises(ValueError, match="digest"):
        identity.RuntimeToolchainContentManifest.from_dict(value)


def test_toolchain_manifest_rejects_resealed_nested_python_runtime_drift(
    identity_root: Path,
) -> None:
    value = _provision_toolchain(identity_root).to_dict()
    payload = value["payload"]
    build_python = payload["toolchain"]["tools"]["build_python"]
    runtime = build_python["runtime"]
    runtime["explicit_files"][0]["node"] = "missing-node"
    runtime_material = dict(runtime)
    runtime_material.pop("runtime_closure_sha256")
    runtime["runtime_closure_sha256"] = identity._digest(runtime_material)
    build_python_material = dict(build_python)
    build_python_material.pop("identity_sha256")
    build_python["identity_sha256"] = identity._digest(build_python_material)
    value["digest"] = identity._digest(payload)

    with pytest.raises(ValueError, match="build Python closure is invalid"):
        identity.RuntimeToolchainContentManifest.from_dict(value)


def test_toolchain_manifest_rejects_nested_mutation_at_consumption(
    identity_root: Path,
) -> None:
    manifest = _provision_toolchain(identity_root)
    with pytest.raises(TypeError):
        manifest.payload["target_triple"] = "wasm32-poison"


def test_toolchain_manifest_concurrent_publication_is_atomic(
    identity_root: Path, tmp_path: Path
) -> None:
    first = _provision_toolchain(identity_root)
    archive = identity_root / "archives" / "libc.a"
    archive.write_bytes(b"different-libc")
    second = _provision_toolchain(identity_root)
    path = tmp_path / "runtime-toolchain.json"
    barrier = threading.Barrier(2)
    errors: list[BaseException] = []

    def publish(manifest: identity.RuntimeToolchainContentManifest) -> None:
        try:
            barrier.wait()
            for _ in range(32):
                manifest.write(path)
        except BaseException as exc:
            errors.append(exc)

    threads = [
        threading.Thread(target=publish, args=(manifest,))
        for manifest in (first, second)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert errors == []
    final = identity.RuntimeToolchainContentManifest.read(path)
    assert final.digest in {first.digest, second.digest}


def test_python_identity_uses_exact_isolated_facade_in_v3_build_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, runtime_receipt: dict[str, object]
) -> None:
    executable = tmp_path / "selected-python.exe"
    executable.write_bytes(b"selected-interpreter-launcher")
    environment = {
        "MOLT_BUILD_PYTHON": str(executable),
        "PYTHON": "ignored-python",
        "PATH": "untrusted-search-path",
    }
    observed: list[tuple[tuple[str, ...], dict[str, Any]]] = []

    def run(argv: Sequence[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        observed.append((tuple(argv), kwargs))
        return subprocess.CompletedProcess(
            argv, 0, stdout=json.dumps(runtime_receipt), stderr=""
        )

    monkeypatch.setattr(identity.process_guard, "run_completed_command", run)
    result = identity._python_identity(environment)

    assert result["runtime"] == runtime_receipt
    assert (
        result["selected_executable"]["sha256"]
        == hashlib.sha256(b"selected-interpreter-launcher").hexdigest()
    )
    assert result["logical_name"] == "build_python"
    assert result["identity_sha256"] == canonical_json_sha256(
        {key: value for key, value in result.items() if key != "identity_sha256"}
    )
    assert observed == [
        (
            (
                str(executable),
                "-I",
                "-S",
                str(
                    Path(identity.__file__).resolve().parents[1]
                    / "python_environment_identity.py"
                ),
                "--capture-runtime",
                "--hash-workers",
                "4",
            ),
            {
                "check": False,
                "capture_output": True,
                "text": True,
                "encoding": "utf-8",
                "env": environment,
                "timeout": 30,
                "memory_guard_prefix": None,
            },
        )
    ]
    assert schema._SCHEMA == "molt.runtime-build-member-identity.v3"
    assert schema._TOOLCHAIN_MANIFEST_SCHEMA == "molt.runtime-toolchain-content.v3"


@pytest.mark.parametrize(
    "corruption", ["malformed", "duplicate-root", "duplicate-node", "nonfinite"]
)
def test_python_identity_rejects_ambiguous_probe_json(
    monkeypatch: pytest.MonkeyPatch, runtime_receipt: dict[str, object], corruption: str
) -> None:
    output = json.dumps(runtime_receipt)
    if corruption == "malformed":
        output = "{not-json"
    elif corruption == "duplicate-root":
        output = '{"schema":"discarded-duplicate",' + output[1:]
    elif corruption == "duplicate-node":
        assert '"size": 1' in output
        output = output.replace('"size": 1', '"size": 1, "size": 1', 1)
    else:
        output = "NaN"
    monkeypatch.setattr(identity, "_command_path", lambda *_args: Path(sys.executable))
    monkeypatch.setattr(
        identity.process_guard,
        "run_completed_command",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 0, stdout=output, stderr=""
        ),
    )
    with pytest.raises(ValueError, match="probe emitted invalid JSON"):
        identity._python_identity({})


def test_python_identity_validates_receipt_after_successful_probe(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(identity, "_command_path", lambda *_args: Path(sys.executable))
    monkeypatch.setattr(
        identity.process_guard,
        "run_completed_command",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 0, stdout="{}", stderr=""
        ),
    )
    with pytest.raises(ValueError, match="runtime closure shape is invalid"):
        identity._python_identity({})


@pytest.mark.parametrize(
    "stdout,stderr,detail",
    [
        ("ignored stdout", " loader closure missing \n", "loader closure missing"),
        (" unresolved import \n", "", "unresolved import"),
        ("", "", "probe failed"),
    ],
)
def test_python_identity_preserves_probe_failure_signal(
    monkeypatch: pytest.MonkeyPatch, stdout: str, stderr: str, detail: str
) -> None:
    monkeypatch.setattr(identity, "_command_path", lambda *_args: Path(sys.executable))
    monkeypatch.setattr(
        identity.process_guard,
        "run_completed_command",
        lambda argv, **_kwargs: subprocess.CompletedProcess(
            argv, 7, stdout=stdout, stderr=stderr
        ),
    )
    with pytest.raises(ValueError, match=detail):
        identity._python_identity({})


def test_python_identity_preserves_timeout_without_retry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    timeout = subprocess.TimeoutExpired([sys.executable, "--capture-runtime"], 30)
    calls = 0

    def run(_argv: Sequence[str], **_kwargs: Any) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        calls += 1
        raise timeout

    monkeypatch.setattr(identity, "_command_path", lambda *_args: Path(sys.executable))
    monkeypatch.setattr(identity.process_guard, "run_completed_command", run)
    with pytest.raises(subprocess.TimeoutExpired) as failure:
        identity._python_identity({})
    assert failure.value is timeout
    assert calls == 1


def test_python_identity_unresolved_interpreter_never_spawns_probe(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(identity, "_command_path", lambda *_args: None)
    monkeypatch.setattr(
        identity.process_guard,
        "run_completed_command",
        lambda *_args, **_kwargs: pytest.fail(
            "unresolved interpreter must not execute"
        ),
    )
    with pytest.raises(ValueError, match="build Python is unresolved"):
        identity._python_identity({})


@pytest.mark.slow
def test_python_identity_live_content_probe() -> None:
    """Native integration owns the actual isolated scanner/consumer boundary."""
    result = identity._python_identity(
        {**os.environ, "MOLT_BUILD_PYTHON": sys.executable}
    )
    runtime = validate_python_runtime_identity(result["runtime"])
    assert runtime["file_nodes"]
    assert runtime["native_dependency_closure"]["status"] == "closed"
    assert result["selected_executable"][
        "sha256"
    ] == toolchain_identity.stable_file_sha256(
        Path(sys.executable), label="test Python executable"
    )


@pytest.mark.parametrize("mutation", ["rewrite", "replace", "restore-bytes"])
def test_python_identity_binds_launcher_generation_across_probe(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    runtime_receipt: dict[str, object],
    mutation: str,
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(b"before")
    before = executable.stat()

    def run(argv, **_kwargs):
        if mutation == "replace":
            replacement = tmp_path / "replacement.exe"
            replacement.write_bytes(b"before")
            replacement.replace(executable)
        else:
            executable.write_bytes(b"after!")
            if mutation == "restore-bytes":
                executable.write_bytes(b"before")
            os.utime(executable, ns=(before.st_atime_ns, before.st_mtime_ns))
        return subprocess.CompletedProcess(
            argv, 0, stdout=json.dumps(runtime_receipt), stderr=""
        )

    monkeypatch.setattr(identity.process_guard, "run_completed_command", run)
    with pytest.raises(ValueError, match="changed"):
        identity._python_identity({"MOLT_BUILD_PYTHON": str(executable)})
