"""V1 single-compile split-runtime unit tests (no real cargo build).

Verifies the combined-compile design invariants that make the dedup correct:

* the reloc and shared specs share one compile identity (same target dir, same
  cargo profile, same feature plan) but keep DISTINCT content fingerprints, so a
  single compile can serve both while the artifacts stay independently cached;
* the combined cargo invocation selects exactly ``staticlib,cdylib`` at Cargo
  level (so no dependency-only rlib is emitted) and routes the shared cdylib
  link args through a ``-C link-arg=@response`` file (not RUSTFLAGS);
* app routing has exactly one atomic pair ensure and no dual-compile fallback.
"""

from __future__ import annotations

from molt.cli.runtime_wasm_validation import (
    RuntimeWasmAdmissionIssue,
    RuntimeWasmAdmissionReport,
)

from functools import partial

import hashlib
import json
import os
import subprocess
import sys
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest
from molt.capability_manifest import CapabilityManifest
from molt.cli.python_source_closure import LocalPythonSourceClosure

from molt.cli import artifact_state as runtime_artifact_state
from molt.cli.backend_cache_setup import _build_cache_variant
from molt.target_python import TargetPythonVersion
from molt.cli import (
    runtime_build_identity,
    runtime_fingerprints,
    runtime_wasm_build,
    runtime_wasm_build_support,
    runtime_wasm_build_spec,
    runtime_wasm_pair_build,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.app_export_contract import build_app_export_contract
from molt.cli.models import (
    _ExternalNativeAbiSymbol,
    _ExternalNativeCapiSymbol,
    _ExternalPackageNativeArtifactPlan,
    _RuntimeArtifactState,
    _PreparedNonNativeResult,
)
from molt.cli.runtime_artifact_selection import (
    RUNTIME_CDYLIB_ARTIFACTS,
    RUNTIME_STATICLIB_ARTIFACTS,
)
from molt.cli.runtime_wasm_build_timings import (
    _reset_runtime_wasm_build_timings,
    _runtime_wasm_build_timings_snapshot,
)
from molt.cli.runtime_wasm_generation import (
    publish_runtime_wasm_generation,
    runtime_wasm_generation_path,
)
from tests.executable_test_support import write_mock_executable
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    mock_wasm_optimizer_cache_fact,
    mock_wasm_optimizer_publications,
    provisioned_wasi_sdk_fixture,
    bind_runtime_wasm_specs as _bind_specs,
    runtime_build_identity as make_runtime_build_identity,
    runtime_toolchain_content_manifest,
    runtime_cargo_plan,
)

_COMMON = dict(
    cargo_profile="release",
    simd_enabled=True,
    freestanding=False,
    stdlib_profile="full",
    resolved_modules=None,
    required_link_features=frozenset(),
    required_exports=None,
)


@pytest.mark.parametrize("cache", ["valid", "missing", "corrupt"])
def test_disabled_pair_build_still_admits_hydrated_generation(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, cache: str
) -> None:
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    shared = make_runtime_build_identity("shared", "cached")
    reloc = make_runtime_build_identity("reloc", "cached")
    state = _RuntimeArtifactState()
    shared_spec, reloc_spec = _specs(tmp_path)
    shared_path, reloc_path = (
        tmp_path / "molt_runtime.wasm",
        tmp_path / "molt_runtime_reloc.wasm",
    )
    ctx = runtime_wasm_pair_build._RuntimeWasmPairBuild(
        runtime_state=state,
        json_output=True,
        cargo_profile="release",
        cargo_timeout=5,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_link_features=frozenset(),
        required_exports=None,
        runtime_wasm=shared_path,
        runtime_reloc_wasm=reloc_path,
        shared_spec=shared_spec,
        reloc_spec=reloc_spec,
        toolchain_manifest_path=tmp_path / "toolchain.json",
        generation_manifest=runtime_wasm_generation_path(shared_path),
        pre_identity=runtime_wasm_pair_build._RuntimeWasmPairIdentity(
            shared.toolchain_manifest, shared, reloc
        ),
    )

    def hydrate(**_kwargs):
        if cache == "missing":
            return None
        source_shared, source_reloc = (
            tmp_path / "shared-source",
            tmp_path / "reloc-source",
        )
        source_shared.write_bytes(b"shared")
        source_reloc.write_bytes(b"reloc")
        generation = publish_runtime_wasm_generation(
            shared_path,
            reloc_path,
            shared_identity=shared,
            reloc_identity=reloc,
            source_shared=source_shared,
            source_reloc=source_reloc,
        )
        if cache == "corrupt":
            generation.shared.write_bytes(b"corrupt")
        return generation

    monkeypatch.setattr(
        runtime_wasm_pair_build, "hydrate_runtime_wasm_pair_from_shared_cache", hydrate
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_prepopulate_combined_runtime_wasm_target",
        lambda **_k: pytest.fail("rebuild-disabled policy started a build"),
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build._RuntimeWasmPairBuild,
        "provision_staging",
        lambda _s: pytest.fail("rebuild-disabled policy provisioned a build"),
    )

    # Generation identity and content admission remain real. These fixtures
    # contain synthetic module bytes, so only format/export validation is stubbed.
    def synthetic_exports(generation, _required):
        try:
            generation.verify_members()
        except ValueError as exc:
            return RuntimeWasmAdmissionReport(
                (RuntimeWasmAdmissionIssue("generation", "observation", str(exc)),)
            )
        return RuntimeWasmAdmissionReport()

    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "runtime_wasm_generation_admission",
        synthetic_exports,
    )
    outcome = runtime_wasm_pair_build._materialize_runtime_wasm_pair(ctx)
    if cache == "valid":
        assert outcome is runtime_wasm_pair_build._PairBuildOutcome.ACCEPTED
    else:
        assert outcome is runtime_wasm_pair_build._PairBuildOutcome.FAILED
        failure = state.runtime_wasm_build_failure
        assert failure is not None
        assert failure.stage == (
            "rebuild-policy" if cache == "missing" else "shared-cache-hydration"
        )


@pytest.fixture(autouse=True)
def _synthetic_cargo_plan(
    runtime_fixture_root: RuntimeFixtureRoot,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setattr(
        runtime_wasm_build_spec,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )


def _specs(root: Path):
    shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime.wasm", reloc=False, **_COMMON
    )
    reloc = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime_reloc.wasm", reloc=True, **_COMMON
    )
    return _bind_specs(shared, reloc, root=root)


@pytest.mark.parametrize("reloc", [False, True])
@pytest.mark.parametrize("freestanding", [False, True])
@pytest.mark.parametrize(
    ("build_profile", "explicit", "requested", "resolved"),
    [
        ("dev", None, "dev-fast", "dev-fast"),
        ("release", None, "release", "wasm-release"),
        ("release", "release-output", "release-output", "release-output"),
    ],
)
def test_public_profile_request_reaches_shared_and_reloc_build_specs(
    tmp_path,
    monkeypatch,
    reloc,
    freestanding,
    build_profile,
    explicit,
    requested,
    resolved,
):
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name

    for name in (
        "MOLT_DEV_CARGO_PROFILE",
        "MOLT_RELEASE_CARGO_PROFILE",
        "MOLT_WASM_CARGO_PROFILE",
        "MOLT_RUNTIME_BUILD_PROFILE",
    ):
        monkeypatch.delenv(name, raising=False)
    if explicit is not None:
        monkeypatch.setenv("MOLT_RELEASE_CARGO_PROFILE", explicit)
    profile_request, error = _resolve_cargo_profile_name(build_profile, wasm=True)
    assert error is None
    assert profile_request == requested
    spec = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        tmp_path,
        tmp_path / "runtime.wasm",
        reloc=reloc,
        **{**_COMMON, "cargo_profile": profile_request, "freestanding": freestanding},
    )
    assert spec.requested_cargo_profile == requested
    assert spec.cargo_profile == resolved
    assert spec.profile_dir == resolved


def test_wasm_cache_variant_binds_runtime_member_identity() -> None:
    common = dict(
        profile="dev",
        runtime_cargo="dev-fast",
        backend_cargo="dev-fast",
        emit="wasm",
        stdlib_split=False,
        codegen_env="same",
        linked=True,
        target_python=TargetPythonVersion(3, 12, 0),
    )
    first = _build_cache_variant(**common, runtime_wasm_codegen_digest="pair-a")
    assert first != _build_cache_variant(**common, runtime_wasm_codegen_digest="pair-b")
    assert first != _build_cache_variant(**common)


@pytest.mark.parametrize("freestanding", [False, True])
def test_codegen_pair_retains_public_abi_without_enabling_unrequested_domains(
    tmp_path: Path,
    freestanding: bool,
) -> None:
    common = {
        **_COMMON,
        "stdlib_profile": "micro",
        "freestanding": freestanding,
        "required_exports": {"PyTuple_New"},
        "full_export_surface": True,
    }
    shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        tmp_path,
        tmp_path / "molt_runtime.wasm",
        reloc=False,
        **common,
    )
    reloc = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        tmp_path,
        tmp_path / "molt_runtime_reloc.wasm",
        reloc=True,
        **common,
    )
    assert "--export-if-defined=molt_add" in reloc.runtime_exports
    assert "--export-if-defined=PyTuple_New" in reloc.runtime_exports
    assert reloc.runtime_exports == shared.runtime_exports
    assert "stdlib_crypto" not in reloc.fingerprint_features


@pytest.mark.parametrize(
    "outcome",
    ["stable", "source-drift", "missing-export", "member-corrupt", "pointer-replaced"],
)
def test_codegen_bound_pair_never_rebuilds_for_final_imports(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    outcome: str,
) -> None:
    shared_identity = make_runtime_build_identity("shared", "bound")
    reloc_identity = make_runtime_build_identity("reloc", "bound")
    source_shared = tmp_path / "source-shared"
    source_reloc = tmp_path / "source-reloc"
    source_shared.write_bytes(b"shared")
    source_reloc.write_bytes(b"reloc")
    generation = publish_runtime_wasm_generation(
        tmp_path / "molt_runtime.wasm",
        tmp_path / "molt_runtime_reloc.wasm",
        shared_identity=shared_identity,
        reloc_identity=reloc_identity,
        source_shared=source_shared,
        source_reloc=source_reloc,
    )
    state = _RuntimeArtifactState(
        runtime_wasm=tmp_path / "molt_runtime.wasm",
        runtime_reloc_wasm=tmp_path / "molt_runtime_reloc.wasm",
    )
    plans = []
    admissions = []
    builds = []
    shared_spec, reloc_spec = _specs(tmp_path)

    def prepare(runtime_state, **kwargs):
        plans.append((kwargs["required_exports"], kwargs["full_export_surface"]))
        current_shared, current_reloc = shared_identity, reloc_identity
        if len(plans) > 1 and outcome == "source-drift":
            current_shared = make_runtime_build_identity("shared", "changed")
            current_reloc = make_runtime_build_identity("reloc", "changed")
        return runtime_wasm_pair_build._RuntimeWasmPairBuild(
            runtime_state=runtime_state,
            json_output=True,
            cargo_profile="release",
            cargo_timeout=5,
            project_root=tmp_path,
            simd_enabled=True,
            freestanding=False,
            stdlib_profile="micro",
            resolved_modules=None,
            required_link_features=frozenset(),
            required_exports=kwargs["required_exports"],
            runtime_wasm=tmp_path / "molt_runtime.wasm",
            runtime_reloc_wasm=tmp_path / "molt_runtime_reloc.wasm",
            shared_spec=shared_spec,
            reloc_spec=reloc_spec,
            toolchain_manifest_path=tmp_path / "toolchain.json",
            generation_manifest=generation.manifest,
            pre_identity=runtime_wasm_pair_build._RuntimeWasmPairIdentity(
                current_shared.toolchain_manifest,
                current_shared,
                current_reloc,
            ),
        )

    def materialize(ctx):
        builds.append("admit-before-codegen")
        assert len(builds) == 1, (
            "final import validation must not build or hydrate another pair"
        )
        assert ctx.accept_generation()
        return runtime_wasm_pair_build._PairBuildOutcome.ACCEPTED

    def shared_exports(generation, required):
        generation.verify_members()
        admissions.append(required)
        return RuntimeWasmAdmissionReport(
            shared_missing_exports=("molt_hash_builtin",)
            if outcome == "missing-export" and required == {"add", "hash_builtin"}
            else ()
        )

    monkeypatch.setattr(
        runtime_wasm_pair_build, "_prepare_runtime_wasm_pair_build", prepare
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_materialize_runtime_wasm_pair", materialize
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "runtime_wasm_generation_admission",
        shared_exports,
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build._RuntimeWasmPairBuild,
        "generation_rejection_details",
        lambda self: {"required_exports": sorted(self.required_exports or ())},
    )
    kwargs = dict(
        json_output=True,
        cargo_profile="release",
        cargo_timeout=5,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
    )
    assert runtime_wasm_pair_build._ensure_runtime_wasm_both(
        state, bind_for_codegen=True, **kwargs
    )
    binding = state.runtime_wasm_codegen_binding
    assert binding is not None
    if outcome == "member-corrupt":
        generation.shared.write_bytes(b"corrupt")
    if outcome == "pointer-replaced":
        generation.manifest.write_text("{}", encoding="utf-8")
    passed = runtime_wasm_pair_build._ensure_runtime_wasm_both(
        state,
        required_exports={"add", "hash_builtin"},
        **kwargs,
    )
    assert passed == (outcome in {"stable", "pointer-replaced"})
    assert plans == [(None, True), (None, True)]
    assert builds == ["admit-before-codegen"]
    assert state.runtime_wasm_codegen_binding is binding
    assert state.runtime_wasm_selected == generation.shared
    assert state.runtime_reloc_wasm_selected == generation.reloc
    assert state.runtime_wasm_generation == binding.generation.manifest
    if passed:
        assert admissions[-1] == {"add", "hash_builtin"}
    else:
        assert state.runtime_wasm_build_failure is not None
        assert state.runtime_wasm_build_failure.stage.startswith("codegen-")


def test_synthetic_tools_do_not_mutate_read_only_source_root(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def forbid_tool_execution(*_args, **_kwargs):
        raise AssertionError("identity-only native fixture must not launch tools")

    monkeypatch.setattr(subprocess, "Popen", forbid_tool_execution)
    source = tmp_path / "read-only-source"
    source.mkdir()
    manifest = source / "Cargo.toml"
    manifest.write_text("[workspace]\n", encoding="utf-8")
    before = {path.name: path.read_bytes() for path in source.iterdir()}
    plan = runtime_cargo_plan(
        source,
        fixture_root=runtime_fixture_root,
        env={
            **{
                f"HOST_{role}": sys.executable for role in ("CC", "CXX", "AR", "RANLIB")
            },
            "RUSTC_WRAPPER": "fixture-wrapper",
            "RUSTC_WORKSPACE_WRAPPER": "fixture-workspace-wrapper",
        },
        cargo_command=("cargo", "rustc"),
    )
    sdk = provisioned_wasi_sdk_fixture(runtime_fixture_root)
    executable_digest = hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest()
    assert sdk.tool_fact("wasm-ld")["sha256"] == executable_digest
    if os.name == "posix":
        assert (
            sdk.wasm_ld.stat().st_mode & 0o7777
            == Path(sys.executable).stat().st_mode & 0o7777
        )
    assert len(plan.wrappers) == 2
    assert all(
        item.identity.sha256 == executable_digest
        for item in plan.executable_custody
        if item.label.startswith("wrapper/")
    )
    assert plan.project_root == source
    assert {path.name: path.read_bytes() for path in source.iterdir()} == before
    assert Path(plan.environment["CARGO_HOME"]).is_relative_to(
        runtime_fixture_root.path
    )
    assert all(
        path.is_relative_to(runtime_fixture_root.path)
        for path in plan.wrappers.values()
    )
    assert all(
        item.entrypoint.is_relative_to(runtime_fixture_root.path)
        for item in plan.rust_resources.files
    )
    assert sdk.wasm_ld.is_relative_to(runtime_fixture_root.path)
    assert (runtime_fixture_root.path / "test-rustlib").is_dir()
    assert sdk.sdk.is_dir()


@pytest.mark.parametrize(
    "root", [_compiler_root(), _compiler_root().parent, _compiler_root() / "src"]
)
def test_actual_source_root_cannot_become_fixture_owner(root: Path) -> None:
    with pytest.raises(ValueError, match="cannot own the compiler source root"):
        RuntimeFixtureRoot(root)


@pytest.mark.parametrize(
    "relative", ["..", "../escape", "nested/../../escape", ".", "absolute"]
)
def test_native_executable_fixture_rejects_unowned_paths(
    runtime_fixture_root: RuntimeFixtureRoot, relative: str
) -> None:
    if relative == "absolute":
        relative = str(runtime_fixture_root.path / "absolute-in-root")
    with pytest.raises(
        ValueError, match="confined relative path|escaped its pytest-owned root"
    ):
        runtime_fixture_root.native_executable(relative)


def test_synthetic_runtime_writes_require_typed_fixture_ownership(
    tmp_path: Path,
) -> None:
    unowned = cast(RuntimeFixtureRoot, tmp_path)
    with pytest.raises(TypeError, match="pytest-owned runtime_fixture_root"):
        runtime_cargo_plan(
            tmp_path, fixture_root=unowned, env={}, cargo_command=("cargo",)
        )
    with pytest.raises(TypeError, match="pytest-owned runtime_fixture_root"):
        provisioned_wasi_sdk_fixture(unowned)
    assert not (tmp_path / "test-rustlib").exists()
    assert not (tmp_path / "runtime-link-inputs").exists()


def test_runtime_publication_authority_is_exact_and_content_addressed(
    tmp_path: Path,
) -> None:
    root = _compiler_root()
    authority_paths = {
        path.relative_to(root).as_posix()
        for path in runtime_build_identity.runtime_build_tooling_paths(root)
    }
    for relative in authority_paths:
        source = root / relative
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())

    _, before = runtime_build_identity._capture_runtime_build_trees(tmp_path, ())
    assert before["file_count"] == len(authority_paths)

    changed = tmp_path / "src/molt/cli/runtime_wasm_build.py"
    changed.write_bytes(changed.read_bytes() + b"\n# publication mutation\n")
    _, after = runtime_build_identity._capture_runtime_build_trees(tmp_path, ())
    assert after["digest"] != before["digest"]


def test_reloc_and_shared_specs_share_compile_but_differ_in_fingerprint() -> None:
    root = _compiler_root()
    shared, reloc = _specs(root)
    assert shared.fingerprint is not None and reloc.fingerprint is not None
    # One compile home + one feature plan for both crate-types...
    assert shared.target_root == reloc.target_root
    assert shared.cargo_profile == reloc.cargo_profile
    assert shared.profile_dir == reloc.profile_dir
    assert shared.no_default_features == reloc.no_default_features
    assert shared.wasm_cargo_features == reloc.wasm_cargo_features
    assert shared.artifact_selection is RUNTIME_CDYLIB_ARTIFACTS
    assert reloc.artifact_selection is RUNTIME_STATICLIB_ARTIFACTS
    # ...but the artifacts have distinct content identities (link args differ).
    assert shared.fingerprint["hash"] != reloc.fingerprint["hash"]
    # Shared link flags carry the split-runtime import ABI.
    for flag in ("--import-memory", "--import-table", "--growable-table"):
        assert flag in shared.link_flags


def test_explicit_sdk_verify_rejects_changed_linker(runtime_fixture_root):
    from molt import llvm_toolchain

    sdk = provisioned_wasi_sdk_fixture(runtime_fixture_root)
    before = sdk.tool_fact("wasm-ld")["sha256"]
    sdk.wasm_ld.write_bytes(sdk.wasm_ld.read_bytes() + b"fixture-change")
    assert (
        provisioned_wasi_sdk_fixture(runtime_fixture_root).tool_fact("wasm-ld")[
            "sha256"
        ]
        == before
    )
    with pytest.raises(llvm_toolchain.LlvmToolchainConfigError, match="tree differs"):
        llvm_toolchain.load_wasi_sdk_installation(
            _compiler_root(), sdk.prefix, verify_tree=True
        )


@pytest.mark.parametrize("freestanding", [False, True])
def test_native_plan_is_the_pre_staging_runtime_export_authority(
    tmp_path: Path,
    freestanding: bool,
) -> None:
    artifact = type(
        "Artifact",
        (),
        {
            "c_api_symbols": (
                _ExternalNativeCapiSymbol(
                    symbol="PyTuple_New",
                    status="cpython_abi_link",
                    primitive_class="cpython_abi",
                    source="required_c_api_symbols",
                ),
                _ExternalNativeCapiSymbol(
                    symbol="PyArray_NDIM",
                    status="project_generated",
                    primitive_class="project_generated",
                    source="required_c_api_symbols",
                ),
            ),
            "abi_symbols": (
                _ExternalNativeAbiSymbol(
                    symbol="PyExc_TypeError",
                    status="external_link",
                    primitive_class="molt_cpython_abi_link_import",
                    source="undefined_symbols",
                ),
                _ExternalNativeAbiSymbol(
                    symbol="molt_hash_new",
                    status="runtime_backed",
                    primitive_class="wasm_runtime_import",
                    source="runtime_symbols+undefined_symbols",
                ),
                _ExternalNativeAbiSymbol(
                    symbol="memcpy",
                    status="external_link",
                    primitive_class="wasm_libc_link_import",
                    source="undefined_symbols",
                ),
            ),
        },
    )()
    plan = _ExternalPackageNativeArtifactPlan(artifacts=(artifact,))  # type: ignore[arg-type]

    assert plan.runtime_export_symbols() == frozenset(
        {"PyTuple_New", "PyExc_TypeError", "molt_hash_new"}
    )
    specs = [
        runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
            tmp_path,
            tmp_path / name,
            reloc=reloc,
            **{
                **_COMMON,
                "stdlib_profile": "micro",
                "freestanding": freestanding,
                "required_exports": plan.runtime_export_symbols(),
                "full_export_surface": True,
            },
        )
        for name, reloc in (
            ("molt_runtime.wasm", False),
            ("molt_runtime_reloc.wasm", True),
        )
    ]
    for spec in specs:
        assert "stdlib_crypto" in spec.fingerprint_features
        for symbol in plan.runtime_export_symbols():
            assert f"--export-if-defined={symbol}" in spec.runtime_exports
    assert specs[0].fingerprint_features == specs[1].fingerprint_features


def test_staticlib_compile_identity_survives_final_export_expansion_and_relinks(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    root = _compiler_root()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    common = {
        **_COMMON,
        "cargo_profile": "release-fast",
        "stdlib_profile": "full",
    }
    early = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        tmp_path / "early_reloc.wasm",
        reloc=True,
        **{**common, "required_exports": {"add"}},
    )
    final = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        tmp_path / "final_reloc.wasm",
        reloc=True,
        **{**common, "required_exports": {"add", "typing_get_origin"}},
    )
    early_shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        tmp_path / "early_shared.wasm",
        reloc=False,
        **{**common, "required_exports": {"add"}},
    )
    final_shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        tmp_path / "final_shared.wasm",
        reloc=False,
        **{**common, "required_exports": {"add", "typing_get_origin"}},
    )
    _, early = _bind_specs(
        early_shared, early, root=root, family_seed="early", compile_seed="same-compile"
    )
    _, final = _bind_specs(
        final_shared, final, root=root, family_seed="final", compile_seed="same-compile"
    )
    assert early.fingerprint is not None and final.fingerprint is not None
    assert early.staticlib_fingerprint is not None
    assert early.fingerprint["meta_digest"] != final.fingerprint["meta_digest"]
    assert early.staticlib_fingerprint["hash"] == final.staticlib_fingerprint["hash"]

    final = final._replace(target_root=target_root)
    staticlib = runtime_wasm_build_support._wasm_runtime_staticlib_path(
        target_root, final.profile_dir
    )
    staticlib.parent.mkdir(parents=True, exist_ok=True)
    staticlib.write_bytes(b"!<arch>\n")
    state_root = target_root / ".molt_state"
    staticlib_sidecar = runtime_artifact_state._runtime_target_fingerprint_path(
        state_root,
        staticlib,
        cargo_profile=final.cargo_profile,
        target_label="wasm32-wasip1",
    )
    staticlib_sidecar.parent.mkdir(parents=True, exist_ok=True)
    runtime_fingerprints._write_runtime_fingerprint(
        staticlib_sidecar,
        early.staticlib_fingerprint,
        artifact=staticlib,
    )

    linked: list[tuple[Path, str]] = []
    monkeypatch.setattr(
        runtime_wasm_build, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_run_runtime_wasm_cargo_build",
        lambda **kwargs: (_ for _ in ()).throw(
            AssertionError("cross-export staticlib reuse must not invoke Cargo")
        ),
    )

    def _relink(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
        export_link_args: str,
        **_kwargs: object,
    ) -> bool:
        del json_output, link_timeout
        linked.append((staticlib_path, export_link_args))
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"\0asm\x01\0\0\0")
        return True

    monkeypatch.setattr(
        runtime_wasm_build, "_link_runtime_staticlib_to_reloc_wasm", _relink
    )
    monkeypatch.setattr(
        runtime_wasm_build,
        "_runtime_missing_exports_for_mode",
        lambda _path, _required, *, reloc: set(),
    )
    output = tmp_path / "molt_runtime_reloc.wasm"
    assert runtime_wasm_build._materialize_runtime_wasm_member_from_target(
        output,
        reloc=True,
        json_output=True,
        cargo_timeout=1.0,
        project_root=root,
        required_exports={"add", "typing_get_origin"},
        resolved_modules=None,
        spec=final,
    )
    assert linked and linked[0][0] == staticlib
    assert "--export-if-defined=molt_typing_get_origin" in linked[0][1]


def test_relocation_root_feature_closure_reports_one_cargo_compile(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    root = _compiler_root()
    target_root = tmp_path / "target"
    state_root = tmp_path / "state"
    common = {
        **_COMMON,
        "cargo_profile": "release-fast",
        "stdlib_profile": "full",
        "required_link_features": frozenset(
            {
                "stdlib_ast",
                "stdlib_crypto",
                "stdlib_http",
                "stdlib_math",
                "stdlib_regex",
            }
        ),
    }

    def _pair(label: str, required_exports: set[str]):
        shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
            root,
            tmp_path / f"{label}_shared.wasm",
            reloc=False,
            **{**common, "required_exports": required_exports},
        )
        reloc = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
            root,
            tmp_path / f"{label}_reloc.wasm",
            reloc=True,
            **{**common, "required_exports": required_exports},
        )
        shared, reloc = _bind_specs(
            shared,
            reloc,
            root=root,
            family_seed=label,
            compile_seed="shared-compile",
        )
        return shared._replace(target_root=target_root), reloc._replace(
            target_root=target_root
        )

    early_shared, early_reloc = _pair("early", {"add"})
    final_shared, final_reloc = _pair(
        "final",
        {
            "add",
            "ast_parse",
            "hash_builtin",
            "ctypes_sizeof",
            "math_sqrt",
            "re_compile",
        },
    )
    assert early_shared.fingerprint["hash"] == final_shared.fingerprint["hash"]
    assert early_reloc.fingerprint != final_reloc.fingerprint
    assert (
        early_reloc.staticlib_fingerprint["hash"]
        == final_reloc.staticlib_fingerprint["hash"]
    )

    profile_root = target_root / "wasm32-wasip1" / early_shared.profile_dir
    cdylib = profile_root / "deps" / "molt_runtime-feedface.wasm"
    staticlib = profile_root / "deps" / "libmolt_runtime-feedface.a"
    cargo_calls: list[list[str]] = []

    def _fake_build(**kwargs):  # noqa: ANN003
        cmd = list(kwargs["cargo_plan"].command)
        cargo_calls.append(cmd)
        cdylib.parent.mkdir(parents=True, exist_ok=True)
        cdylib.write_bytes(b"\0asm\x01\0\0\0")
        staticlib.write_bytes(b"!<arch>\n")
        stdout = json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": "path+file:///repo/runtime/molt-runtime#0.0.1",
                "target": {"name": "molt_runtime"},
                "filenames": [str(cdylib), str(staticlib)],
            }
        )
        return subprocess.CompletedProcess(cmd, 0, stdout, ""), cdylib

    monkeypatch.setattr(
        runtime_wasm_pair_build, "_run_runtime_wasm_cargo_build", _fake_build
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_inspect_wasm_binary", lambda _path: "valid"
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda _path: True,
    )

    _reset_runtime_wasm_build_timings()
    try:
        assert runtime_wasm_pair_build._prepopulate_combined_runtime_wasm_target(
            runtime_state=_RuntimeArtifactState(),
            shared_spec=early_shared,
            reloc_spec=early_reloc,
            json_output=True,
            cargo_timeout=None,
            project_root=root,
            simd_enabled=True,
            freestanding=False,
        )
        assert runtime_wasm_pair_build._prepopulate_combined_runtime_wasm_target(
            runtime_state=_RuntimeArtifactState(),
            shared_spec=final_shared,
            reloc_spec=final_reloc,
            json_output=True,
            cargo_timeout=None,
            project_root=root,
            simd_enabled=True,
            freestanding=False,
        )
        snapshot = _runtime_wasm_build_timings_snapshot()
        assert snapshot is not None
        assert snapshot["cargo_compile_builds"] == 1
        assert len(cargo_calls) == 1
    finally:
        _reset_runtime_wasm_build_timings()


def _test_pair_identity():
    return runtime_wasm_pair_build._RuntimeWasmPairIdentity(
        toolchain=runtime_toolchain_content_manifest(
            "test-family",
            target_triple="wasm32-wasip1",
        ),
        shared=make_runtime_build_identity("shared", "test-family"),
        reloc=make_runtime_build_identity("reloc", "test-family"),
    )


def _test_pair_context(
    tmp_path: Path,
    *,
    required_exports: set[str] | None = None,
) -> runtime_wasm_pair_build._RuntimeWasmPairBuild:
    shared, reloc = _specs(_compiler_root())
    canonical_shared = tmp_path / "wasm" / "molt_runtime.wasm"
    canonical_reloc = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    return runtime_wasm_pair_build._RuntimeWasmPairBuild(
        runtime_state=_RuntimeArtifactState(),
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=tmp_path,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_link_features=frozenset(),
        required_exports=required_exports,
        runtime_wasm=canonical_shared,
        runtime_reloc_wasm=canonical_reloc,
        shared_spec=shared,
        reloc_spec=reloc,
        toolchain_manifest_path=tmp_path / "toolchain.json",
        generation_manifest=canonical_shared.with_name("molt_runtime.generation.json"),
        pre_identity=_test_pair_identity(),
    )


def test_pair_member_staging_is_identity_local_and_never_process_cached(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    ctx = _test_pair_context(tmp_path)
    identity = ctx.pre_identity
    assert identity is not None
    canonical_shared = ctx.runtime_wasm
    canonical_reloc = ctx.runtime_reloc_wasm
    shared = ctx.shared_spec
    reloc = ctx.reloc_spec
    calls: list[tuple[Path, bool, object]] = []

    def ensure(  # noqa: ANN003
        path: Path, *, reloc: bool, spec: object, **_kwargs
    ) -> bool:
        calls.append((path, reloc, spec))
        return True

    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_materialize_runtime_wasm_member_from_target",
        ensure,
    )
    ctx.provision_staging()
    first_root = ctx.staging_root
    assert first_root is not None
    assert first_root.parent.name == identity.shared.family_digest
    assert ctx.staging_member(reloc=False).name == canonical_shared.name
    assert ctx.staging_member(reloc=True).name == canonical_reloc.name
    assert ctx.ensure_member(reloc=False)
    assert ctx.ensure_member(reloc=False)
    assert ctx.ensure_member(reloc=True)
    assert ctx.ensure_member(reloc=True)
    assert calls == [
        (ctx.staging_member(reloc=False), False, shared),
        (ctx.staging_member(reloc=False), False, shared),
        (ctx.staging_member(reloc=True), True, reloc),
        (ctx.staging_member(reloc=True), True, reloc),
    ]
    concurrent_ctx = replace(
        ctx,
        staging_root=None,
        staging_shared=None,
        staging_reloc=None,
    )
    concurrent_ctx.provision_staging()
    concurrent_root = concurrent_ctx.staging_root
    assert concurrent_root is not None
    assert concurrent_root != first_root
    assert concurrent_root.parent == first_root.parent
    concurrent_ctx.cleanup_staging()
    assert not concurrent_root.exists()
    ctx.cleanup_staging()
    assert not first_root.exists()


def test_pair_target_materialization_keeps_canonical_spec_without_output_sidecar(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    shared, _reloc = _specs(_compiler_root())
    destination = tmp_path / "staging" / "molt_runtime.wasm"
    observed: list[tuple[Path, object, bool]] = []

    def reuse(ctx, *, persist_output_fingerprint: bool):  # noqa: ANN001
        observed.append((ctx.runtime_wasm, ctx.spec, persist_output_fingerprint))
        return True

    monkeypatch.setattr(runtime_wasm_build, "_reuse_target_runtime_wasm", reuse)
    assert runtime_wasm_build._materialize_runtime_wasm_member_from_target(
        destination,
        reloc=False,
        json_output=True,
        cargo_timeout=5.0,
        project_root=tmp_path,
        required_exports=None,
        resolved_modules=None,
        spec=shared,
    )
    assert observed == [(destination, shared, False)]


def test_final_required_export_abi_closes_the_cargo_feature_plan() -> None:
    root = _compiler_root()
    required_exports = {
        # Actual field names imported from module ``molt_runtime`` by the app.
        # The runtime export authority canonicalizes them to ``molt_*`` symbols
        # before the generated symbol-to-feature projection.
        "ast_parse",
        "ctypes_sizeof",
        "math_sqrt",
        "re_compile",
    }
    shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        root / "wasm" / "molt_runtime.wasm",
        reloc=False,
        **{
            **_COMMON,
            "stdlib_profile": "micro",
            "required_exports": required_exports,
        },
    )
    reloc = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        root / "wasm" / "molt_runtime_reloc.wasm",
        reloc=True,
        **{
            **_COMMON,
            "stdlib_profile": "micro",
            "required_exports": required_exports,
        },
    )

    expected = {"stdlib_ast", "stdlib_http", "stdlib_math", "stdlib_regex"}
    assert expected <= set(shared.wasm_cargo_features)
    assert shared.wasm_cargo_features == reloc.wasm_cargo_features
    assert expected <= set(shared.fingerprint_features)


def test_combined_cargo_cmd_selects_exact_pair_and_uses_response_file(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = _compiler_root()
    shared, reloc = _specs(root)
    captured: dict[str, object] = {}

    def _fake_build(
        *,
        cargo_plan,
        cargo_timeout,
        profile_dir,
        target_root_override,
        json_output,
        artifact_kind,
    ):
        cmd = cargo_plan.command
        captured["cmd"] = list(cmd)
        captured["env"] = dict(cargo_plan.environment)
        # Return a non-zero build so _prepopulate returns without touching disk.
        return (
            subprocess.CompletedProcess(cmd, 1, "", "stopped-for-test"),
            Path("unused"),
        )

    monkeypatch.setattr(
        runtime_wasm_pair_build, "_run_runtime_wasm_cargo_build", _fake_build
    )
    # Force a cold target dir so the fast-path reuse check misses and we build.
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_current_runtime_target_artifact",
        lambda *a, **k: None,
    )

    ok = runtime_wasm_pair_build._prepopulate_combined_runtime_wasm_target(
        runtime_state=_RuntimeArtifactState(),
        shared_spec=shared,
        reloc_spec=reloc,
        json_output=True,
        cargo_timeout=None,
        project_root=root,
        simd_enabled=True,
        freestanding=False,
    )
    assert ok is False  # fake build returned non-zero
    cmd = captured["cmd"]
    assert isinstance(cmd, list)
    # The producer overrides the manifest rlib default with exactly the two
    # external artifact types consumed by split runtime.
    selector = cmd.index("--crate-type")
    assert cmd[selector : selector + 2] == [
        "--crate-type",
        "staticlib,cdylib",
    ]
    assert selector < cmd.index("--")
    assert "--lib" in cmd
    # Shared cdylib link args delivered via a single response-file link arg.
    link_arg_tokens = [
        tok for tok in cmd if isinstance(tok, str) and tok.startswith("link-arg=@")
    ]
    assert len(link_arg_tokens) == 1
    # RUSTFLAGS must NOT carry the per-export link args (they moved to -C link-arg).
    env = captured["env"]
    assert "--export-if-defined" not in env.get("RUSTFLAGS", "")


@pytest.mark.parametrize("report_staticlib", [True, False])
def test_combined_build_requires_and_fingerprints_only_reported_crate_types(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    report_staticlib: bool,
) -> None:
    root = _compiler_root()
    shared, reloc = _specs(root)
    target_root = tmp_path / "target"
    shared = shared._replace(target_root=target_root)
    reloc = reloc._replace(target_root=target_root)
    profile_root = target_root / "wasm32-wasip1" / shared.profile_dir
    deps = profile_root / "deps"
    stale_cdylib = profile_root / "molt_runtime.wasm"
    stale_staticlib = profile_root / "libmolt_runtime.a"
    reported_cdylib = deps / "molt_runtime-feedface.wasm"
    reported_staticlib = deps / "libmolt_runtime-feedface.a"
    for path, payload in (
        (stale_cdylib, b"stale-cdylib"),
        (stale_staticlib, b"stale-staticlib"),
        (reported_cdylib, b"reported-cdylib"),
        (reported_staticlib, b"reported-staticlib"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    reported_filenames = [str(reported_cdylib)]
    if report_staticlib:
        reported_filenames.append(str(reported_staticlib))
    cargo_stdout = json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": "path+file:///repo/runtime/molt-runtime#0.0.1",
            "target": {"name": "molt_runtime"},
            "filenames": reported_filenames,
        }
    )

    def _fake_build(**kwargs):  # noqa: ANN003
        cmd = kwargs["cargo_plan"].command
        return (
            subprocess.CompletedProcess(cmd, 0, cargo_stdout, ""),
            reported_cdylib,
        )

    state_root = tmp_path / ".molt_state"
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_run_runtime_wasm_cargo_build", _fake_build
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_current_runtime_target_artifact",
        lambda *a, **k: None,
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build, "_inspect_wasm_binary", lambda _path: "valid"
    )
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda _path: True,
    )

    assert (
        runtime_wasm_pair_build._prepopulate_combined_runtime_wasm_target(
            runtime_state=_RuntimeArtifactState(),
            shared_spec=shared,
            reloc_spec=reloc,
            json_output=True,
            cargo_timeout=None,
            project_root=root,
            simd_enabled=True,
            freestanding=False,
        )
        is report_staticlib
    )

    def _target_fingerprint(path: Path) -> Path:
        return runtime_artifact_state._runtime_target_fingerprint_path(
            state_root,
            path,
            cargo_profile=shared.cargo_profile,
            target_label="wasm32-wasip1",
        )

    assert _target_fingerprint(reported_cdylib).exists() is report_staticlib
    assert _target_fingerprint(reported_staticlib).exists() is report_staticlib
    assert not _target_fingerprint(stale_cdylib).exists()
    assert not _target_fingerprint(stale_staticlib).exists()


# ---------------------------------------------------------------------------
# App-path routing: the split-runtime app build (`molt build --target wasm
# --split-runtime`, the witness) must route the reloc staticlib + shared cdylib
# through ONE combined ensure so the runtime builds ONCE, not twice, per app
# build, with no dual-compile compatibility lane.
# ---------------------------------------------------------------------------

import molt.cli.non_native_output as nno  # noqa: E402


def _host_receipt(*, runtime: bool = False) -> str:
    artifacts = {
        "main": {
            "source": "output_linked.wasm",
            "path": "output_linked.molt.cwasm",
            "source_sha256": "a" * 64,
            "sha256": "b" * 64,
            "size": 12,
        }
    }
    if runtime:
        artifacts["runtime"] = {
            "source": "molt_runtime.wasm",
            "path": "molt_runtime.molt.cwasm",
            "source_sha256": "c" * 64,
            "sha256": "d" * 64,
            "size": 18,
        }
    return json.dumps(
        {"version": 1, "kind": "molt-wasm-precompile", "artifacts": artifacts}
    )


def test_precompile_receipt_accepts_host_main_and_runtime_paths() -> None:
    artifacts = nno._precompile_receipt_artifacts(_host_receipt(runtime=True))
    assert artifacts["main"]["path"] == "output_linked.molt.cwasm"
    assert artifacts["runtime"]["path"] == "molt_runtime.molt.cwasm"


@pytest.mark.parametrize(
    "receipt",
    [
        "not-json",
        json.dumps({"version": 2, "kind": "molt-wasm-precompile", "artifacts": {}}),
        json.dumps({"version": 1, "kind": "wrong", "artifacts": {}}),
        json.dumps({"version": 1, "kind": "molt-wasm-precompile", "artifacts": {}}),
        json.dumps(
            {
                "version": 1,
                "kind": "molt-wasm-precompile",
                "artifacts": {"main": None, "runtime": None},
            }
        ),
        json.dumps(
            {
                "version": 1,
                "kind": "molt-wasm-precompile",
                "artifacts": {"main": {"path": "x"}},
            }
        ),
    ],
)
def test_precompile_receipt_rejects_malformed_host_output(receipt: str) -> None:
    with pytest.raises(ValueError, match="molt-wasm-host"):
        nno._precompile_receipt_artifacts(receipt)


@pytest.mark.parametrize(
    "field, value",
    [("source", ""), ("size", True), ("size", 0), ("sha256", "A" * 64)],
)
def test_precompile_receipt_rejects_invalid_host_artifact_fields(
    field: str, value: object
) -> None:
    receipt = json.loads(_host_receipt())
    receipt["artifacts"]["main"][field] = value
    with pytest.raises(ValueError, match="invalid main artifact"):
        nno._precompile_receipt_artifacts(json.dumps(receipt))


def test_precompile_receipt_output_validation_fails_closed(tmp_path: Path) -> None:
    artifacts = nno._precompile_receipt_artifacts(_host_receipt())
    with pytest.raises(ValueError, match="did not publish main"):
        nno._validate_precompile_receipt_outputs(artifacts)

    output = tmp_path / "output_linked.molt.cwasm"
    output.write_bytes(b"wrong")
    artifacts["main"]["path"] = str(output)
    with pytest.raises(ValueError, match="corrupt main"):
        nno._validate_precompile_receipt_outputs(artifacts)


# Publication, source-mutation, container-integrity, and atomic-replacement
# tests moved to runtime/molt-wasm-host: Rust is the sole producer.  This file
# covers Python's invocation ordering, diagnostics, and receipt-consumer boundary.


def _empty_app_export_contract(tmp_path: Path) -> Path:
    path = tmp_path / "app_export_contract.json"
    path.write_text(
        json.dumps(
            build_app_export_contract(
                entry_module="app_out",
                ir={
                    "functions": [
                        {
                            "name": "app_out__module_init",
                            "app_callable_bindings": [],
                        }
                    ]
                },
                registry_digest="c" * 64,
            )
        ),
        encoding="utf-8",
    )
    return path


def _record_ensure(name: str, ret: bool, log: list[str]):
    def _ensure(required_exports=None) -> bool:  # noqa: ANN001
        log.append(name)
        return ret

    return _ensure


def _run_app_ensure_routing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    freestanding: bool,
    both_callable: bool,
    both_ret: bool,
) -> list[str]:
    """Drive `_prepare_non_native_build_result` up to the runtime-ensure branch.

    All ensures record their name; each configured return value is chosen so the
    function short-circuits (returns a ``_fail``) right after the ensure
    decision -- either because an ensure returns False, or because the shared
    runtime artifact path does not exist.  The returned list is the ORDER of
    ensures actually invoked, which is exactly the routing decision under test.
    """
    # Stub the wasm inspection helpers so no real module is parsed.
    monkeypatch.setattr(
        nno, "_collect_wasm_module_import_names", lambda *a, **k: {"molt_PyA"}
    )
    monkeypatch.setattr(nno, "_validate_wasm_structural", lambda *a, **k: None)
    output_wasm = tmp_path / "app_out.wasm"
    output_wasm.write_bytes(b"\0asm")
    runtime_reloc = tmp_path / "molt_runtime_reloc.wasm"
    runtime_reloc.write_bytes(b"\0asm")
    # Deliberately MISSING so the non-freestanding path _fails at exists() right
    # after the shared-ensure decision (keeps the test off the real link path).
    runtime_shared_missing = tmp_path / "molt_runtime.wasm"

    log: list[str] = []
    _result, err = nno._prepare_non_native_build_result(
        resolved_capability_policy=CapabilityManifest().resolve(),
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=freestanding,
        linked=True,
        require_linked=False,
        linked_output_path=None,
        output_artifact=output_wasm,
        json_output=True,
        runtime_state=_RuntimeArtifactState(
            runtime_wasm=runtime_shared_missing,
            runtime_reloc_wasm=runtime_reloc,
            runtime_wasm_selected=runtime_shared_missing,
            runtime_reloc_wasm_selected=runtime_reloc,
        ),
        ensure_runtime_wasm_both=(
            _record_ensure("both", both_ret, log) if both_callable else None
        ),
        runtime_cargo_profile="release",
        molt_root=tmp_path,
        split_runtime=True,
        wasm_facts_scanner=tmp_path / "molt-wasm-facts",
        app_export_contract_path=_empty_app_export_contract(tmp_path),
    )
    # Every configured scenario short-circuits before a successful build.
    assert err is not None
    return log


def test_app_path_default_routes_through_combined_ensure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=True,
        both_ret=True,
    )
    # ONE combined ensure; the standalone reloc/shared ensures are NOT invoked.
    assert log == ["both"]


def test_app_path_fails_closed_when_combined_authority_is_absent(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=False,
        both_ret=True,
    )
    assert log == []


def test_app_path_freestanding_also_requires_atomic_pair(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=True,
        both_callable=True,
        both_ret=False,
    )
    assert log == ["both"]


def test_unlinked_app_path_builds_atomic_runtime_pair_before_staging(
    isolated_molt_cache: Path, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(
        nno, "_collect_wasm_module_import_names", lambda *a, **k: {"molt_PyA"}
    )
    output_wasm = tmp_path / "app_out.wasm"
    output_wasm.write_bytes(b"\0asm")
    calls: list[set[str] | frozenset[str] | None] = []

    def ensure_pair(required_exports=None) -> bool:  # noqa: ANN001
        calls.append(required_exports)
        return False

    _result, err = nno._prepare_non_native_build_result(
        resolved_capability_policy=CapabilityManifest().resolve(),
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        linked=False,
        require_linked=False,
        linked_output_path=None,
        output_artifact=output_wasm,
        json_output=True,
        runtime_state=_RuntimeArtifactState(),
        ensure_runtime_wasm_both=ensure_pair,
        runtime_cargo_profile="release",
        molt_root=tmp_path,
        wasm_facts_scanner=tmp_path / "molt-wasm-facts",
    )

    assert calls == [{"molt_PyA"}]
    assert err == 2


def _prepare_host_precompile_routing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    fixture_root: RuntimeFixtureRoot,
    outcome: str,
    verify_reuse: bool = False,
    precompile: bool = True,
    freestanding: bool = False,
    runtime_profile: str = "release",
) -> tuple[_PreparedNonNativeResult | None, int | None, list[str], Path]:
    """Run the real private deployment and host receipt admission boundary."""
    from molt import artifact_publication

    monkeypatch.delenv("MOLT_WASM_PRECOMPILED_PATH", raising=False)
    monkeypatch.delenv("MOLT_WASM_PRECOMPILED_RUNTIME_PATH", raising=False)
    linked = outcome != "unlinked"
    output = tmp_path / "app_out.wasm"
    linked_output = tmp_path / "output_linked.wasm"
    output.write_bytes(b"\0asm\x01\0\0\0")
    linked_output.write_bytes(b"previous linked generation")
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text('{"previous":"deployment"}\n', encoding="utf-8")
    native_path = tmp_path / "output_linked.molt.cwasm"
    native_path.write_bytes(b"previous native generation")
    pair_dir = tmp_path / "runtime-pair"
    pair_dir.mkdir()
    shared = pair_dir / "molt_runtime.wasm"
    reloc = pair_dir / "molt_runtime_reloc.wasm"
    generation = pair_dir / "runtime-generation.json"
    expected_identity = pair_dir / "runtime-expected-identity.json"
    for path in (shared, reloc):
        path.write_bytes(output.read_bytes())
    for path in (generation, expected_identity):
        path.write_text("{}", encoding="utf-8")
    state = _RuntimeArtifactState(
        runtime_wasm=shared,
        runtime_reloc_wasm=reloc,
        runtime_wasm_selected=shared,
        runtime_reloc_wasm_selected=reloc,
        runtime_wasm_generation=generation,
        runtime_wasm_expected_identity=expected_identity,
    )
    events: list[str] = []
    host_binary = tmp_path / "molt-wasm-host"
    if precompile:
        host_binary.write_bytes(b"fixture host identity")
    else:
        monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_binary))
    # The mocked link child still receives real content-admitted scanner bytes.
    # This native-image fixture proves custody, not scanner execution behavior.
    scanner = fixture_root.native_executable("molt-wasm-facts")
    optimizer_fact = mock_wasm_optimizer_cache_fact(fixture_root)
    monkeypatch.setattr(nno, "wasm_optimizer_cache_fact", lambda: optimizer_fact)

    def ensure_pair(required_exports=None) -> bool:  # noqa: ANN001
        assert required_exports == {"anchor"}
        events.append("ensure-pair")
        return True

    def imports(path: Path, namespace: str) -> set[str]:
        return {"anchor"} if path == output and namespace == "molt_runtime" else set()

    real_app_exports = nno._app_export_manifest

    def app_exports(contract, artifact):  # type: ignore[no-untyped-def]
        assert artifact != linked_output and artifact.name == linked_output.name
        events.append("app-exports")
        return real_app_exports(contract, artifact)

    def resolve_host(root: Path, *, cargo_profile: str) -> str | None:
        if not precompile:
            pytest.fail("plain WASM emission selected a precompile host")
        assert root == tmp_path and cargo_profile == runtime_profile
        events.append("resolve-host")
        return None if outcome == "missing-host" else str(host_binary)

    def run_child(command, **kwargs):  # type: ignore[no-untyped-def]
        if "--output" in command:
            assert ("--freestanding" in command) == freestanding
            assert command[command.index("--wasm-facts-scanner") + 1] == str(scanner)
            expected_inputs = [
                (Path(command[index + 1]), command[index + 2])
                for index, argument in enumerate(command)
                if argument == "--expected-input"
            ]
            assert scanner.resolve() in {path for path, _digest in expected_inputs}
            for path, digest in expected_inputs:
                assert digest == hashlib.sha256(path.read_bytes()).hexdigest()
            events.append("link")
            private = Path(command[command.index("--output") + 1])
            assert private != linked_output
            payloads = {"linked": (private, output.read_bytes())}
            payloads.update(
                mock_wasm_optimizer_publications(command, payloads, optimizer_fact)
            )
            candidates = {}
            for role, (final, payload) in payloads.items():
                stage = artifact_publication.staged_output_path(final)
                stage.write_bytes(payload)
                candidates[role] = (stage, final)
            # Outer deployment owns its receipt; direct tool invocations can
            # additionally request one. Match the child's optional protocol.
            request = (
                nno.link_fingerprints.FinalLinkReceiptRequest.read(
                    Path(command[command.index("--link-receipt-request") + 1])
                )
                if "--link-receipt-request" in command
                else None
            )
            nno.link_fingerprints.publish_link_outputs(candidates, receipt=request)
            return subprocess.CompletedProcess(command, 0, "", "")
        assert precompile, "plain WASM emission invoked a native host"
        assert command[:2] == [str(host_binary), "--precompile"]
        private_manifest = Path(command[2])
        assert private_manifest != manifest_path
        manifest = json.loads(private_manifest.read_text(encoding="utf-8"))
        private_linked = private_manifest.parent / manifest["modules"]["linked"]["path"]
        assert (
            manifest["modules"]["linked"]["sha256"]
            == hashlib.sha256(private_linked.read_bytes()).hexdigest()
        )
        assert manifest["abi"]["app_exports"]["bindings"] == []
        assert kwargs["cwd"] == tmp_path and kwargs["timeout"] == 60
        private_native = Path(kwargs["env"]["MOLT_WASM_PRECOMPILED_PATH"])
        assert private_native != native_path
        events.append("invoke-host")
        # Even a partially successful host may only modify this invocation.
        payload = b"opaque host-produced native container"
        private_native.parent.mkdir(parents=True, exist_ok=True)
        private_native.write_bytes(payload)
        if outcome == "child-error":
            return subprocess.CompletedProcess(
                command, 1, "", "host compile rejected input"
            )
        if outcome == "timeout":
            raise subprocess.TimeoutExpired(
                command, 60, stderr=b"host compiler stalled"
            )
        assert outcome == "success"
        receipt = {
            "version": 1,
            "kind": "molt-wasm-precompile",
            "artifacts": {
                "main": {
                    "source": str(private_linked),
                    "path": str(private_native),
                    "source_sha256": hashlib.sha256(
                        private_linked.read_bytes()
                    ).hexdigest(),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                    "size": len(payload),
                }
            },
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(receipt), "")

    monkeypatch.setattr(nno, "_collect_wasm_module_import_names", imports)
    monkeypatch.setattr(nno, "_validate_wasm_structural", lambda _path: None)
    monkeypatch.setattr(
        nno,
        "local_python_import_closure",
        lambda *_args: LocalPythonSourceClosure(
            paths=(),
            source_sha256={},
            content_digest=hashlib.sha256(b"").hexdigest(),
            source_bytes=0,
        ),
    )
    monkeypatch.setattr(
        nno, "_wasm_export_function_signatures", lambda *_args, **_kwargs: {}
    )
    monkeypatch.setattr(nno, "_app_export_manifest", app_exports)
    monkeypatch.setattr(nno, "resolve_molt_wasm_host_binary", resolve_host)
    monkeypatch.setattr(nno, "_run_completed_command", run_child)
    # Host routing is under test, not toolchain selection: bind a hermetic
    # wasm-ld so the routing fixture is independent of the local WASI SDK state.
    hermetic_linker = write_mock_executable(
        tmp_path / ("wasm-ld.exe" if os.name == "nt" else "wasm-ld"), b"wasm-ld"
    )
    monkeypatch.setattr(
        nno.wasm_toolchain,
        "resolve_wasm_linker",
        lambda: nno.wasm_toolchain.WasmLinkerIdentity(
            hermetic_linker,
            "22.1.0",
            None,
            hashlib.sha256(b"wasm-ld").hexdigest(),
        ),
    )
    build_kwargs = dict(
        resolved_capability_policy=CapabilityManifest().resolve(),
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=True,
        is_wasm_freestanding=freestanding,
        linked=linked,
        require_linked=False,
        linked_output_path=linked_output if linked else None,
        output_artifact=output,
        json_output=True,
        runtime_state=state,
        ensure_runtime_wasm_both=ensure_pair,
        runtime_cargo_profile=runtime_profile,
        molt_root=tmp_path,
        precompile=precompile,
        wasm_facts_scanner=scanner,
        app_export_contract_path=_empty_app_export_contract(tmp_path),
    )
    prepared, error = nno._prepare_non_native_build_result(**build_kwargs)
    if verify_reuse:
        assert error is None and prepared is not None
        before = {
            path: path.stat().st_mtime_ns
            for path in (
                linked_output,
                manifest_path,
                *((native_path,) if precompile else ()),
            )
        }
        if not precompile:
            # An unused host selection cannot become a deployment cache input.
            monkeypatch.setenv(
                "MOLT_WASM_HOST_BIN", str(tmp_path / "other-absent-host")
            )
        event_count = len(events)
        reused, reuse_error = nno._prepare_non_native_build_result(**build_kwargs)
        assert reuse_error is None and reused is not None
        assert events[event_count:] == [
            "ensure-pair",
            *(("resolve-host",) if precompile else ()),
        ]
        assert {path: path.stat().st_mtime_ns for path in before} == before
        if precompile:
            native_path.write_bytes(b"tampered host container")
            repaired, repair_error = nno._prepare_non_native_build_result(
                **build_kwargs
            )
            assert repair_error is None and repaired is not None
            assert events.count("invoke-host") == 2
            assert native_path.read_bytes() == b"opaque host-produced native container"
        else:
            assert not native_path.exists()
    assert events and events[0] == "ensure-pair"
    if error is not None:
        assert linked_output.read_bytes() == b"previous linked generation"
        assert (
            manifest_path.read_text(encoding="utf-8") == '{"previous":"deployment"}\n'
        )
        assert native_path.read_bytes() == b"previous native generation"
    return prepared, error, events, native_path


@pytest.mark.parametrize("freestanding", [False, True])
@pytest.mark.parametrize("runtime_profile", ["dev-fast", "release-output"])
def test_plain_wasm_deployment_and_reuse_never_select_a_precompile_host(
    isolated_molt_cache: Path,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_fixture_root: RuntimeFixtureRoot,
    freestanding: bool,
    runtime_profile: str,
) -> None:
    prepared, error, events, native_path = _prepare_host_precompile_routing(
        monkeypatch,
        tmp_path,
        fixture_root=runtime_fixture_root,
        outcome="success",
        precompile=False,
        freestanding=freestanding,
        runtime_profile=runtime_profile,
        verify_reuse=True,
    )
    assert error is None and prepared is not None
    assert events == ["ensure-pair", "link", "app-exports", "ensure-pair"]
    assert prepared.consumer_output == tmp_path / "output_linked.wasm"
    assert prepared.consumer_output.read_bytes() == b"\0asm\x01\0\0\0"
    assert prepared.artifacts is not None
    assert prepared.artifacts["manifest"] == str(tmp_path / "manifest.json")
    assert "cwasm" not in prepared.artifacts
    assert "cwasm_output" not in prepared.extra_fields
    assert not native_path.exists()


def test_precompile_build_routes_linked_manifest_to_host_and_consumes_receipt(
    isolated_molt_cache: Path,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_fixture_root: RuntimeFixtureRoot,
) -> None:
    prepared, error, events, native_path = _prepare_host_precompile_routing(
        monkeypatch, tmp_path, fixture_root=runtime_fixture_root, outcome="success"
    )
    assert error is None
    assert prepared is not None
    assert events == [
        "ensure-pair",
        "resolve-host",
        "link",
        "app-exports",
        "invoke-host",
    ]
    assert prepared.artifacts is not None
    assert prepared.artifacts["cwasm"] == str(native_path)
    assert prepared.artifacts["manifest"] == str(tmp_path / "manifest.json")
    assert prepared.extra_fields["cwasm_output"] == str(native_path)
    assert prepared.consumer_output == tmp_path / "output_linked.wasm"
    assert f"Precompiled {native_path}" in prepared.success_messages


def test_precompile_deployment_cache_covers_host_outputs_without_rewriting(
    isolated_molt_cache: Path,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_fixture_root: RuntimeFixtureRoot,
) -> None:
    _prepare_host_precompile_routing(
        monkeypatch,
        tmp_path,
        fixture_root=runtime_fixture_root,
        outcome="success",
        verify_reuse=True,
    )


@pytest.mark.parametrize(
    "outcome,diagnostic,tail",
    [
        ("missing-host", "requires a matching molt-wasm-host binary", ["resolve-host"]),
        (
            "child-error",
            "host compile rejected input",
            ["resolve-host", "link", "app-exports", "invoke-host"],
        ),
        (
            "timeout",
            "timed out after 60 seconds: host compiler stalled",
            ["resolve-host", "link", "app-exports", "invoke-host"],
        ),
        (
            "unlinked",
            "requires linked or split-runtime output with a canonical manifest",
            [],
        ),
    ],
)
def test_precompile_build_failures_reach_exact_routing_boundary(
    isolated_molt_cache: Path,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_fixture_root: RuntimeFixtureRoot,
    capsys: pytest.CaptureFixture[str],
    outcome: str,
    diagnostic: str,
    tail: list[str],
) -> None:
    prepared, error, events, native_path = _prepare_host_precompile_routing(
        monkeypatch, tmp_path, fixture_root=runtime_fixture_root, outcome=outcome
    )
    assert prepared is None and error == 2
    expected = ["ensure-pair"]
    assert events == expected + tail, "test did not reach its intended failure boundary"
    report = json.loads(capsys.readouterr().out)
    assert report["status"] == "error" and report["command"] == "build"
    assert len(report["errors"]) == 1 and diagnostic in report["errors"][0]
    assert native_path.read_bytes() == b"previous native generation"
    assert (tmp_path / "manifest.json").read_text(
        encoding="utf-8"
    ) == '{"previous":"deployment"}\n'


def _observed_pair_fixture(tmp_path: Path, *, flags: int = 0):
    """Real modules with a typed add export and an independent linking symbol."""
    from molt._wasm_abi_generated import (
        WASM_RESERVED_RUNTIME_CALLABLE_BASE,
        WASM_RESERVED_RUNTIME_CALLABLES,
    )
    from molt.wasm_artifact import _build_wasm_sections
    from tests.wasm_callable_table_fixtures import _wasm_string, _wasm_u32

    ctx = _test_pair_context(tmp_path, required_exports={"add"})
    shared = tmp_path / "shared-source.wasm"
    reloc = tmp_path / "reloc-source.wasm"
    imports = (
        b"\x02"
        + _wasm_string("env")
        + _wasm_string("__indirect_function_table")
        + b"\x01\x70\x00\x80\x02"
        + _wasm_string("env")
        + _wasm_string("memory")
        + b"\x02\x00\x02"
    )
    prefix = WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * len(
        WASM_RESERVED_RUNTIME_CALLABLES
    )
    signature = b"\x01\x60\x02\x7e\x7e\x01\x7e"
    code = b"\x01\x04\x00\x42\x00\x0b"
    shared.write_bytes(
        _build_wasm_sections(
            [
                (1, signature),
                (2, imports),
                (3, b"\x01\x00"),
                (7, b"\x01" + _wasm_string("molt_add") + b"\x00\x00"),
                (9, b"\x01\x00\x41\x01\x0b" + _wasm_u32(prefix) + bytes(prefix)),
                (10, code),
            ]
        )
    )
    # A valid extended-const global initializer irrelevant to reloc admission
    # and split layout. It previously tripped the eager global parser.
    reloc_imports = (
        b"\x04"
        + imports[1:]
        + _wasm_string("env")
        + _wasm_string("base")
        + b"\x03\x7f\x00"
        + _wasm_string("env")
        + _wasm_string("external_add")
        + b"\x00\x00"
    )
    symbol_index = 0 if flags & 0x10 else 1
    symbol = (
        b"\x01\x00"
        + _wasm_u32(flags)
        + _wasm_u32(symbol_index)
        + _wasm_string("molt_add")
    )
    linking = _wasm_string("linking") + b"\x02\x08" + _wasm_u32(len(symbol)) + symbol
    reloc.write_bytes(
        _build_wasm_sections(
            [
                (1, signature),
                (2, reloc_imports),
                (3, b"\x01\x00"),
                (6, b"\x01\x7f\x00\x23\x00\x41\x01\x6a\x0b"),
                (10, code),
                (0, linking),
            ]
        )
    )
    generation = publish_runtime_wasm_generation(
        ctx.runtime_wasm,
        ctx.runtime_reloc_wasm,
        shared_identity=ctx.pre_identity.shared,
        reloc_identity=ctx.pre_identity.reloc,
        source_shared=shared,
        source_reloc=reloc,
    )
    return ctx, generation


@pytest.mark.parametrize("flags, accepted", [(0, True), (2, False), (0x50, False)])
def test_generation_admission_and_diagnostic_share_typed_linking_obligations(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, flags: int, accepted: bool
) -> None:
    from molt.cli import runtime_wasm_validation

    ctx, generation = _observed_pair_fixture(tmp_path, flags=flags)
    # Structural wasm-tools execution is outside the observation/admission claim.
    monkeypatch.setattr(
        runtime_wasm_validation, "_validate_wasm_structural", lambda _: None
    )
    report = runtime_wasm_validation.runtime_wasm_generation_admission(
        generation, {"add"}
    )
    assert report.accepted is accepted
    assert report.shared_missing_exports == ()
    assert report.reloc_missing_symbols == (() if accepted else ("molt_add",))
    selected = ctx.accept_generation(observed_generation=generation)
    assert (selected is generation) is accepted
    assert ctx.generation_rejection_details() == report.details()


def test_split_layout_ignores_reloc_sections_and_source_binding_reuses_parsing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import toolchain_identity
    from molt.cli import runtime_wasm_generation, runtime_wasm_validation
    from molt.cli.wasm_codegen_layout import prepare_wasm_codegen_layout
    from molt import wasm_linking_symbols

    ctx, generation = _observed_pair_fixture(tmp_path)
    reads, structural, linking = [], [], []
    read = toolchain_identity.read_stable_regular_file
    scan = wasm_linking_symbols.wasm_linking_defined_names

    def capture(identity, **kwargs):
        reads.append(identity.path)
        return read(identity, **kwargs)

    def structure(path):
        structural.append(path)
        return None

    def names(path, expected, **kwargs):
        linking.append(dict(expected))
        return scan(path, expected, **kwargs)

    monkeypatch.setattr(toolchain_identity, "read_stable_regular_file", capture)
    monkeypatch.setattr(runtime_wasm_validation, "_validate_wasm_structural", structure)
    monkeypatch.setattr(wasm_linking_symbols, "wasm_linking_defined_names", names)
    binding = runtime_wasm_generation.bind_runtime_wasm_codegen(generation, {"add"})
    layout = prepare_wasm_codegen_layout(binding, linked=True, split_runtime=True)
    assert layout.table_base == 1
    assert reads == [generation.shared]
    assert (
        ctx.accept_generation(observed_generation=binding.generation)
        is binding.generation
    )
    assert reads == [generation.shared, generation.reloc]
    # Once admitted, a replacement of the mutable pointer cannot redirect bind
    # or force final admission to consume a second, unadmitted generation.
    generation.manifest.write_bytes(b"corrupt mutable pointer")
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "read_runtime_wasm_generation",
        lambda *args, **kwargs: pytest.fail(
            "final admission reopened mutable selection"
        ),
    )
    repinned = runtime_wasm_generation.bind_runtime_wasm_codegen(
        ctx.accepted_generation, {"add"}
    )
    assert (
        prepare_wasm_codegen_layout(repinned, linked=True, split_runtime=True) == layout
    )
    repinned.verify()
    assert (
        ctx.accept_generation(observed_generation=repinned.generation)
        is repinned.generation
    )
    assert reads == [generation.shared, generation.reloc]
    assert structural == [generation.shared, generation.reloc]
    assert linking == [{"molt_add": "function"}]


@pytest.mark.parametrize("consumer", ["facts", "linking", "structure"])
def test_runtime_fact_caches_reject_changed_content_with_matching_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, consumer: str
) -> None:
    from dataclasses import replace
    from molt import toolchain_identity
    from molt.cli import runtime_wasm_validation

    _ctx, generation = _observed_pair_fixture(tmp_path)
    monkeypatch.setattr(
        runtime_wasm_validation, "_validate_wasm_structural", lambda path: None
    )
    if consumer == "facts":
        consume = generation.facts
        field = "shared_member_identity"
    elif consumer == "linking":

        def consume():
            return generation.linking_names({"molt_add": "function"})

        field = "reloc_member_identity"
    else:
        consume = generation.validate_structure
        field = "shared_member_identity"
    consume()
    old = getattr(generation, field)
    before = old.path.stat()
    data = bytearray(old.path.read_bytes())
    data[-1] ^= 1
    old.path.write_bytes(data)
    os.utime(old.path, ns=(before.st_atime_ns, before.st_mtime_ns))
    current = toolchain_identity.stable_regular_file_identity(
        old.path, label="current metadata fixture"
    )
    # Keep the old digest and cached facts while modeling a permitted equal
    # metadata observation. Rejection must come from the actual bytes.
    object.__setattr__(generation, field, replace(current, sha256=old.sha256))
    with pytest.raises(ValueError, match="content changed"):
        consume()


def test_generation_diagnostic_preserves_failed_observation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ctx, generation = _observed_pair_fixture(tmp_path)
    generation.reloc.write_bytes(b"changed after capture")
    assert ctx.accept_generation(observed_generation=generation) is None
    details = ctx.generation_rejection_details()
    assert details["issues"][0]["reason"] == "observation"
    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "read_runtime_wasm_generation",
        lambda *args, **kwargs: pytest.fail(
            "diagnostics observed a different generation"
        ),
    )
    assert ctx.generation_rejection_details() == details


def test_generation_linking_parse_failure_has_one_admission_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import runtime_wasm_validation
    from molt.wasm_artifact import _build_wasm_sections, parse_wasm_sections
    from tests.wasm_callable_table_fixtures import _wasm_string, _wasm_u32

    ctx, generation = _observed_pair_fixture(tmp_path)
    source = tmp_path / "bad-linking-source.wasm"
    # A declared symbol-table count without a symbol is not missing exports;
    # it is malformed linking metadata, which the diagnostic must report.
    symbol_table = b"\x01"
    linking = (
        _wasm_string("linking")
        + b"\x02\x08"
        + _wasm_u32(len(symbol_table))
        + symbol_table
    )
    source.write_bytes(
        _build_wasm_sections(
            [
                *[
                    (kind, data)
                    for kind, data in parse_wasm_sections(generation.reloc.read_bytes())
                    if kind != 0
                ],
                (0, linking),
            ]
        )
    )
    malformed = publish_runtime_wasm_generation(
        ctx.runtime_wasm,
        ctx.runtime_reloc_wasm,
        shared_identity=generation.shared_identity,
        reloc_identity=generation.reloc_identity,
        source_shared=generation.shared,
        source_reloc=source,
    )
    monkeypatch.setattr(
        runtime_wasm_validation, "_validate_wasm_structural", lambda _: None
    )
    assert ctx.accept_generation(observed_generation=malformed) is None
    details = ctx.generation_rejection_details()
    assert [(item["member"], item["reason"]) for item in details["issues"]] == [
        ("reloc", "linking")
    ]
    assert "Unexpected EOF" in details["issues"][0]["detail"]
