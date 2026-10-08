from __future__ import annotations

import os
import hashlib
import json
import shutil
import sys
import threading
import time
from pathlib import Path

import pytest

from molt import toolchain_identity
from molt.cli import runtime_build_identity as identity
from molt.cli import runtime_native_build
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.exact_json import canonical_json_bytes, canonical_json_sha256
from molt.wasi_sysroot import resolve_wasi_sysroot_layout
from tests.operation_probe import same_thread_probe
from tests.runtime_build_identity_helper import build_python_identity_fixture
from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan
from molt.cli import runtime_cargo_plan as cargo_plans
from molt.cli import runtime_identity_schema as schema
from molt.python_runtime_identity import validate_python_runtime_identity
from tests.python_environment_test_support import runtime_identity_manifest
from tests.executable_test_support import (
    build_native_executable,
    native_executable_name,
    write_mock_executable,
)
from tests.rustc_test_support import rustc_target_metadata_output


# Fake host triple shared by every resolved test plan and its environment.
_TEST_HOST_TARGET = "test-host"


@pytest.fixture(scope="module")
def runtime_receipt() -> dict[str, object]:
    return runtime_identity_manifest()


def _test_plan(
    root: Path,
    env: dict[str, str],
    *,
    response_path: Path | None = None,
    target_triple: str | None = "wasm32-wasip1",
):
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
            *(("--target", target_triple) if target_triple else ()),
            *link_args,
        ),
        requested_target=target_triple,
        host_target=_TEST_HOST_TARGET,
    )


def _provision_toolchain(root: Path) -> schema.RuntimeToolchainContentManifest:
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
    runtime_features: tuple[str, ...] = ("stdlib_micro",),
) -> schema.RuntimeBuildIdentity:
    archive_root = root / "archives"
    sysroot = root / "wasi-sysroot"
    archives = [
        archive_root / "libc.a",
        archive_root / "libcompiler_builtins.rlib",
        archive_root / "libc-printscan-long-double.a",
        archive_root / "libclang_rt.builtins-wasm32.a",
    ]
    target = None if kind == "native" else "wasm32-wasip1"
    ambient = cargo_plans._CargoEnvironment(os.environ)
    # Ambient cc-rs inputs (the RunContext exports CFLAGS_<wasm target>, and
    # Xcode shells export SDKROOT) would enter every identity resolved here.
    # Each test supplies the exact C environment it asserts through extra_env.
    for name in (
        *cargo_plans.runtime_c_flag_environment_names(
            target or _TEST_HOST_TARGET, _TEST_HOST_TARGET
        ),
        *cargo_plans._C_SEARCH_ENVIRONMENTS,
    ):
        ambient.pop(name, None)
    env = dict(ambient)
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
    if kind == "native":
        env.update(
            {
                f"{name}_{_TEST_HOST_TARGET}": sys.executable
                for name in ("CC", "CXX", "AR", "RANLIB")
            }
        )
    env.update(extra_env or {})
    env["RUSTFLAGS"] = base_rustflags
    plan = _test_plan(root, env, response_path=response_path, target_triple=target)
    if kind == "native":
        return runtime_native_build._runtime_build_identity_for_plan(
            root,
            env=plan.environment,
            cargo_plan=plan,
            cargo_profile=profile,
            target_triple=target,
            runtime_features=runtime_features,
            cargo_command=plan.command,
        )
    if kind == "cpython-abi":
        return identity.resolve_wasm_cpython_abi_build_identity(
            root,
            env=plan.environment,
            cargo_plan=plan,
            cargo_profile=profile,
            target_triple="wasm32-wasip1",
            rustflags=base_rustflags,
            cargo_command=plan.command,
            artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
            wasi_sysroot=sysroot,
        )
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
        members=(
            schema.RuntimeBuildMemberPlan(
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
            schema.RuntimeBuildMemberPlan(
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
    from molt.cli.cargo_target_cfg import parse_rustc_target_metadata

    def target_metadata(_rustc, _target, _flags, **_kwargs):
        stdout, stderr = rustc_target_metadata_output(
            tmp_path / "rust-sysroot", "unix", target=_target
        )
        return parse_rustc_target_metadata(stdout, stderr)

    monkeypatch.setattr(cargo_plans, "_rust_target_metadata", target_metadata)
    monkeypatch.setattr(cargo_plans, "_rust_resource_roots", lambda *args, **kwargs: ())
    monkeypatch.setattr(
        identity,
        "_python_identity",
        lambda _env, **_kwargs: build_python_identity_fixture(),
    )
    (tmp_path / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits = "release"\n[profile.dev-fast]\ninherits = "dev"\n',
        encoding="utf-8",
    )
    source = tmp_path / "runtime" / "src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("pub fn runtime() {}\n", encoding="utf-8")
    tooling = tmp_path / "runtime-planner.py"
    tooling.write_text("# runtime planning authority\n", encoding="utf-8")
    monkeypatch.setattr(
        identity,
        "runtime_build_tooling_paths",
        lambda root: (root / "runtime-planner.py",),
    )
    sysroot = tmp_path / "wasi-sysroot"
    (sysroot / "include").mkdir(parents=True)
    (sysroot / "include" / "errno.h").write_text(
        "#define WASI_ERRNO 1\n", encoding="utf-8"
    )
    (sysroot / "include" / "stddef.h").write_text(
        "typedef int size_t;\n", encoding="utf-8"
    )
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
        "_cargo_crate_source_closure",
        lambda *, project_root, **_kwargs: (project_root / "runtime" / "src",),
    )
    monkeypatch.setattr(
        cargo_plans.RuntimeCargoPlan,
        "rust_resource_identity",
        lambda cargo_plan: {
            "host_triple": _TEST_HOST_TARGET,
            "selected_target": cargo_plan.target,
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


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "cpython-abi"])
def test_resolvers_own_one_source_and_tooling_observation_per_call(
    identity_root: Path, monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    observations: list[tuple[str, ...]] = []
    real_capture = identity.RuntimeTreeIndex.capture

    def capture(cls, roots):
        labels = tuple(label for label, _path in roots)
        if any(label.startswith(("source/", "runtime-tooling/")) for label in labels):
            observations.append(labels)
        return real_capture(roots)

    monkeypatch.setattr(identity.RuntimeTreeIndex, "capture", classmethod(capture))

    def resolve():
        return _resolve(identity_root, kind=kind, publication="tree-observation")

    before = resolve()
    assert observations == [
        ("source/runtime/src", "runtime-tooling/runtime-planner.py")
    ]
    # A separate live admission must see files introduced after the first one.
    (identity_root / "runtime/src/new.rs").write_bytes(b"new source input")
    after_source = resolve()
    assert len(observations) == 2
    assert before.compile_digest != after_source.compile_digest
    assert (
        before.payload["family"]["publication_authority"]
        == after_source.payload["family"]["publication_authority"]
    )
    (identity_root / "runtime-planner.py").write_bytes(b"new publication input")
    after_tooling = resolve()
    assert len(observations) == 3
    assert after_source.compile_digest == after_tooling.compile_digest
    assert after_source.family_digest != after_tooling.family_digest
    assert observations == [observations[0]] * 3


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
    assert schema.RuntimeBuildIdentity.from_dict(shared.to_dict()) == shared


def test_runtime_features_change_exact_compile_and_member_identity(
    identity_root: Path,
) -> None:
    def resolve(features: tuple[str, ...]) -> schema.RuntimeBuildIdentity:
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
    )
    (identity_root / "runtime-planner.py").write_text(
        "# changed runtime planning authority\n", encoding="utf-8"
    )
    after = _resolve(
        identity_root,
        kind="reloc",
        publication="relocatable-runtime-publication-v2",
    )

    assert before.compile_digest == after.compile_digest
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
        schema.RuntimeBuildIdentity.from_dict(value)


def test_identity_json_objects_reject_non_string_key_aliasing() -> None:
    with pytest.raises(TypeError, match="keys must be strings"):
        schema._freeze_json({1: "integer", "1": "string"})

    with pytest.raises(ValueError, match="incomplete"):
        schema.RuntimeBuildIdentity.from_dict(
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
        schema._digest(payload)
        == hashlib.sha256(canonical_json_bytes(payload)).hexdigest()
    )
    assert schema._digest(payload) == canonical_json_sha256(payload)
    with pytest.raises(ValueError, match="non-finite JSON number"):
        schema._digest({"invalid": float("nan")})


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
    value["family_digest"] = schema._digest(family)
    value["digest"] = schema._digest(value["payload"])

    with pytest.raises(ValueError, match="family shape"):
        schema.RuntimeBuildIdentity.from_dict(value)


def _reseal_runtime_build_identity(value: dict[str, object]) -> None:
    payload = value["payload"]
    family = payload["family"]
    compile_payload = family["compile"]
    compile_digest = schema._digest(compile_payload)
    family["compile_digest"] = compile_digest
    value["compile_digest"] = compile_digest
    value["family_digest"] = schema._digest(family)
    value["digest"] = schema._digest(payload)


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
        schema.RuntimeBuildIdentity.from_dict(outer_extra)

    compile_extra = json.loads(json.dumps(baseline))
    compile_extra["payload"]["family"]["compile"]["legacy"] = True
    _reseal_runtime_build_identity(compile_extra)
    with pytest.raises(ValueError, match="compile/family shape"):
        schema.RuntimeBuildIdentity.from_dict(compile_extra)

    member_type = json.loads(json.dumps(baseline))
    member_type["payload"]["family"]["members"]["shared"]["preserve_debug"] = 1
    _reseal_runtime_build_identity(member_type)
    with pytest.raises(ValueError, match="member shape"):
        schema.RuntimeBuildIdentity.from_dict(member_type)

    direct_payload = member_type["payload"]
    with pytest.raises(ValueError, match="member shape"):
        schema.RuntimeBuildIdentity(
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

    _, before = identity._capture_runtime_build_trees(tmp_path, ())
    unrelated.write_text("# unrelated 2\n", encoding="utf-8")
    _, after_unrelated = identity._capture_runtime_build_trees(tmp_path, ())
    owned = tmp_path / identity._RUNTIME_BUILD_TOOLING_RELPATHS[0]
    owned.write_text("# runtime authority changed\n", encoding="utf-8")
    _, after_owned = identity._capture_runtime_build_trees(tmp_path, ())

    assert before["schema"] == "molt.runtime-build-tooling-authority.v2"
    assert before["file_count"] == len(authority_paths)
    assert after_unrelated == before
    assert after_owned["digest"] != before["digest"]


def test_runtime_tooling_source_projection_is_independent_of_install_layout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import python_environment_identity as capture

    source = tmp_path / "compiler-source"
    expected = identity.runtime_build_tooling_paths(source)
    package = tmp_path / "environment/Lib/site-packages/molt"
    monkeypatch.setattr(
        identity, "__file__", str(package / "cli/runtime_build_identity.py")
    )
    monkeypatch.setattr(
        capture, "__file__", str(package / "python_environment_identity.py")
    )
    assert identity.runtime_build_tooling_paths(source) == expected
    assert source / "src/molt/python_environment_identity.py" in expected
    assert source / "src/sitecustomize.py" in expected
    assert source / "pyproject.toml" in expected


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
    assert changed_pythonpath == baseline
    assert build_script["python_import_policy"] == "isolated-no-site-v1"
    assert "PYTHONPATH" not in build_script

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


@pytest.mark.parametrize("raw", [None, "", os.pathsep, "missing", "source-root"])
def test_build_script_python_import_policy_never_enumerates_ambient_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, raw: str | None
) -> None:
    environment = {} if raw is None else {"PYTHONPATH": raw}
    if raw == "source-root":
        environment["PYTHONPATH"] = str(tmp_path)
    monkeypatch.setattr(
        identity,
        "_tree_identity",
        lambda *_args, **_kwargs: pytest.fail("ambient import roots were enumerated"),
    )
    result = identity._build_python_script_environment_identity(
        environment, build_python_identity=build_python_identity_fixture()
    )
    assert result == identity._build_python_script_environment_identity(
        {}, build_python_identity=build_python_identity_fixture()
    )


@pytest.mark.parametrize(
    "script_schema",
    [
        "molt.runtime-build-script-environment",
        "molt.cpython-abi-build-script-environment",
    ],
)
def test_build_script_identity_rejects_retired_import_authority_and_policy_drift(
    script_schema: str,
) -> None:
    build_python = build_python_identity_fixture()
    payload = {
        "schema": script_schema + ".v2",
        **identity._build_python_script_environment_identity(
            {}, build_python_identity=build_python
        ),
    }
    if script_schema == "molt.runtime-build-script-environment":
        payload.update(
            {
                "MOLT_WASM_CPYTHON_ABI_EXPORTS": "ignored-for-target",
                "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": "ignored-for-target",
                "MOLT_WASM_LONGDOUBLE_ARCHIVE": {"state": "ignored-for-target"},
                "MOLT_WASM_BUILTINS_ARCHIVE": {"state": "ignored-for-target"},
            }
        )
    schema._validated_build_script_environment(
        payload, target="x86_64-test-native", build_python=build_python
    )
    for changed, message in (
        ({**payload, "schema": script_schema + ".v1"}, "schema"),
        ({**payload, "PYTHONPATH": {"state": "unset"}}, "shape"),
        ({**payload, "python_import_policy": "ambient"}, "import policy"),
    ):
        with pytest.raises(ValueError, match=message):
            schema._validated_build_script_environment(
                changed, target="x86_64-test-native", build_python=build_python
            )


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
    projection = identity._RuntimeFlagProjection.capture((("wasi-sysroot", sysroot),))
    assert projection.token(f"-I{sysroot / 'include'}") == "-I${wasi-sysroot}/include"
    with pytest.raises(ValueError, match="absolute host path"):
        projection.token(f"--sysroot={sysroot}bar")
    with pytest.raises(ValueError, match="absolute host path"):
        projection.token(f"embedded{sysroot}")
    with pytest.raises(ValueError, match="absolute host path"):
        projection.token("-I/opt/poison")


@pytest.mark.parametrize("token_count", [1, 256, 8192])
def test_flag_projection_resolves_roots_once_not_per_export(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, token_count: int
) -> None:
    roots = (("source", tmp_path), ("nested", tmp_path / "nested"))
    resolved: list[Path] = []
    original = Path.resolve

    def resolve(path: Path, *, strict: bool = False) -> Path:
        resolved.append(path)
        return original(path, strict=strict)

    monkeypatch.setattr(Path, "resolve", same_thread_probe(original, resolve))
    projection = identity._RuntimeFlagProjection.capture(roots)
    assert resolved == [path for _label, path in roots]
    exports = tuple(
        f"--export-if-defined=molt_test_{index}" for index in range(token_count)
    )
    assert projection.link_args(exports) == list(exports)
    assert projection.rustflags(exports) == list(exports)
    assert resolved == [path for _label, path in roots]
    operand = tmp_path / "nested" / "input.a"
    assert projection.token(str(operand)) == "${source}/nested/input.a"
    assert resolved == [path for _label, path in roots] + [operand]


def test_flag_projection_preserves_ordered_root_precedence(tmp_path: Path) -> None:
    projection = identity._RuntimeFlagProjection.capture(
        (("parent", tmp_path), ("child", tmp_path / "nested"))
    )
    operand = str(tmp_path / "nested" / "input.a")
    assert projection.token(operand) == "${parent}/nested/input.a"
    command_projection = projection.prepend_root("command", tmp_path / "nested")
    assert command_projection.token(operand) == "${command}/input.a"
    assert projection.token(operand) == "${parent}/nested/input.a"


def test_flag_projection_is_recaptured_not_process_cached(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    logical = tmp_path / "logical"
    before = tmp_path / "before"
    after = tmp_path / "after"
    current = before
    original = Path.resolve

    def resolve(path: Path, *, strict: bool = False) -> Path:
        return current if path == logical else original(path, strict=strict)

    monkeypatch.setattr(Path, "resolve", resolve)
    first = identity._RuntimeFlagProjection.capture((("input", logical),))
    current = after
    second = identity._RuntimeFlagProjection.capture((("input", logical),))
    assert first.token(str(before / "value")) == "${input}/value"
    assert second.token(str(after / "value")) == "${input}/value"
    with pytest.raises(ValueError, match="unknown absolute host path"):
        second.token(str(before / "value"))


def test_flag_projection_requires_cargo_plan_for_response_inputs(
    tmp_path: Path,
) -> None:
    projection = identity._RuntimeFlagProjection.capture((("source", tmp_path),))
    with pytest.raises(ValueError, match="requires captured Cargo plan custody"):
        projection.token("-Clink-arg=@" + str(tmp_path / "runtime.rsp"))


def test_tool_version_banner_never_serializes_installed_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = tmp_path / "clang.exe"
    write_mock_executable(tool, b"MZtool-bytes")
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
    file_count = 4 * identity._TREE_HASH_BATCH_SIZE + 1
    for index in range(file_count):
        (source / f"{index:04}.rs").write_bytes(bytes([index % 256]) * (index + 1))
    original = identity._hash_tree_input_file
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)
    serial = identity._tree_identity((("runtime/source", source),), require_all=True)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 4)

    def forward(file: identity._TreeInputFile) -> str:
        time.sleep((file_count - 1 - int(file.path.stem)) * 0.00001)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", forward)
    first = identity._tree_identity((("runtime/source", source),), require_all=True)

    def reverse(file: identity._TreeInputFile) -> str:
        time.sleep(int(file.path.stem) * 0.00001)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", reverse)
    second = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert serial == first == second
    assert first["file_count"] == file_count


@pytest.mark.parametrize(
    "file_count,expected_futures,expected_pending",
    [(3, 3, 3), (6 * identity._TREE_HASH_BATCH_SIZE + 7, 7, 6)],
)
def test_tree_identity_parallel_scheduler_bounds_in_flight_futures(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    file_count: int,
    expected_futures: int,
    expected_pending: int,
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(file_count):
        (source / f"{index:04}.rs").write_text(
            f"pub const VALUE_{index}: usize = {index};\n", encoding="utf-8"
        )
    pending_sizes: list[int] = []
    submitted = 0
    original_wait = identity.wait

    class ObservedExecutor(identity.ThreadPoolExecutor):
        def submit(self, *args, **kwargs):
            nonlocal submitted
            submitted += 1
            return super().submit(*args, **kwargs)

    def observed_wait(futures: object, **kwargs: object) -> object:
        pending_sizes.append(len(futures))  # type: ignore[arg-type]
        return original_wait(futures, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 3)
    monkeypatch.setattr(identity, "wait", observed_wait)
    monkeypatch.setattr(identity, "ThreadPoolExecutor", ObservedExecutor)

    result = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert result["file_count"] == file_count
    # One owned-read pass batches large closures and keeps small closures parallel.
    assert submitted == expected_futures
    assert pending_sizes
    assert max(pending_sizes) == expected_pending


def test_tree_identity_batched_capture_rejects_late_file_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(2 * identity._TREE_HASH_BATCH_SIZE + 1):
        (source / f"{index:04}.rs").write_bytes(b"before")
    victim = source / f"{identity._TREE_HASH_BATCH_SIZE + 3:04}.rs"
    original = identity._hash_tree_input_file

    def mutate_before_open(file: identity._TreeInputFile) -> str:
        if file.path == victim:
            before = victim.stat()
            victim.write_bytes(b"after!")
            os.utime(victim, ns=(before.st_atime_ns, before.st_mtime_ns))
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", mutate_before_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 2)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


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


@pytest.mark.parametrize("restore_mtime", [False, True])
def test_tree_identity_rejects_mutation_after_admission_before_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, restore_mtime: bool
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"before")
    original = identity._hash_tree_input_file

    def mutate_after_admission(file: identity._TreeInputFile) -> str:
        # The owning worker has admitted this read handle before invoking hash.
        before = file.path.stat()
        file.path.write_bytes(b"after!")
        os.utime(
            file.path,
            ns=(
                before.st_atime_ns,
                before.st_mtime_ns
                if restore_mtime
                else before.st_mtime_ns + 1_000_000_000,
            ),
        )
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", mutate_after_admission)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_cargo_configuration_parses_and_identifies_one_owned_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = tmp_path / "config.toml"
    raw = b"[build]\njobs = 3\n"
    config.write_bytes(raw)
    opens = []
    open_descriptor = toolchain_identity.open_stable_read_descriptor

    def counted_open(path: Path) -> int:
        opens.append(path)
        return open_descriptor(path)

    monkeypatch.setattr(toolchain_identity, "open_stable_read_descriptor", counted_open)
    captured = cargo_plans._capture_config(config, label="fixture config")
    assert captured.document == {"build": {"jobs": 3}}
    assert captured.identity.sha256 == hashlib.sha256(raw).hexdigest()
    assert captured.identity.size == len(raw)
    assert opens == [config]


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
    original = identity._capture_tree_input_file

    def delete_before_open(file: identity._TreeInputCandidate) -> tuple[int, str]:
        file.path.unlink()
        return original(file)

    monkeypatch.setattr(identity, "_capture_tree_input_file", delete_before_open)
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
    # lld selects its flavor from argv[0]: `wasm-ld` is a role name for the
    # generic driver. Identity must probe the role spelling, never the link
    # target, or the generic driver refuses to run and the tool is misrecorded.
    generic = build_native_executable(
        tmp_path / native_executable_name("lld"),
        """
fn main() {
    let argv0 = std::env::args().next().unwrap_or_default();
    let stem = std::path::Path::new(&argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    if stem == "wasm-ld" {
        println!("LLD 22.1.8");
    } else {
        eprintln!("lld is a generic driver");
        std::process::exit(1);
    }
}
""",
    )
    role = tmp_path / native_executable_name("wasm-ld")
    try:
        role.symlink_to(generic)
    except OSError as exc:  # Windows without the symlink privilege
        pytest.skip(f"host cannot create file symlinks: {exc}")

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
    restored = schema.RuntimeToolchainContentManifest.read(manifest_path)
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
        monkeypatch.setattr(
            identity, "_python_identity", lambda _env, **_kwargs: python
        )
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
        schema.RuntimeToolchainContentManifest.from_dict(value)


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
    runtime["runtime_closure_sha256"] = schema._digest(runtime_material)
    build_python_material = dict(build_python)
    build_python_material.pop("identity_sha256")
    build_python["identity_sha256"] = schema._digest(build_python_material)
    value["digest"] = schema._digest(payload)

    with pytest.raises(ValueError, match="build Python closure is invalid"):
        schema.RuntimeToolchainContentManifest.from_dict(value)


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

    def publish(manifest: schema.RuntimeToolchainContentManifest) -> None:
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
    final = schema.RuntimeToolchainContentManifest.read(path)
    assert final.digest in {first.digest, second.digest}


class _SessionInput:
    def __init__(self, owner):
        self.owner = owner
        self.closed = False

    def write(self, request):
        assert request == "verify\n"
        self.owner.verifications += 1
        if self.owner.verify is not None:
            self.owner.verify()
        self.owner.lines.put(self.owner.digest + "\n")

    def flush(self):
        pass

    def close(self):
        self.closed = True
        self.owner.closed = True
        self.owner.lines.put("")


class _SessionOutput:
    def __init__(self, owner):
        self.owner = owner

    def readline(self, limit):
        return self.owner.lines.get(timeout=5)

    def close(self):
        pass


class _CaptureSession:
    def __init__(self, payload):
        import queue

        self.lines = queue.Queue()
        self.selection = {
            "schema": "molt-python-startup-selection-v1",
            "native": {"paths": ["original"]},
        }
        self.fresh_selection = self.selection
        self.fresh_calls = []
        self.lines.put(
            json.dumps({"runtime": payload, "startup_selection": self.selection}) + "\n"
        )
        self.terminal = False
        self.digest = payload["runtime_closure_sha256"]
        self.stdin = _SessionInput(self)
        self.stdout = _SessionOutput(self)
        self.closed = False
        self.returncode = None
        self.verifications = 0
        self.verify = None

    def poll(self):
        return self.returncode


@pytest.fixture
def python_session(tmp_path, monkeypatch, runtime_receipt):
    from molt.cli import runtime_build_python as admission

    selected = tmp_path / "selected-python.exe"
    write_mock_executable(selected, b"selected-interpreter-launcher")
    session = _CaptureSession(runtime_receipt)
    calls = []

    class Executor:
        def start_guarded(self, command, **kwargs):
            calls.append((command, kwargs))
            return session

        def run(self, command, **kwargs):
            from types import SimpleNamespace

            session.fresh_calls.append((command, kwargs))
            return SimpleNamespace(stdout=json.dumps(session.fresh_selection))

        def wait_owned(self, process, **kwargs):
            assert process is session
            assert process.closed
            process.returncode = process.returncode or 0
            process.terminal = True
            return process.returncode

    monkeypatch.setattr(
        admission.process_guard, "source_command_executor", lambda _prefix: Executor()
    )
    return admission, selected, session, calls


def test_python_identity_uses_exact_isolated_facade_in_v3_build_receipt(
    python_session, runtime_receipt
):
    admission, selected, session, calls = python_session
    environment = {
        "MOLT_BUILD_PYTHON": str(selected),
        "PYTHON": "ignored-python",
        "PATH": "captured-path",
    }
    with admission.BuildPythonAdmission() as owner:
        first = identity._python_identity(environment, admission=owner)
        first["runtime"]["version"] = "caller mutation"
        second = identity._python_identity(environment, admission=owner)
    assert session.closed
    assert session.verifications == 1
    assert len(session.fresh_calls) == 1
    assert session.fresh_calls[0][0][-1] == "--runtime-selection"
    assert session.fresh_calls[0][1]["env"] == environment
    assert len(calls) == 1
    assert second["runtime"] == runtime_receipt
    assert second["identity_sha256"] == canonical_json_sha256(
        {key: value for key, value in second.items() if key != "identity_sha256"}
    )
    assert calls[0][0] == [
        str(selected),
        "-B",
        "-I",
        "-S",
        str(
            Path(identity.__file__).resolve().parents[1]
            / "python_environment_identity.py"
        ),
        "--capture-runtime",
        "--runtime-session",
        "--hash-workers",
        "4",
    ]
    assert calls[0][1]["env"] == environment
    with pytest.raises(ValueError, match="revoked"):
        owner.capture(environment)


@pytest.mark.parametrize(
    "change", ["interpreter", "same-size-restored-mtime", "loader-policy", "exit"]
)
def test_build_python_admission_rejects_changed_selection_without_restart(
    python_session, tmp_path, change
):
    admission, selected, session, calls = python_session
    environment = {"MOLT_BUILD_PYTHON": str(selected), "PATH": "original"}
    owner = admission.BuildPythonAdmission()
    owner.capture(environment)
    before = selected.stat()
    if change == "interpreter":
        alternate = tmp_path / "alternate.exe"
        alternate.write_bytes(selected.read_bytes())
        environment["MOLT_BUILD_PYTHON"] = str(alternate)
    elif change == "same-size-restored-mtime":
        selected.write_bytes(b"x" * before.st_size)
        os.utime(selected, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif change == "loader-policy":
        environment["PATH"] = "changed"
    else:
        session.returncode = 7
    with pytest.raises(ValueError):
        owner.capture(environment)
    assert session.closed
    assert len(calls) == 1
    with pytest.raises(ValueError, match="revoked"):
        owner.capture(environment)


@pytest.mark.parametrize(
    "corruption",
    ["malformed", "duplicate-root", "duplicate-node", "nonfinite", "invalid-receipt"],
)
def test_python_identity_rejects_ambiguous_probe_json(
    python_session, runtime_receipt, corruption
):
    admission, selected, session, _calls = python_session
    output = json.dumps(runtime_receipt)
    if corruption == "malformed":
        output = "{not-json"
    elif corruption == "duplicate-root":
        output = '{"schema":"discarded-duplicate",' + output[1:]
    elif corruption == "duplicate-node":
        output = output.replace('"size": 1', '"size": 1, "size": 1', 1)
    elif corruption == "nonfinite":
        output = "NaN"
    else:
        output = "{}"
    session.lines.get_nowait()
    if corruption in {"duplicate-root", "duplicate-node"}:
        output = (
            '{"runtime":'
            + output
            + ',"startup_selection":'
            + json.dumps(session.selection)
            + "}"
        )
    session.lines.put(output + "\n")
    with pytest.raises(ValueError):
        identity._python_identity({"MOLT_BUILD_PYTHON": str(selected)})
    assert session.closed


def test_python_identity_failed_close_revokes_admission(python_session):
    admission, selected, session, calls = python_session
    owner = admission.BuildPythonAdmission()
    env = {"MOLT_BUILD_PYTHON": str(selected)}
    owner.capture(env)
    session.returncode = 9
    with pytest.raises(ValueError, match="exit 9"):
        owner.close()
    with pytest.raises(ValueError, match="revoked"):
        owner.capture(env)
    assert len(calls) == 1


def test_python_identity_timeout_revokes_without_retry(python_session, monkeypatch):
    admission, selected, session, calls = python_session
    session.lines.get_nowait()
    # Initial content capture owns the admission budget; later verify requests
    # use the separate response budget. Exercise the boundary capture calls.
    monkeypatch.setattr(admission, "_ADMISSION_TIMEOUT", 0.001)
    owner = admission.BuildPythonAdmission()
    with pytest.raises(ValueError, match="timed out"):
        owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    assert session.closed
    assert len(calls) == 1
    with pytest.raises(ValueError, match="revoked"):
        owner.capture({"MOLT_BUILD_PYTHON": str(selected)})


def test_build_python_scope_borrows_only_live_operation_owner(python_session):
    from types import SimpleNamespace

    admission, selected, session, calls = python_session
    state = SimpleNamespace(build_python_admission=None)
    environment = {"MOLT_BUILD_PYTHON": str(selected)}
    with admission.build_python_scope(state) as owner:
        first = owner.capture(environment)
        with admission.build_python_scope(state) as borrowed:
            assert borrowed is owner
            assert borrowed.capture(environment) == first
        assert not session.closed
    assert session.closed
    assert state.build_python_admission is None
    assert len(calls) == 1


def test_python_identity_unresolved_interpreter_never_spawns_probe(
    python_session, tmp_path
):
    _admission, _selected, _session, calls = python_session
    with pytest.raises(ValueError, match="unavailable"):
        identity._python_identity({"MOLT_BUILD_PYTHON": str(tmp_path / "missing")})
    assert not calls


@pytest.mark.slow
def test_python_identity_live_content_probe() -> None:
    """The real guarded child retains its context through two phase boundaries."""
    from molt.cli.runtime_build_python import BuildPythonAdmission

    environment = {**os.environ, "MOLT_BUILD_PYTHON": sys.executable}
    with BuildPythonAdmission() as owner:
        first = owner.capture(environment)
        assert owner.capture(environment) == first
    runtime = validate_python_runtime_identity(first["runtime"])
    assert runtime["file_nodes"]
    assert runtime["native_dependency_closure"]["status"] == "closed"
    assert first["selected_executable"][
        "sha256"
    ] == toolchain_identity.stable_file_sha256(
        Path(sys.executable), label="test Python executable"
    )


@pytest.mark.parametrize("change", ["launcher-environment", "new-native-provider"])
def test_build_python_admission_observes_fresh_startup_selection(
    python_session, change
):
    admission, selected, session, calls = python_session
    env = {"MOLT_BUILD_PYTHON": str(selected), "LD_LIBRARY_PATH": "same-directory"}
    owner = admission.BuildPythonAdmission()
    owner.capture(env)
    if change == "launcher-environment":
        env["MOLT_REAL_PYTHON"] = "different-python"
    session.fresh_selection = {
        "schema": "molt-python-startup-selection-v1",
        "native": {"paths": ["different-provider"]},
    }
    with pytest.raises(ValueError, match="fresh startup selection changed"):
        owner.capture(env)
    assert len(calls) == 1
    assert session.fresh_calls[0][1]["env"] == env
    assert session.verifications == 0
    assert owner.terminal


def test_build_python_admission_verifies_content_after_fresh_selection(python_session):
    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    env = {"MOLT_BUILD_PYTHON": str(selected)}
    owner.capture(env)

    def reject_changed_content():
        assert len(session.fresh_calls) == 1
        raise ValueError("retained file generation changed")

    session.verify = reject_changed_content
    with pytest.raises(ValueError, match="file generation changed"):
        owner.capture(env)


def test_build_python_cleanup_retains_owner_and_primary_exception(
    python_session, monkeypatch, capsys
):
    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    original_stdout = session.stdout

    def blocked_wait(*args, **kwargs):
        raise RuntimeError("guardian still owns live child")

    def forbidden_close():
        raise AssertionError("must not close a potentially reader-locked stream")

    monkeypatch.setattr(owner._executor, "wait_owned", blocked_wait)
    monkeypatch.setattr(original_stdout, "close", forbidden_close)
    primary = LookupError("primary operation failed")
    with pytest.raises(LookupError) as caught:
        with owner:
            raise primary
    assert caught.value is primary
    assert owner._process is session
    assert not owner.terminal
    assert owner.cleanup_failure.admission is owner
    assert "guardian still owns live child" in primary.__notes__[0]
    assert json.loads(capsys.readouterr().err)["kind"] == "build-python-cleanup-failure"
    monkeypatch.setattr(original_stdout, "close", lambda: None)

    def terminal_wait(*args, **kwargs):
        session.terminal = True
        session.returncode = 0
        return 0

    monkeypatch.setattr(owner._executor, "wait_owned", terminal_wait)
    owner.close()
    assert owner.terminal


def test_build_python_cleanup_preserves_recorded_failure(
    python_session, monkeypatch, capsys
):
    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()

    def failed_operation():
        with owner:
            owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
            session.returncode = 9
            owner.record_failure("Cargo failed with its recorded diagnostic")
            return False

    assert failed_operation() is False
    diagnostic = json.loads(capsys.readouterr().err)
    assert "Cargo failed" in diagnostic["primary_failure"]
    assert "exit 9" in diagnostic["error"]
