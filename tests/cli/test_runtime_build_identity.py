from __future__ import annotations

from molt.cli.runtime_wasm_build_support import runtime_reloc_publication_action

from molt.process_environment import ProcessEnvironmentBuilder

import os
import hashlib
import json
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import replace
from functools import partial
from pathlib import Path

import pytest

from molt import toolchain_identity
from molt.cli import runtime_build_identity as identity
from molt.cli.cargo_search_path import derives_cargo_search_path
from molt.cli import runtime_native_build
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.exact_json import canonical_json_bytes, canonical_json_sha256
from molt.source_root import compiler_source_root
from molt.wasi_sysroot import resolve_wasi_sysroot_layout
from tests.process_guard_common import current_thread_only, install_module_view
from tests.executable_test_support import native_executable_donor
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    publication_executor,
    clear_wasm_toolchain_selection,
    provisioned_wasi_sdk_fixture,
)
from molt.llvm_toolchain import (
    wasi_c_abi_plan,
    project_wasm_toolchain_environment,
    load_wasi_sdk_installation,
    LlvmToolchainConfigError,
    WasiSdkInstallation,
)
from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan
from molt.cli import runtime_cargo_plan as cargo_plans
from molt.cli import rustc_probes
from molt.cli import runtime_identity_schema as schema
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.python_runtime_identity import (
    PYTHON_RUNTIME_SELECTION_SCHEMA,
    validate_python_runtime_identity,
)
from tests.python_environment_test_support import runtime_identity_manifest
from tests.executable_test_support import (
    build_native_executable,
    native_fixture_build_timeout,
    native_executable_name,
    write_mock_executable,
)
from tests.rustc_test_support import rustc_target_metadata_output


# Fake host triple shared by every resolved test plan and its environment.
_TEST_HOST_TARGET = "test-host"


@pytest.fixture(scope="module")
def runtime_receipt() -> dict[str, object]:
    return runtime_identity_manifest()


def _sdk(root: Path):
    return provisioned_wasi_sdk_fixture(RuntimeFixtureRoot(root))


def _c_abi(root: Path):
    return wasi_c_abi_plan(_sdk(root))


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


def _provision_toolchain(
    root: Path, *, wasi_sdk: WasiSdkInstallation | None = None
) -> schema.RuntimeToolchainContentManifest:
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.update(
        {
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "MOLT_BUILD_PYTHON": sys.executable,
            "CARGO_HOME": str(root / "test-cargo-home"),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    if wasi_sdk is not None:
        env.update(project_wasm_toolchain_environment(wasi_sdk, environ={}))
    plan = _test_plan(root, env)
    return identity.provision_wasm_runtime_toolchain_content_manifest(
        project_root=root,
        env=plan.environment,
        cargo_plan=plan,
        target_triple="wasm32-wasip1",
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
    wasi_sdk: WasiSdkInstallation | None = None,
) -> schema.RuntimeBuildIdentity:
    target = None if kind == "native" else "wasm32-wasip1"
    ambient = ProcessEnvironmentBuilder(os.environ)
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
    if kind != "native" and wasi_sdk is not None:
        env.update(project_wasm_toolchain_environment(wasi_sdk, environ={}))
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
        assert plan.wasi_c_abi is not None
        return identity.resolve_wasm_cpython_abi_build_identity(
            root,
            env=plan.environment,
            cargo_plan=plan,
            cargo_profile=profile,
            target_triple="wasm32-wasip1",
            cargo_command=plan.command,
            artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
            wasi_sysroot=plan.wasi_c_abi.sysroot,
        )
    shared, reloc = identity.resolve_wasm_runtime_build_family_identities(
        root,
        env=plan.environment,
        cargo_plan=plan,
        cargo_profile=profile,
        target_triple="wasm32-wasip1",
        runtime_features=runtime_features,
        cargo_command=plan.command,
        producer_artifact_selection=artifact_selection,
        members=(
            schema.RuntimeBuildMemberPlan(
                kind="shared",
                resolved_rustflags=(
                    "-C",
                    "panic=abort",
                    "--cfg",
                    "shared",
                    *((("-C", f"link-arg=@{response_path}")) if response_path else ()),
                ),
                publication=schema.RuntimePublicationAction(
                    schema.RuntimePublicationTransform.SHARED,
                    publication == "unstripped-debug-v1",
                    identity.capture_runtime_publication_executor(
                        root, schema.RuntimePublicationTransform.SHARED
                    ),
                ),
                link_args=("--export=shared",),
            ),
            schema.RuntimeBuildMemberPlan(
                kind="reloc",
                resolved_rustflags=("-C", "panic=abort", "--cfg", "reloc"),
                publication=runtime_reloc_publication_action(
                    ("-C", "link-arg=--export=reloc"),
                    module_name="molt_runtime_reloc.wasm",
                    executor=identity.capture_runtime_publication_executor(
                        root, schema.RuntimePublicationTransform.RELOC
                    ),
                ),
                link_args=("--export=reloc",),
            ),
        ),
    )
    return shared if kind == "shared" else reloc


@pytest.fixture
def identity_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, readonly_wasi_sdk
) -> Path:
    from molt.cli.cargo_target_cfg import parse_rustc_target_metadata

    original_plan = _test_plan
    clear_wasm_toolchain_selection(monkeypatch)

    def plan_with_sdk(root, env, *, target_triple="wasm32-wasip1", **kwargs):
        if target_triple in {"wasm32-wasip1", "wasm32-unknown-unknown"} and not any(
            key in env for key in ("WASI_SDK_PATH", "WASI_SDK_PREFIX")
        ):
            env = {
                **project_wasm_toolchain_environment(readonly_wasi_sdk(), environ={}),
                **env,
            }
        return original_plan(root, env, target_triple=target_triple, **kwargs)

    monkeypatch.setattr(sys.modules[__name__], "_test_plan", plan_with_sdk)
    original_run = cargo_plans.process_guard.run_completed_command

    def tool_probe(command, **kwargs):
        if list(command) == [sys.executable, "--version"]:
            return subprocess.CompletedProcess(command, 0, "cargo 1.99.0\n", "")
        return original_run(command, **kwargs)

    install_module_view(
        monkeypatch,
        "process_guard",
        cargo_plans.process_guard,
        cargo_plans,
        run_completed_command=tool_probe,
    )

    def target_metadata(_rustc, _target, _flags, **_kwargs):
        stdout, stderr = rustc_target_metadata_output(
            tmp_path / "rust-sysroot", "unix", target=_target
        )
        return parse_rustc_target_metadata(stdout, stderr)

    monkeypatch.setattr(rustc_probes, "rust_target_metadata", target_metadata)
    monkeypatch.setattr(cargo_plans, "_rust_resource_roots", lambda *args, **kwargs: ())
    (tmp_path / "Cargo.toml").write_text(
        '[profile.release-output]\ninherits = "release"\n[profile.dev-fast]\ninherits = "dev"\n',
        encoding="utf-8",
    )
    source = tmp_path / "runtime" / "src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("pub fn runtime() {}\n", encoding="utf-8")
    for transform in schema.RuntimePublicationTransform:
        for relative in schema.runtime_publication_executor_paths(transform):
            executor = tmp_path / relative
            executor.parent.mkdir(parents=True, exist_ok=True)
            executor.write_text("# publication executor fixture\n", encoding="utf-8")
    tooling = tmp_path / "runtime-planner.py"
    tooling.write_text("# runtime planning authority\n", encoding="utf-8")
    archive_root = tmp_path / "archives"
    archive_root.mkdir()
    (archive_root / "libcompiler_builtins.rlib").write_bytes(b"rust compiler builtins")
    # This separate directory is a Rust --sysroot flag resource in relocation tests.
    (tmp_path / "wasi-sysroot").mkdir()
    monkeypatch.setattr(
        identity,
        "runtime_source_paths",
        lambda root, _features=(): (root / "runtime" / "src",),
        raising=True,
    )
    monkeypatch.setattr(
        identity,
        "wasm_cpython_abi_source_paths",
        lambda project_root: (project_root / "runtime" / "src",),
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
def test_resolvers_own_one_source_observation_per_call(
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
    assert observations == [("source/runtime/src",)]
    # A separate live admission must see files introduced after the first one.
    (identity_root / "runtime/src/new.rs").write_bytes(b"new source input")
    after_source = resolve()
    assert len(observations) == 2
    assert before.compile_digest != after_source.compile_digest
    assert (
        before.payload["family"]["members"] == after_source.payload["family"]["members"]
    )
    (identity_root / "runtime-planner.py").write_bytes(b"new publication input")
    after_tooling = resolve()
    assert len(observations) == 3
    assert after_source.compile_digest == after_tooling.compile_digest
    assert after_source == after_tooling
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


def test_identity_observes_mutable_source_and_retains_managed_sdk_generation(
    identity_root: Path,
) -> None:
    installation = _sdk(identity_root)
    before = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        wasi_sdk=installation,
    )
    (identity_root / "runtime" / "src" / "lib.rs").write_text(
        "pub fn runtime_changed() {}\n", encoding="utf-8"
    )
    after_source = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        wasi_sdk=installation,
    )
    assert after_source.digest != before.digest
    assert after_source.family_digest != before.family_digest

    load_wasi_sdk_installation(
        compiler_source_root(), installation.prefix, verify_tree=True
    )
    (_c_abi(identity_root).include / "stddef.h").write_text(
        "typedef unsigned size_t;\n", encoding="utf-8"
    )
    after_header = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        wasi_sdk=installation,
    )
    assert after_header.digest == after_source.digest
    assert after_header.family_digest == after_source.family_digest
    with pytest.raises(LlvmToolchainConfigError, match="filesystem tree differs"):
        load_wasi_sdk_installation(
            compiler_source_root(), installation.prefix, verify_tree=True
        )


def test_readonly_sdk_reuse_keeps_real_receipt_and_function_owned_inputs(
    identity_root, readonly_wasi_sdk, monkeypatch
):
    from molt import llvm_toolchain, wasi_sdk_identity

    shared = readonly_wasi_sdk()
    assert readonly_wasi_sdk() is not shared
    # Independently exercise complete production receipt/tree verification once;
    # the unit plans below must consume this same admitted generation.
    llvm_toolchain.load_wasi_sdk_installation(
        compiler_source_root(), shared.prefix, verify_tree=True
    )
    receipt = shared.prefix / ".molt-wasi-sdk.json"
    before = receipt.read_bytes()
    assert not receipt.stat().st_mode & 0o222
    assert not shared.wasm_ld.stat().st_mode & 0o222
    assert not shared.prefix.is_relative_to(identity_root)

    def forbidden_tree_capture(_root):
        pytest.fail("read-only plan republished or rehashed an SDK generation")

    for owner in (llvm_toolchain, wasi_sdk_identity):
        monkeypatch.setattr(
            owner,
            "wasi_sdk_tree_identity",
            current_thread_only(forbidden_tree_capture, owner.wasi_sdk_tree_identity),
        )
    first = _resolve(
        identity_root, kind="shared", publication="strip-final-link-metadata-v1"
    )
    repeated = _resolve(
        identity_root, kind="shared", publication="strip-final-link-metadata-v1"
    )
    assert first == repeated
    assert not (identity_root / "toolchains").exists()
    assert (identity_root / "runtime" / "src" / "lib.rs").is_file()
    assert receipt.read_bytes() == before


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


def test_unconsumed_planner_edit_preserves_complete_product_identity(
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
    assert before.family_digest == after.family_digest
    assert before.digest == after.digest


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
                "schema": "molt.runtime-build-member-identity.v5",
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
    member_type["payload"]["family"]["members"]["shared"]["publication"][
        "preserve_debug"
    ] = 1
    _reseal_runtime_build_identity(member_type)
    with pytest.raises(ValueError, match="publication|member shape"):
        schema.RuntimeBuildIdentity.from_dict(member_type)

    direct_payload = member_type["payload"]
    with pytest.raises(ValueError, match="publication|member shape"):
        schema.RuntimeBuildIdentity(
            digest=member_type["digest"],
            compile_digest=member_type["compile_digest"],
            family_digest=member_type["family_digest"],
            payload=direct_payload,
        )


@pytest.mark.parametrize(
    "relative",
    [
        "src/molt/dx.py",
        "src/molt/cli/runtime_build_identity.py",
        "src/molt/cli/runtime_wasm_build_spec.py",
        "src/molt/file_publication.py",
    ],
)
@pytest.mark.parametrize("kind", ["native", "shared", "reloc"])
def test_tooling_source_is_not_a_product_input(identity_root, relative, kind):
    before = _resolve(identity_root, kind=kind, publication="stable")
    path = identity_root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("# unrelated implementation edit\n", encoding="utf-8")
    after = _resolve(identity_root, kind=kind, publication="stable")
    assert before == after


def test_native_cpu_policy_splits_the_native_runtime_identity(
    identity_root: Path,
) -> None:
    def resolve(policy: RustCpuPolicy) -> schema.RuntimeBuildIdentity:
        flags = policy.rustflags(("-C", "panic=abort"))
        return _resolve(
            identity_root,
            kind="native",
            publication="tree-observation",
            base_rustflags=" ".join(flags),
        )

    baseline = resolve(RustCpuPolicy.BASELINE)
    native = resolve(RustCpuPolicy.NATIVE)

    # A native-arch staticlib runs only on CPUs like the build host, so it must
    # never satisfy a baseline build's admission, or the other way around.
    assert baseline.compile_digest != native.compile_digest
    assert baseline.digest != native.digest
    assert resolve(RustCpuPolicy.NATIVE).digest == native.digest


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
    expected = {"CXXFLAGS": ("-fno-rtti",)}
    for name in (
        "CFLAGS_wasm32-wasip1",
        "CFLAGS_wasm32_wasip1",
        "CXXFLAGS_wasm32-wasip1",
        "CXXFLAGS_wasm32_wasip1",
    ):
        expected[name] = (
            ("-O1", "--no-default-config")
            if name == "CFLAGS_wasm32-wasip1"
            else ("--no-default-config",)
        )
    assert (
        baseline.payload["family"]["compile"]["common_config"][
            "ambient_c_build_environment"
        ]
        == expected
    )


def test_build_script_environment_is_semantic_content_identity(
    identity_root: Path,
) -> None:
    pythonpath = identity_root / "pythonpath"
    pythonpath.mkdir()
    (pythonpath / "helper.py").write_text("VERSION = 1\n", encoding="utf-8")
    relocated = identity_root / "relocated-pythonpath"
    shutil.copytree(pythonpath, relocated)
    common = {
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": "Py_False, PyLong_Type; Py_False",
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": "Py_False",
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
    assert build_script["MOLT_WASI_C_ABI_PLAN"]["state"] == "selected"
    assert set(build_script["MOLT_WASI_C_ABI_PLAN"]["content"]["members"]) == {
        "libc",
        "long_double",
        "compiler_rt",
        "crt_command",
        "crt_reactor",
    }
    assert str(identity_root) not in json.dumps(baseline.to_dict(), sort_keys=True)

    (relocated / "helper.py").write_text("VERSION = 2\n", encoding="utf-8")
    changed_pythonpath = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={**common, "PYTHONPATH": str(relocated)},
    )
    assert changed_pythonpath == baseline
    assert "python_import_policy" not in build_script
    assert "build_python" not in build_script
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


@pytest.mark.skipif(
    not derives_cargo_search_path(), reason="Windows keeps the caller's PATH"
)
def test_cargo_plan_path_is_derived_from_its_tools_not_its_caller(
    identity_root: Path,
) -> None:
    """HF-385: `uv run molt` and `.venv/bin/molt` differ only in PATH."""
    caller_only = identity_root / "caller-only-bin"
    caller_only.mkdir()
    base = {
        "RUSTC": sys.executable,
        "CARGO": sys.executable,
        "CARGO_HOME": str(identity_root / "test-cargo-home"),
    }
    plans = [
        _test_plan(identity_root, {**base, "PATH": path}, target_triple=None)
        for path in (
            os.environ["PATH"],
            os.pathsep.join((str(caller_only), os.environ["PATH"])),
        )
    ]
    first, second = (plan.environment["PATH"] for plan in plans)
    assert first == second
    entries = first.split(os.pathsep)
    assert str(caller_only) not in entries
    # The selected rustc's directory leads; POSIX's standard utilities close it.
    assert entries[0] == str(Path(sys.executable).parent)
    standard = [entry for entry in os.confstr("CS_PATH").split(os.pathsep) if entry]
    assert [entry for entry in entries if entry in standard] == standard
    assert set(entries) <= {
        *standard,
        *(str(path.parent) for path in plans[0].tools.values()),
    }
    stale = replace(
        plans[0], environment=plans[0].environment.updated({"PATH": str(caller_only)})
    )
    with pytest.raises(ValueError, match="PATH differs from its selected tools"):
        stale.verify()


@pytest.mark.parametrize("producer", ["native", "wasm-cpython-abi"])
def test_no_plan_provisioning_uses_cargo_tools_without_python_capture(
    tmp_path, monkeypatch, runtime_fixture_root, producer, readonly_wasi_sdk
):
    from functools import partial
    from molt.python_runtime_admission import BuildPythonAdmission
    from tests.runtime_build_identity_helper import runtime_cargo_plan

    environment = {"MOLT_BUILD_PYTHON": "missing-generator-is-not-a-rust-input"}
    sdk = readonly_wasi_sdk() if producer == "wasm-cpython-abi" else None
    if sdk is not None:
        environment.update(project_wasm_toolchain_environment(sdk, environ={}))
    monkeypatch.setattr(
        identity,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )
    monkeypatch.setattr(
        BuildPythonAdmission,
        "capture",
        lambda *_a, **_k: pytest.fail("Rust toolchain capture admitted Python"),
    )
    if producer == "native":
        manifest = identity.provision_native_runtime_toolchain_content_manifest(
            project_root=tmp_path,
            env=environment,
            target_triple=None,
            cargo_command=("cargo", "rustc"),
        )
        tools = manifest.payload["toolchain"]["tools"]
    else:
        assert sdk is not None
        content = identity._wasm_compile_toolchain_content(
            project_root=tmp_path,
            env=environment,
            target_triple="wasm32-wasip1",
            wasi_sysroot=wasi_c_abi_plan(sdk).sysroot,
        )
        tools = content["tools"]
    assert {"cargo", "rustc"} <= tools.keys()
    assert "build_python" not in tools
    assert environment["MOLT_BUILD_PYTHON"] == "missing-generator-is-not-a-rust-input"


@pytest.mark.parametrize(
    "script_schema",
    [
        "molt.runtime-build-script-environment",
        "molt.cpython-abi-build-script-environment",
    ],
)
def test_build_script_identity_rejects_retired_python_fields_and_old_schema(
    script_schema,
):
    payload = {
        "schema": script_schema + ".v4",
        "MOLT_WASI_C_ABI_PLAN": {"state": "ignored-for-target"},
    }
    if script_schema == "molt.runtime-build-script-environment":
        payload.update(
            {
                "MOLT_WASM_CPYTHON_ABI_EXPORTS": "ignored-for-target",
                "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": "ignored-for-target",
            }
        )
    schema._validated_build_script_environment(payload, target="x86_64-test-native")
    for changed, message in (
        ({**payload, "schema": script_schema + ".v3"}, "schema"),
        ({**payload, "PYTHONPATH": {"state": "unset"}}, "shape"),
        ({**payload, "python_import_policy": "isolated-no-site-v1"}, "shape"),
        ({**payload, "build_python": {}}, "shape"),
    ):
        with pytest.raises(ValueError, match=message):
            schema._validated_build_script_environment(
                changed, target="x86_64-test-native"
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
    _sdk(identity_root)
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
        wasi_sdk=_sdk(identity_root),
    )
    second = _resolve(
        relocated,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        response_path=second_response,
        base_rustflags=f"-C panic=abort --sysroot={relocated / 'wasi-sysroot'}",
        wasi_sdk=_sdk(relocated),
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


def test_identity_records_flag_tokens_exactly() -> None:
    # The projection once split flag strings with a host-dependent shlex and
    # stripped '"' from every token, so `--cfg feature="std"` was recorded as
    # `feature="std` and a quoted Windows path kept its quotes.
    projection = identity._RuntimeFlagProjection.capture(())
    tokens = ("--cfg", 'feature="std"', "-C", "opt-level=3")
    assert projection.rustflags(tokens) == list(tokens)
    with pytest.raises(TypeError, match="token sequences"):
        projection.rustflags("--cfg feature=std")
    with pytest.raises(TypeError, match="tuple of tokens"):
        schema.RuntimeBuildMemberPlan(
            kind="shared",
            resolved_rustflags="-C panic=abort",  # type: ignore[arg-type]
            link_args=(),
            publication=schema.RuntimePublicationAction(
                schema.RuntimePublicationTransform.SHARED,
                False,
                publication_executor(schema.RuntimePublicationTransform.SHARED),
            ),
        )


@pytest.mark.parametrize("joined", [False, True])
def test_remap_projection_preserves_textual_matching_and_semantics(tmp_path, joined):
    root = tmp_path / "source = tree"
    projection = identity._RuntimeFlagProjection.capture((("source", root),))

    def remap(source, destination):
        operand = f"{source}={destination}"
        return (
            ("--remap-path-prefix=" + operand,)
            if joined
            else ("--remap-path-prefix", operand)
        )

    first = remap(root, "/molt/source")
    second = remap(root / "runtime", "/molt/runtime")
    expected = [
        "--remap-path-prefix=${source}=/molt/source",
        "--remap-path-prefix=${source}" + os.sep + "runtime=/molt/runtime",
    ]
    assert projection.rustflags((*first, *second)) == expected
    assert projection.rustflags(("cargo", "rustc", "--", *first)) == [
        "cargo",
        "rustc",
        "--",
        expected[0],
    ]
    assert projection.rustflags((*second, *first)) == expected[::-1]
    assert projection.rustflags(remap(root, "/different")) != expected[:1]
    assert projection.rustflags(remap("${source}", "/molt/source")) != expected[:1]
    assert projection.rustflags((*first, "--remap-path-scope=debuginfo")) != (
        projection.rustflags((*first, "--remap-path-scope=all"))
    )
    # rustc does textual replacement, not filesystem normalization. These
    # sources can match different strings even if they resolve to one path.
    dotted = os.fspath(root) + os.sep + "runtime" + os.sep + ".."
    assert projection.rustflags(remap(dotted, "/molt/source")) == [
        "--remap-path-prefix=${source}"
        + os.sep
        + "runtime"
        + os.sep
        + "..=/molt/source"
    ]
    with pytest.raises(ValueError, match="unknown absolute host path in Rust remap"):
        projection.rustflags(remap(os.fspath(root) + "-other", "/molt/source"))
    # A destination is semantic text, including an absolute-looking string or
    # '@'; it is neither a host input nor a linker response file.
    assert projection.rustflags(remap(root, "/unowned/@destination")) == [
        "--remap-path-prefix=${source}=/unowned/@destination"
    ]


def test_remap_projection_does_not_fold_windows_case_or_separators():
    # The exact textual root is intentional; this exercises Windows spelling
    # without pretending the host can resolve a foreign filesystem.
    projection = identity._RuntimeFlagProjection(
        roots=(("source", Path("unused"), r"C:\owned\Source"),)
    )
    assert projection.rustflags((r"--remap-path-prefix=C:\owned\Source=/molt",)) == [
        "--remap-path-prefix=${source}=/molt"
    ]
    for source in (r"C:\owned\source", "C:/owned/Source"):
        with pytest.raises(
            ValueError, match="unknown absolute host path in Rust remap"
        ):
            projection.rustflags((f"--remap-path-prefix={source}=/molt",))


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "cpython-abi"])
def test_runtime_source_and_build_remaps_relocate_without_losing_flags(
    identity_root, tmp_path_factory, kind
):
    relocated = tmp_path_factory.mktemp("remap= second source") / "source"
    shutil.copytree(identity_root, relocated)
    results = []
    for root in (identity_root, relocated):
        # Owned target/config flags must survive. Applying an encoded env
        # override before Cargo selection would silently lose this cfg.
        config = root / ".cargo" / "config.toml"
        config.parent.mkdir(exist_ok=True)
        target = _TEST_HOST_TARGET if kind == "native" else "wasm32-wasip1"
        config.write_text(
            f'[target.{target}]\nrustflags = ["--cfg", "owned_configuration"]\n',
            encoding="utf-8",
        )
        # _resolve deliberately selects RUSTFLAGS; the plan-level control
        # below separately checks the configuration-only selection.
        results.append(
            _resolve(
                root,
                kind=kind,
                publication="remap-source-build",
                extra_env={
                    "CARGO_TARGET_DIR": str(root.parent / (root.name + "-build"))
                },
            )
        )
    assert results[0] == results[1]
    encoded = json.dumps(results[0].to_dict(), sort_keys=True)
    assert str(identity_root) not in encoded and str(relocated) not in encoded
    family = results[0].payload["family"]
    flags = (
        family["members"]["staticlib"]["resolved_rustflags"]
        if kind == "cpython-abi"
        else family["compile"]["common_config"]["base_rustflags"]
    )
    assert "--remap-path-prefix=${source}=/molt/source" in flags
    assert "--remap-path-prefix=${cargo-build}=/molt/build" in flags


@pytest.mark.parametrize("build_location", ["inside", "outside", "same", "ancestor"])
def test_runtime_remap_order_and_cargo_configuration_selection(
    identity_root, monkeypatch, build_location
):
    from molt.flag_strings import FlagStringRule

    monkeypatch.delenv("RUSTFLAGS", raising=False)
    monkeypatch.delenv("CARGO_ENCODED_RUSTFLAGS", raising=False)
    config = identity_root / ".cargo" / "config.toml"
    config.parent.mkdir(exist_ok=True)
    config.write_text(
        '[target.wasm32-wasip1]\nrustflags = ["--cfg", "owned_configuration"]\n',
        encoding="utf-8",
    )
    build = {
        "inside": identity_root / "target",
        "outside": identity_root.parent / "outside-build",
        "same": identity_root,
        "ancestor": identity_root.parent,
    }[build_location]
    plan = _test_plan(
        identity_root,
        {
            **os.environ,
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "CARGO_HOME": str(identity_root / "cargo-home"),
            "CARGO_TARGET_DIR": str(build),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        },
    )
    assert ("--cfg", "owned_configuration") in tuple(
        zip(plan.rustflags, plan.rustflags[1:])
    )
    remaps = [
        flag for flag in plan.rustflags if flag.startswith("--remap-path-prefix=")
    ]
    source_map = f"--remap-path-prefix={identity_root}=/molt/source"
    build_map = f"--remap-path-prefix={build}=/molt/build"
    if build_location in {"same", "ancestor"}:
        assert remaps == [build_map, source_map]
    else:
        # Outside roots do not overlap; their relative order has no effect.
        assert set(remaps) == {source_map, build_map}
        if build_location == "inside":
            assert remaps == [source_map, build_map]
    assert (
        FlagStringRule.CARGO_ENCODED.decode(plan.environment["CARGO_ENCODED_RUSTFLAGS"])
        == plan.rustflags
    )
    plan.verify()


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

    monkeypatch.setattr(Path, "resolve", current_thread_only(resolve, original))
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
    # Toolchain identity runs the version probe.
    install_module_view(
        monkeypatch,
        "subprocess",
        subprocess,
        toolchain_identity,
        run=lambda *_args, **_kwargs: type(
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

    monkeypatch.setattr(identity.host_capacity, "worker_count", resource_ceiling)

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
        identity.host_capacity, "worker_count", lambda **_kwargs: 10_000
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
    foreign = tmp_path / "foreign.bin"
    foreign.write_bytes(b"unrelated descriptor traffic")
    opens = []
    open_descriptor = toolchain_identity.open_stable_read_descriptor

    def counted_open(path: Path) -> int:
        if Path(path) == config:
            opens.append(path)
        return open_descriptor(path)

    monkeypatch.setattr(toolchain_identity, "open_stable_read_descriptor", counted_open)
    with toolchain_identity.open_stable_regular_file(
        foreign, label="foreign"
    ) as opened:
        assert opened.stream.read() == b"unrelated descriptor traffic"
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
def test_executable_identity_preserves_role_selecting_symlink(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # lld selects its flavor from argv[0]: `wasm-ld` is a role name for the
    # generic driver. Identity must probe the role spelling, never the link
    # target, or the generic driver refuses to run and the tool is misrecorded.
    cold_timeout = native_fixture_build_timeout()
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
        timeout=cold_timeout,
    )
    role = tmp_path / native_executable_name("wasm-ld")
    try:
        role.symlink_to(generic)
    except OSError as exc:  # Windows without the symlink privilege
        pytest.skip(f"host cannot create file symlinks: {exc}")

    # This is the image's first correctness execution, including OS assessment.
    # Keep the actual probe and its argv/environment; product defaults stay 30s.
    monkeypatch.setattr(
        identity,
        "probe_executable",
        partial(identity.probe_executable, timeout=cold_timeout),
    )
    tool = identity._executable_identity(
        "wasm-ld",
        "wasm-ld",
        env={"PATH": str(tmp_path)},
    )

    assert identity._command_path("wasm-ld", {"PATH": str(tmp_path)}) == role
    assert tool["version"] == "22.1.8"


@pytest.mark.parametrize("resource", ["archive", "sysroot", "configuration"])
def test_portable_manifest_is_evidence_not_live_capture_authority(
    identity_root: Path, monkeypatch: pytest.MonkeyPatch, resource: str
) -> None:
    installation = _sdk(identity_root)
    before = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        wasi_sdk=installation,
    )
    manifest_path = identity_root / "toolchain.json"
    before.toolchain_manifest.write(manifest_path)
    restored = schema.RuntimeToolchainContentManifest.read(manifest_path)
    assert restored == before.toolchain_manifest
    if resource == "archive":
        (_c_abi(identity_root).path("libc")).write_bytes(b"changed")
    elif resource == "sysroot":
        (_c_abi(identity_root).include / "errno.h").write_text(
            "#define WASI_ERRNO 2\\n", encoding="utf-8"
        )
    else:
        config = identity_root / ".cargo" / "config.toml"
        config.parent.mkdir(exist_ok=True)
        config.write_text("[build]\nincremental = false\n", encoding="utf-8")
    after = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        wasi_sdk=installation,
    )
    if resource == "configuration":
        assert after.toolchain_manifest != restored
        assert after.compile_digest != before.compile_digest
    else:
        assert after.toolchain_manifest == restored
        assert after.compile_digest == before.compile_digest
        with pytest.raises(LlvmToolchainConfigError, match="filesystem tree differs"):
            load_wasi_sdk_installation(
                compiler_source_root(), _sdk(identity_root).prefix, verify_tree=True
            )


def test_managed_manifest_keeps_provisioned_generation_until_explicit_verification(
    identity_root: Path,
) -> None:
    installation = _sdk(identity_root)
    before = _provision_toolchain(identity_root, wasi_sdk=installation)
    archive = _c_abi(identity_root).path("libc")
    original = archive.read_bytes()
    archive.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))

    after = _provision_toolchain(identity_root, wasi_sdk=installation)

    assert after.digest == before.digest
    with pytest.raises(LlvmToolchainConfigError, match="filesystem tree differs"):
        load_wasi_sdk_installation(
            compiler_source_root(), _sdk(identity_root).prefix, verify_tree=True
        )


def test_toolchain_manifest_is_relocatable_and_rejects_tampering(
    identity_root: Path, tmp_path: Path
) -> None:
    _sdk(identity_root)
    relocated = tmp_path / "relocated-toolchain"
    shutil.copytree(identity_root, relocated)
    first = _provision_toolchain(identity_root, wasi_sdk=_sdk(identity_root))
    second = _provision_toolchain(relocated, wasi_sdk=_sdk(relocated))
    assert first == second
    portable = json.dumps(first.to_dict(), sort_keys=True)
    assert str(identity_root) not in portable
    assert str(relocated) not in portable

    value = first.to_dict()
    value["payload"]["target_triple"] = "wasm32-poison"
    with pytest.raises(ValueError, match="digest"):
        schema.RuntimeToolchainContentManifest.from_dict(value)


def test_toolchain_manifest_rejects_resealed_retired_python_tool(identity_root):
    value = _provision_toolchain(identity_root).to_dict()
    payload = value["payload"]
    payload["toolchain"]["tools"]["build_python"] = {}
    value["digest"] = schema._digest(payload)
    with pytest.raises(ValueError, match="tools are invalid"):
        schema.RuntimeToolchainContentManifest.from_dict(value)


def test_toolchain_manifest_rejects_nested_mutation_at_consumption(
    identity_root: Path,
) -> None:
    manifest = _provision_toolchain(identity_root)
    with pytest.raises(TypeError):
        manifest.payload["target_triple"] = "wasm32-poison"


def test_toolchain_manifest_concurrent_publication_is_atomic(
    identity_root: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first = _provision_toolchain(identity_root)
    config = identity_root / ".cargo" / "config.toml"
    config.parent.mkdir(exist_ok=True)
    config.write_text("[build]\nincremental = false\n", encoding="utf-8")
    second = _provision_toolchain(identity_root)
    assert first.digest != second.digest
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
        if self.owner.respond_to_verify:
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
            "schema": PYTHON_RUNTIME_SELECTION_SCHEMA,
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
        self.respond_to_verify = True
        self.wait_timeouts = []
        self.verifier_prefixes = []
        self.cancellation_requests = 0

    def poll(self):
        return self.returncode

    def request_cancel(self):
        self.cancellation_requests += 1

    def wait(self, timeout=None):
        # The guarded command's own wait: closure follows the child's exit.
        self.wait_timeouts.append(timeout)
        assert self.closed
        assert self.verifier_prefixes[0].is_dir()
        self.returncode = self.returncode or 0
        self.terminal = True
        return self.returncode


@pytest.fixture
def python_session(tmp_path, monkeypatch, runtime_receipt):
    from molt import python_runtime_admission as admission

    selected = tmp_path / "selected-python.exe"
    write_mock_executable(selected, b"selected-interpreter-launcher")
    session = _CaptureSession(runtime_receipt)
    calls = []

    def live_empty_prefix(command):
        option = command[command.index("-X") + 1]
        assert option.startswith("pycache_prefix=")
        prefix = Path(option.split("=", 1)[1])
        assert prefix.is_dir() and not prefix.is_symlink()
        assert list(prefix.iterdir()) == []
        assert prefix not in session.verifier_prefixes
        session.verifier_prefixes.append(prefix)
        return prefix

    class Executor:
        def start_guarded(self, command, **kwargs):
            live_empty_prefix(command)
            calls.append((command, kwargs))
            return session

        def run(self, command, **kwargs):
            from types import SimpleNamespace

            assert session.verifier_prefixes[0].is_dir()
            live_empty_prefix(command)
            session.fresh_calls.append((command, kwargs))
            return SimpleNamespace(stdout=json.dumps(session.fresh_selection))

    monkeypatch.setattr(
        admission.process_guard, "source_command_executor", lambda _prefix: Executor()
    )
    return admission, selected, session, calls


def test_python_admission_uses_exact_isolated_facade(python_session, runtime_receipt):
    from molt.process_environment import ProcessEnvironment

    admission, selected, session, calls = python_session
    environment = {
        "MOLT_BUILD_PYTHON": str(selected),
        "PYTHON": "ignored-python",
        "PATH": "captured-path",
    }
    with admission.BuildPythonAdmission() as owner:
        first = owner.capture(environment)
        assert session.verifier_prefixes[0].is_dir()
        first["runtime"]["version"] = "caller mutation"
        second = owner.capture(environment)
        assert session.verifier_prefixes[0].is_dir()
        assert not session.verifier_prefixes[1].exists()
    assert session.closed
    assert all(not prefix.exists() for prefix in session.verifier_prefixes)
    assert session.verifications == 1
    assert len(session.fresh_calls) == 1
    assert session.fresh_calls[0][0][-1] == "--runtime-selection"
    assert session.fresh_calls[0][1]["env"] == ProcessEnvironment(environment)
    assert len(calls) == 1
    assert second["runtime"] == runtime_receipt
    assert second["identity_sha256"] == canonical_json_sha256(
        {key: value for key, value in second.items() if key != "identity_sha256"}
    )
    assert calls[0][0] == [
        str(selected),
        "-B",
        "-I",
        "-X",
        f"pycache_prefix={session.verifier_prefixes[0]}",
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
    assert calls[0][1]["env"] == ProcessEnvironment(environment)
    with pytest.raises(ValueError, match="revoked"):
        owner.capture(environment)


@pytest.mark.parametrize("windows_names", [False, True])
def test_build_python_admission_preserves_name_rule_at_both_launches(
    python_session, windows_names
):
    from molt.process_environment import NameRule, ProcessEnvironment

    admission, selected, session, calls = python_session
    rule = NameRule.WINDOWS if windows_names else NameRule.EXACT
    environment = ProcessEnvironment(
        {
            "MOLT_BUILD_PYTHON": str(selected),
            "Path": "distinct alias",
            "PATH": "original",
        },
        rule=rule,
    )
    with admission.BuildPythonAdmission() as owner:
        owner.capture(environment)
        owner.capture(environment)
    assert calls[0][1]["env"] == environment
    assert session.fresh_calls[0][1]["env"] == environment
    assert calls[0][1]["env"].rule is session.fresh_calls[0][1]["env"].rule is rule
    assert session.verifications == 1


@pytest.mark.parametrize("windows_names", [False, True])
def test_build_python_loader_policy_distinguishes_posix_path_alias(
    python_session, windows_names
):
    from molt.process_environment import NameRule, ProcessEnvironment

    admission, selected, session, _calls = python_session
    environment = ProcessEnvironment(
        {"MOLT_BUILD_PYTHON": str(selected), "PATH": "original"},
        rule=NameRule.WINDOWS if windows_names else NameRule.EXACT,
    )
    with admission.BuildPythonAdmission() as owner:
        owner.capture(environment)
        changed = environment.updated({"Path": "changed"})
        if windows_names:
            with pytest.raises(ValueError, match="selection/configuration changed"):
                owner.capture(changed)
            assert session.fresh_calls == []
        else:
            owner.capture(changed)
            assert session.fresh_calls[0][1]["env"] == changed
            assert session.verifications == 1


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
        with admission.BuildPythonAdmission() as owner:
            owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
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


@pytest.mark.parametrize("phase", ["admission", "reuse"])
def test_python_identity_timeout_revokes_without_retry(
    python_session, monkeypatch, phase
):
    admission, selected, session, calls = python_session
    owner = admission.BuildPythonAdmission()
    environment = {"MOLT_BUILD_PYTHON": str(selected)}
    if phase == "admission":
        session.lines.get_nowait()
        monkeypatch.setattr(admission, "_ADMISSION_TIMEOUT", 0.001)
        expected_timeout = admission._ADMISSION_TIMEOUT
    else:
        owner.capture(environment)
        session.respond_to_verify = False
        expected_timeout = admission._RESPONSE_TIMEOUT
    response_get = owner._responses.get
    timeouts = []

    def bounded_get(*, timeout):
        timeouts.append(timeout)
        assert timeout == expected_timeout
        # Exercise the real queue-empty timeout path without waiting the full
        # reuse budget. Only this operation's response queue is intercepted.
        return response_get(timeout=0.001)

    monkeypatch.setattr(owner._responses, "get", bounded_get)
    with pytest.raises(ValueError, match="timed out"):
        owner.capture(environment)
    assert timeouts == [expected_timeout]
    assert session.verifications == (phase == "reuse")
    assert len(session.fresh_calls) == (phase == "reuse")
    if session.fresh_calls:
        assert session.fresh_calls[0][1]["timeout"] == admission._RESPONSE_TIMEOUT
    assert session.closed
    assert owner.terminal
    assert len(calls) == 1
    with pytest.raises(ValueError, match="revoked"):
        owner.capture(environment)
    assert len(calls) == 1


def test_python_identity_unresolved_interpreter_never_spawns_probe(
    python_session, tmp_path
):
    admission, _selected, _session, calls = python_session
    with pytest.raises(ValueError, match="unavailable"):
        with admission.BuildPythonAdmission() as owner:
            owner.capture({"MOLT_BUILD_PYTHON": str(tmp_path / "missing")})
    assert not calls


@pytest.mark.slow
@pytest.mark.parametrize("with_custody", [False, True])
def test_python_identity_live_content_probe(with_custody) -> None:
    """The real guarded child retains its context through two phase boundaries."""
    from molt.python_runtime_admission import BuildPythonAdmission

    environment = {**os.environ, "MOLT_BUILD_PYTHON": sys.executable}
    with BuildPythonAdmission(with_custody=with_custody) as owner:
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

    if with_custody:
        from molt.python_environment_identity import validate_python_capture

        capture = validate_python_capture(first["capture"])
        assert capture["identity"] == runtime
        assert capture["file_custody"] and capture["node_custody"]
        assert (
            first["startup_selection"]["selected_executable"]["lexical"]
            == sys.executable
        )
    else:
        assert "capture" not in first and "startup_selection" not in first


@pytest.mark.parametrize("change", ["launcher-environment", "new-native-provider"])
def test_build_python_admission_observes_fresh_startup_selection(
    python_session, change
):
    from molt.process_environment import ProcessEnvironment

    admission, selected, session, calls = python_session
    env = {"MOLT_BUILD_PYTHON": str(selected), "LD_LIBRARY_PATH": "same-directory"}
    owner = admission.BuildPythonAdmission()
    owner.capture(env)
    if change == "launcher-environment":
        env["MOLT_REAL_PYTHON"] = "different-python"
    session.fresh_selection = {
        "schema": PYTHON_RUNTIME_SELECTION_SCHEMA,
        "native": {"paths": ["different-provider"]},
    }
    with pytest.raises(ValueError, match="fresh startup selection changed"):
        owner.capture(env)
    assert len(calls) == 1
    assert session.fresh_calls[0][1]["env"] == ProcessEnvironment(env)
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
    verifier_prefix = session.verifier_prefixes[0]
    original_stdout = session.stdout

    def blocked_wait(*args, **kwargs):
        raise RuntimeError("guardian still owns live child")

    def forbidden_close():
        raise AssertionError("must not close a potentially reader-locked stream")

    monkeypatch.setattr(session, "wait", blocked_wait)
    monkeypatch.setattr(original_stdout, "close", forbidden_close)
    primary = LookupError("primary operation failed")
    with pytest.raises(LookupError) as caught:
        with owner:
            raise primary
    assert caught.value is primary
    assert owner._process is session
    assert not owner.terminal
    assert verifier_prefix.is_dir()
    assert owner.cleanup_failure.admission is owner
    assert "guardian still owns live child" in primary.__notes__[0]
    assert json.loads(capsys.readouterr().err)["kind"] == "build-python-cleanup-failure"
    monkeypatch.setattr(original_stdout, "close", lambda: None)

    def terminal_wait(*args, **kwargs):
        session.terminal = True
        session.returncode = 0
        return 0

    monkeypatch.setattr(session, "wait", terminal_wait)
    owner.close()
    assert owner.terminal
    assert not verifier_prefix.exists()


def test_build_python_verifier_prefix_outlives_response_reader(
    python_session, monkeypatch
):
    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    verifier_prefix = session.verifier_prefixes[0]
    reader = owner._reader
    assert reader is not None

    class ReaderClosure:
        complete = False

        def join(self, *, timeout):
            assert verifier_prefix.is_dir()
            reader.join(timeout=timeout)

        def is_alive(self):
            return not self.complete or reader.is_alive()

    closure = ReaderClosure()
    monkeypatch.setattr(owner, "_reader", closure)
    try:
        with pytest.raises(
            admission.BuildPythonCleanupError, match="reader remains live"
        ):
            owner.close()
        assert session.terminal and not owner.terminal
        assert verifier_prefix.is_dir()
    finally:
        closure.complete = True
        owner.close()
    assert owner.terminal and not verifier_prefix.exists()


def test_build_python_verifier_prefix_cleanup_retains_exact_owner_for_retry(
    python_session, monkeypatch, capsys
):
    from molt import temporary_artifacts

    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    allocation = owner._verifier_prefix_owner
    assert isinstance(allocation, temporary_artifacts.OwnedTemporaryDirectory)
    allocation_path = Path(allocation.name)
    verifier_prefix = session.verifier_prefixes[0]
    assert allocation_path.resolve(strict=True) == verifier_prefix
    delete = temporary_artifacts.delete_path
    attempts = []

    def fail_first_deletion(path):
        if path == allocation_path:
            attempts.append(path)
            if len(attempts) == 1:
                return False, "independent temporary deletion failure"
        return delete(path)

    monkeypatch.setattr(temporary_artifacts, "delete_path", fail_first_deletion)
    primary = LookupError("primary operation failed")
    try:
        with pytest.raises(LookupError) as caught:
            with owner:
                raise primary
        assert caught.value is primary
        assert owner.terminal and verifier_prefix.is_dir()
        assert owner._verifier_prefix_owner is allocation
        assert "independent temporary deletion failure" in primary.__notes__[0]
        diagnostic = json.loads(capsys.readouterr().err)
        assert diagnostic["kind"] == "build-python-cleanup-failure"
        owner.close()
        assert len(attempts) == 2
        assert owner._verifier_prefix_owner is None
        assert not verifier_prefix.exists()
    finally:
        monkeypatch.setattr(temporary_artifacts, "delete_path", delete)
        owner.close()


@pytest.mark.parametrize("fault", ["oversized", "missing-newline", "queue-full"])
def test_build_python_failed_reader_cancels_before_guardian_wait(
    python_session, monkeypatch, fault
):
    import threading

    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    session.lines.get_nowait()
    if fault == "oversized":
        monkeypatch.setattr(admission, "_RESPONSE_LIMIT", 128)
        session.lines.put("x" * 129 + "\n")
    elif fault == "missing-newline":
        session.lines.put("unterminated output")
    else:
        for _ in range(4):
            session.lines.put("{}\n")
    cancelled = threading.Event()
    order = []
    request_cancel = session.request_cancel
    wait = session.wait
    get_response = owner._responses.get

    def cancel():
        order.append("cancel")
        request_cancel()
        cancelled.set()

    def guardian_wait(*args, **kwargs):
        # An undrained verbose child cannot prove closure until its guardian
        # cancels it. Fail immediately rather than simulating an unbounded wait.
        assert cancelled.is_set()
        order.append("wait")
        return wait(*args, **kwargs)

    def await_reader_failure(*, timeout):
        # Let the real response thread hit the independent stream/queue fault
        # before the parent consumes anything, making queue-full deterministic.
        assert cancelled.wait(2), "failed reader did not request guard cancellation"
        return get_response(timeout=timeout)

    monkeypatch.setattr(session, "request_cancel", cancel)
    monkeypatch.setattr(session, "wait", guardian_wait)
    monkeypatch.setattr(owner._responses, "get", await_reader_failure)
    with pytest.raises(ValueError, match="identity probe failed|unsolicited output"):
        owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    assert order.index("cancel") < order.index("wait")
    assert owner.terminal
    assert not session.verifier_prefixes[0].exists()


def test_build_python_unstarted_reader_preserves_primary_and_retries(
    python_session, monkeypatch, capsys
):
    import threading

    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    start = threading.Thread.start
    primary = RuntimeError("independent reader start failure")

    def refuse_reader_start(thread):
        if thread.name == "build-python-admission-response":
            raise primary
        return start(thread)

    with monkeypatch.context() as patch:
        patch.setattr(threading.Thread, "start", refuse_reader_start)
        with pytest.raises(RuntimeError) as caught:
            owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    assert caught.value is primary
    assert session.cancellation_requests == 1
    assert session.terminal and not owner.terminal
    prefix = session.verifier_prefixes[0]
    assert prefix.is_dir()
    assert owner._reader is not None and owner._reader.ident is None
    assert owner.cleanup_failure.admission is owner
    assert "cannot join thread before it is started" in primary.__notes__[0]
    assert owner.cleanup_failure.verifier_prefix in primary.__notes__[0]
    assert Path(owner.cleanup_failure.verifier_prefix).resolve(strict=True) == prefix
    assert json.loads(capsys.readouterr().err)["kind"] == "build-python-cleanup-failure"
    # The retained real Thread can complete after its startup fault is removed.
    # The session does not start a duplicate reader on its own.
    owner._reader.start()
    owner._reader.join(timeout=5)
    assert not owner._reader.is_alive()
    owner.close()
    assert owner.terminal and not prefix.exists()


def test_build_python_unresolved_prefix_survives_owner_collection(
    python_session, monkeypatch
):
    import gc
    import weakref
    from molt import temporary_artifacts

    admission, selected, session, _calls = python_session

    def fail_guardian_wait(*args, **kwargs):
        raise RuntimeError("guardian closure remains unproved")

    monkeypatch.setattr(session, "wait", fail_guardian_wait)

    def abandoned_session():
        owner = admission.BuildPythonAdmission()
        owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
        allocation = owner._verifier_prefix_owner
        assert allocation is not None
        # Preserve the original cleanup function and allocation identity solely
        # for test teardown; no strong reference keeps the owner alive.
        cleanup = temporary_artifacts._cleanup_temporary_directory
        target = Path(allocation.name)
        allocation_identity = dict(allocation._identity)
        with pytest.raises(admission.BuildPythonCleanupError) as raised:
            owner.close()
        assert str(target) in str(raised.value)
        owner._reader.join(timeout=5)
        assert not owner._reader.is_alive()
        return (
            weakref.ref(owner),
            weakref.ref(allocation),
            target,
            cleanup,
            allocation_identity,
        )

    owner_ref, allocation_ref, target, cleanup, allocation_identity = (
        abandoned_session()
    )
    try:
        gc.collect()
        assert owner_ref() is None and allocation_ref() is None
        assert not session.terminal
        assert target.is_dir()
    finally:
        # The synthetic session has no real child. Retire with the captured
        # allocation identity, never one reconstructed from the surviving path.
        cleanup(target, allocation_identity)


def test_build_python_close_waits_for_the_helper_exit_without_a_deadline(
    python_session,
):
    """A fixed 10 s bound failed builds that had succeeded when host load
    slowed the helper's exit (HF-512). After EOF the close waits as
    `subprocess.Popen.__exit__` does."""
    admission, selected, session, _calls = python_session
    owner = admission.BuildPythonAdmission()
    owner.capture({"MOLT_BUILD_PYTHON": str(selected)})
    owner.close()
    assert session.closed and owner.terminal
    assert session.wait_timeouts == [None]


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


@pytest.mark.parametrize("batch", [False, True])
def test_native_executable_fixture_preserves_independent_images(
    runtime_fixture_root, batch
):
    source = native_executable_donor()
    original = hashlib.sha256(source.read_bytes()).hexdigest()
    names = ("native-fixture/first", "native-fixture/second")
    if batch:
        first, second = runtime_fixture_root.native_executables(names)
    else:
        first, second = (runtime_fixture_root.native_executable(name) for name in names)
    for image in (first, second):
        assert not image.is_symlink()
        assert not image.samefile(source)
        assert hashlib.sha256(image.read_bytes()).hexdigest() == original
        # Windows has no execute bits: os.stat derives them from the name's
        # extension, and these fixture names have none.
        if os.name != "nt":
            assert image.stat().st_mode & 0o111 == source.stat().st_mode & 0o111
    assert not first.samefile(second)
    with first.open("r+b") as stream:
        stream.write(b"changed fixture")
    assert hashlib.sha256(second.read_bytes()).hexdigest() == original
    assert hashlib.sha256(source.read_bytes()).hexdigest() == original
    assert runtime_fixture_root.native_executable(names[0]) == first
    assert first.read_bytes().startswith(b"changed fixture")
    first.unlink()
    first.write_bytes(b"retargeted fixture")
    assert hashlib.sha256(second.read_bytes()).hexdigest() == original
    assert hashlib.sha256(source.read_bytes()).hexdigest() == original


def test_native_executable_batch_never_reuses_mutated_donor(runtime_fixture_root):
    source = native_executable_donor()
    original = hashlib.sha256(source.read_bytes()).hexdigest()
    existing = runtime_fixture_root.native_executable("native-fixture/existing")
    existing.write_bytes(b"deliberate custody mutation")
    retained, first, second = runtime_fixture_root.native_executables(
        (
            "native-fixture/existing",
            "native-fixture/new-first",
            "native-fixture/new-second",
        )
    )
    assert retained == existing
    assert retained.read_bytes() == b"deliberate custody mutation"
    assert not first.samefile(second)
    for image in (first, second):
        assert hashlib.sha256(image.read_bytes()).hexdigest() == original


def test_native_executable_batch_retains_images_without_probing_source(
    runtime_fixture_root, monkeypatch
):
    from tests import runtime_build_identity_helper as helper

    image = runtime_fixture_root.native_executable("retained-image")
    image.write_bytes(b"retained mutation")
    monkeypatch.setattr(
        helper,
        "native_executable_donor",
        lambda: pytest.fail("an empty or existing-only batch must not request a donor"),
    )
    assert runtime_fixture_root.native_executables(()) == ()
    assert runtime_fixture_root.native_executables(("retained-image",)) == (image,)
    assert runtime_fixture_root.native_executable("retained-image") == image
    assert image.read_bytes() == b"retained mutation"


@pytest.mark.parametrize("relative", ["../outside", "."])
def test_native_executable_batch_confines_every_path_before_copy(
    runtime_fixture_root, relative
):
    with pytest.raises(ValueError, match="native executable fixture"):
        runtime_fixture_root.native_executables(("not-created", relative))
    assert not (runtime_fixture_root.path / "not-created").exists()


@pytest.mark.skipif(sys.platform != "darwin", reason="Apple copyfile error contract")
def test_native_executable_copy_error_preserves_existing_destination(tmp_path):
    import ctypes

    from tests.runtime_build_identity_helper import _copy_native_image

    if not hasattr(ctypes.CDLL("/usr/lib/libSystem.B.dylib"), "clonefile"):
        pytest.skip("host lacks COPYFILE_CLONE capability")
    destination = tmp_path / "existing-image"
    destination.write_bytes(b"existing fixture must survive")
    with pytest.raises(FileExistsError):
        _copy_native_image(Path(sys.executable).resolve(strict=True), destination)
    assert destination.read_bytes() == b"existing fixture must survive"


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "cpython-abi"])
def test_rust_build_identity_has_no_python_generator_dependency(
    identity_root, monkeypatch, kind
):
    from molt.python_runtime_admission import BuildPythonAdmission

    monkeypatch.setattr(
        BuildPythonAdmission,
        "capture",
        lambda *_a, **_k: pytest.fail("Rust build admitted Python"),
    )
    before = _resolve(identity_root, kind=kind, publication="no-python-generator")
    after = _resolve(
        identity_root,
        kind=kind,
        publication="no-python-generator",
        extra_env={
            "MOLT_BUILD_PYTHON": "missing-build-python",
            "PYTHON": "missing-shadowed-python",
            "MOLT_BUILD_PYTHON_SIGNATURE": "not-a-signature",
            "PYTHONPATH": "missing-ambient-root",
        },
    )
    assert after == before
    assert "build_python" not in after.toolchain_manifest.payload["toolchain"]["tools"]


@pytest.mark.parametrize("kind", ["native", "shared", "reloc", "cpython-abi"])
@pytest.mark.parametrize("coordinate", ["member", "family", "toolchain"])
def test_retired_rust_python_sidecar_epochs_are_rejected(
    identity_root, kind, coordinate
):
    current = _resolve(identity_root, kind=kind, publication="old-epoch")
    if coordinate == "toolchain":
        value = current.toolchain_manifest.to_dict()
        value["schema"] = "molt.runtime-toolchain-content.v3"
        with pytest.raises(ValueError, match="schema"):
            schema.RuntimeToolchainContentManifest.from_dict(value)
        return
    value = current.to_dict()
    if coordinate == "member":
        value["schema"] = "molt.runtime-build-member-identity.v3"
    else:
        value["payload"]["family"]["schema"] = "molt.runtime-build-family.v3"
        _reseal_runtime_build_identity(value)
    with pytest.raises(ValueError, match="schema|family shape"):
        schema.RuntimeBuildIdentity.from_dict(value)


@pytest.mark.parametrize(
    "mutation", ["order", "flag", "exports", "module-name", "missing", "duplicate"]
)
def test_publication_action_covers_link_semantics_and_rejects_incomplete_inputs(
    mutation,
):
    action = runtime_reloc_publication_action(
        ("-C", "link-arg=--export=molt_add"),
        module_name="molt_runtime_reloc.wasm",
        executor=publication_executor(schema.RuntimePublicationTransform.RELOC),
    )
    command = list(action.link_command)
    if mutation == "missing":
        command.remove(schema.RuntimePublicationInput.LIBC)
    elif mutation == "duplicate":
        command.append(schema.RuntimePublicationInput.STATICLIB)
    elif mutation == "order":
        a, b = (
            command.index(schema.RuntimePublicationInput.LIBC),
            command.index(schema.RuntimePublicationInput.COMPILER_RT),
        )
        command[a], command[b] = command[b], command[a]
    elif mutation == "flag":
        command[0] = "--relocatable"
    elif mutation == "module-name":
        command[command.index("--soname=molt_runtime_reloc.wasm")] = (
            "--soname=other.wasm"
        )
    if mutation in {"missing", "duplicate"}:
        with pytest.raises(ValueError, match="incomplete"):
            replace(action, link_command=tuple(command))
        return
    changed = replace(
        action,
        link_command=tuple(command),
        link_exports=("--export=molt_other",)
        if mutation == "exports"
        else action.link_exports,
    )
    assert canonical_json_sha256(action.to_dict()) != canonical_json_sha256(
        changed.to_dict()
    )
    assert schema.RuntimePublicationAction.from_dict(changed.to_dict()) == changed
    from tests.runtime_build_identity_helper import runtime_build_identity

    baseline = runtime_build_identity("reloc")

    def with_action(selected):
        wire = baseline.to_dict()
        wire["payload"]["family"]["members"]["reloc"]["publication"] = (
            selected.to_dict()
        )
        _reseal_runtime_build_identity(wire)
        return schema.RuntimeBuildIdentity.from_dict(wire)

    before, after = with_action(action), with_action(changed)
    assert before.compile_digest == after.compile_digest
    assert before.family_digest != after.family_digest
    assert (
        schema.runtime_build_fingerprint(before)["hash"]
        != schema.runtime_build_fingerprint(after)["hash"]
    )


@pytest.mark.parametrize("coordinate", ["member", "family"])
def test_pre_action_runtime_identity_epoch_is_rejected(coordinate):
    from tests.runtime_build_identity_helper import runtime_build_identity

    wire = runtime_build_identity("shared").to_dict()
    if coordinate == "member":
        wire["schema"] = "molt.runtime-build-member-identity.v4"
    else:
        wire["payload"]["family"]["schema"] = "molt.runtime-build-family.v4"
        _reseal_runtime_build_identity(wire)
    with pytest.raises(ValueError, match="schema|family shape"):
        schema.RuntimeBuildIdentity.from_dict(wire)


@pytest.mark.parametrize(
    "kind,relative",
    [
        ("native", "src/molt/cli/native_link_manifest.py"),
        ("native", "src/molt/cli/native_link_custody.py"),
        ("native", "src/molt/native_target_shape.py"),
        ("shared", "src/molt/wasm_artifact.py"),
        ("reloc", "src/molt/cli/wasm_link_args.py"),
        ("reloc", "src/molt/cli/runtime_identity_schema.py"),
    ],
)
def test_executor_edit_changes_publication_only_and_planner_edit_reuses(
    identity_root, kind, relative
):
    before = _resolve(identity_root, kind=kind, publication="stable")
    path = identity_root / relative
    path.write_bytes(path.read_bytes() + b"# changed output algorithm\n")
    after = _resolve(identity_root, kind=kind, publication="stable")
    assert before.compile_digest == after.compile_digest
    assert before.digest != after.digest
    assert before.family_digest != after.family_digest
    planner = identity_root / "src/molt/cli/runtime_wasm_build_spec.py"
    planner.parent.mkdir(parents=True, exist_ok=True)
    planner.write_text(
        "# planner change with identical resolved action\n", encoding="utf-8"
    )
    assert after == _resolve(identity_root, kind=kind, publication="stable")


@pytest.mark.parametrize(
    "mutation", ["absent", "null", "missing", "partial", "extra", "bad-digest"]
)
def test_publication_executor_receipt_rejects_incomplete_closure(mutation):
    action = schema.RuntimePublicationAction(
        schema.RuntimePublicationTransform.SHARED,
        False,
        publication_executor(schema.RuntimePublicationTransform.SHARED),
    ).to_dict()
    if mutation == "absent":
        del action["executor"]
    elif mutation == "null":
        action["executor"] = None
    else:
        executor = action["executor"]
        if mutation == "missing":
            executor["missing"] = executor["roots"][:1]
        elif mutation == "partial":
            executor["roots"] = executor["roots"][:-1]
            executor["file_count"] -= 1
        elif mutation == "extra":
            executor["roots"].append("unowned.py")
            executor["file_count"] += 1
        else:
            executor["digest"] = "not-a-digest"
    with pytest.raises(ValueError, match="publication|executor"):
        schema.RuntimePublicationAction.from_dict(action)


def test_copy_only_capi_has_no_python_executor(identity_root):
    before = _resolve(identity_root, kind="cpython-abi", publication="stable")
    (identity_root / "src/molt/wasm_artifact.py").write_text(
        "# changed rewrite\n", encoding="utf-8"
    )
    assert _resolve(identity_root, kind="cpython-abi", publication="stable") == before
    action = schema.runtime_publication_action(before)
    assert action.executor is None
    with pytest.raises(ValueError, match="byte-preserving"):
        replace(
            action,
            executor=publication_executor(schema.RuntimePublicationTransform.SHARED),
        )
