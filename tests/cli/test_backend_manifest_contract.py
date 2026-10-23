from __future__ import annotations

from pathlib import Path
import re
import tomllib
from types import SimpleNamespace
from typing import Any

import pytest


ROOT = Path(__file__).resolve().parents[2]


def _load_workspace_manifest() -> dict[str, Any]:
    with (ROOT / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def _load_backend_manifest() -> dict[str, Any]:
    with (ROOT / "runtime" / "molt-backend" / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def _load_native_backend_manifest() -> dict[str, Any]:
    with (ROOT / "runtime" / "molt-backend-native" / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def _canonical_profile_value(key: str, value: object) -> object:
    if type(value) is bool:
        if key == "debug":
            return 2 if value else 0
        if key == "strip":
            return "symbols" if value else "none"
    return value


def _effective_profile(profiles: dict, name: str) -> dict:
    """Independent Cargo-inheritance oracle for declarative profile contracts."""
    declared = profiles[name]
    parent = declared.get("inherits")
    resolved = _effective_profile(profiles, parent) if parent is not None else {}
    for key, value in declared.items():
        if key == "inherits":
            continue
        if key == "package":
            packages = resolved.setdefault("package", {})
            for package, policy in value.items():
                packages.setdefault(package, {}).update(policy)
        else:
            resolved[key] = _canonical_profile_value(key, value)
    return resolved


def test_backend_manifest_does_not_depend_on_obj_model() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]
    assert "molt-obj-model" not in dependencies


def test_backend_publication_is_owned_by_the_shared_artifact_crate() -> None:
    workspace = _load_workspace_manifest()
    assert "runtime/molt-artifact-publish" in workspace["workspace"]["members"]
    dependency = _load_backend_manifest()["dependencies"]["molt-artifact-publish"]
    assert dependency == {"path": "../molt-artifact-publish"}
    crate = ROOT / "runtime/molt-artifact-publish"
    manifest = tomllib.loads((crate / "Cargo.toml").read_text(encoding="utf-8"))
    assert not manifest.get("dependencies")
    assert not manifest.get("features")
    assert set(manifest["target"]) == {"cfg(unix)", "cfg(windows)"}
    # Opened-descriptor admission rejects FIFOs without waiting for a writer.
    assert manifest["target"]["cfg(unix)"]["dependencies"] == {"libc": "0.2"}
    assert set(manifest["target"]["cfg(windows)"]["dependencies"]) == {"windows-sys"}
    backend = ROOT / "runtime/molt-backend/src"
    assert not (backend / "backend_process/atomic_publish.rs").exists()
    for source in backend.rglob("*.rs"):
        text = source.read_text(encoding="utf-8")
        assert "atomic_publish::" not in text, source
        assert "mod atomic_publish" not in text, source
        assert not re.search(
            r"pub(?:\([^)]*\))?\s+use\s+molt_artifact_publish", text
        ), source
    authority = (crate / "src/lib.rs").read_text(encoding="utf-8")
    assert "feature = " not in authority
    assert "pub enum PublicationState" in authority
    plan = tomllib.loads((ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8"))
    commands = {command["id"]: command for command in plan["command"]}
    compiler_argv = commands["rust.test.compiler-authorities"]["argv"]
    assert "molt-artifact-publish" in {
        compiler_argv[index + 1]
        for index, argument in enumerate(compiler_argv[:-1])
        if argument == "-p"
    }
    rules = {rule["name"]: rule for rule in plan["rule"]}
    for consumer in ("backend-native", "wasm-host"):
        assert "runtime/molt-artifact-publish/**/*.rs" in rules[consumer]["globs"]
        assert "runtime/molt-artifact-publish/Cargo.toml" in rules[consumer]["globs"]
    assert any(
        "cargo test" in gate and "-p molt-artifact-publish" in gate
        for gate in rules["backend-native"]["gates"]
    )


def test_backend_manifest_keeps_wasmparser_test_only() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]
    dev_dependencies = manifest.get("dev-dependencies", {})

    assert "wasmparser" not in dependencies
    assert "wasm-encoder" not in dependencies
    # Version selection belongs to Cargo manifests/lock. The tests must use
    # the same parser/encoder generation as the owning artifact-facts crate.
    facts = tomllib.loads(
        (ROOT / "runtime/molt-wasm-facts/Cargo.toml").read_text(encoding="utf-8")
    )
    for package in ("wasmparser", "wasm-encoder"):
        dependency = facts["dependencies"][package]
        version = dependency["version"] if isinstance(dependency, dict) else dependency
        assert dev_dependencies[package] == version


def test_backend_manifest_uses_serde_with_derive_feature() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]

    # serde is required for JSON boundary, IR serialization, and TIR
    assert "serde" in dependencies
    serde_dep = dependencies["serde"]
    assert "derive" in serde_dep.get("features", [])


def test_backend_manifest_uses_minimal_cranelift_codegen_features() -> None:
    manifest = _load_native_backend_manifest()
    codegen_dependency = manifest["dependencies"]["cranelift-codegen"]

    assert codegen_dependency["default-features"] is False
    assert set(codegen_dependency["features"]) == {
        "arm64",
        "host-arch",
        "std",
        "unwind",
        "x86",
    }


def test_backend_manifest_has_no_duplicate_cranelift_target_overlays() -> None:
    manifest = _load_native_backend_manifest()
    assert all(
        "cranelift-codegen" not in table.get("dependencies", {})
        for table in manifest["target"].values()
    )


def _declared_workspace_package_names(manifest: dict) -> set[str]:
    # Read the explicit workspace authority, not a molt-* name classifier.
    return {
        tomllib.loads((ROOT / member / "Cargo.toml").read_text(encoding="utf-8"))[
            "package"
        ]["name"]
        for member in manifest["workspace"]["members"]
    }


def test_workspace_dev_dependency_symbols_have_one_wildcard_authority() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    workspace_names = _declared_workspace_package_names(manifest)
    assert profiles["dev-fast"]["inherits"] == "dev"
    assert "package" not in profiles["dev-fast"]

    for profile_name in ("dev", "dev-fast"):
        packages = _effective_profile(profiles, profile_name)["package"]
        # Cargo applies this to every non-workspace member, including future
        # dependencies. Named overrides merge per field, so opt-only hot
        # exceptions still use wildcard debug=0, without duplicated rows.
        assert packages["*"] == {"debug": 0}
        assert {
            name: policy
            for name, policy in packages.items()
            if name != "*" and name not in workspace_names
        } == {
            "cranelift-codegen": {"opt-level": 1},
            "regalloc2": {"opt-level": 1},
            "sha2": {"opt-level": 1},
        }


def test_workspace_dev_dependency_wildcard_preserves_workspace_policy() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    workspace_names = _declared_workspace_package_names(manifest)
    expected_hot_members = {
        "molt-backend": {"opt-level": 1, "debug": 0},
        "molt-backend-native": {"opt-level": 1, "debug": 0},
        "molt-backend-luau": {"opt-level": 1, "debug": 0},
        "molt-backend-rust": {"opt-level": 1, "debug": 0},
        "molt-backend-wasm": {"opt-level": 1, "debug": 0},
        "molt-tir": {"opt-level": 1, "debug": 0},
        "molt-passes": {"opt-level": 1, "debug": 0},
        "molt-ir": {"opt-level": 1, "debug": 0},
        "molt-codegen-abi": {"opt-level": 1, "debug": 0},
        "molt-artifact-publish": {"opt-level": 1, "debug": 0},
        "molt-runtime": {"opt-level": 2, "debug": 0},
        # The maintained vendor patches are members only so their upstream
        # tests resolve; they keep the dependency debug policy, not a hot
        # override.
        "cranelift-object": {"debug": 0},
        "zlib-rs": {"debug": 0},
    }
    assert expected_hot_members.keys() <= workspace_names

    for profile_name, member_debug in (("dev", 2), ("dev-fast", 1)):
        profile = _effective_profile(profiles, profile_name)
        # Cargo excludes workspace members from package."*". Members without
        # a named override keep the profile's debug/optimization settings;
        # the existing workspace hot exceptions retain their complete policy.
        assert profile["debug"] == member_debug
        assert profile["opt-level"] == 0
        assert {
            name: policy
            for name, policy in profile["package"].items()
            if name in workspace_names
        } == expected_hot_members


def test_workspace_dev_fast_does_not_force_opt_level() -> None:
    manifest = _load_workspace_manifest()
    dev_fast_profile = manifest["profile"]["dev-fast"]

    assert "opt-level" not in dev_fast_profile


def test_profile_children_have_no_mirrored_inherited_settings() -> None:
    profiles = _load_workspace_manifest()["profile"]
    for name, declared in profiles.items():
        parent = declared.get("inherits")
        if parent is None:
            continue
        inherited = _effective_profile(profiles, parent)
        for key, value in declared.items():
            if key == "inherits":
                continue
            if key == "package":
                for package, policy in value.items():
                    inherited_policy = inherited.get("package", {}).get(package, {})
                    for setting, selected in policy.items():
                        assert selected != inherited_policy.get(setting), (
                            f"{name}.{package}.{setting} mirrors {parent}"
                        )
            else:
                assert _canonical_profile_value(key, value) != inherited.get(key), (
                    f"{name}.{key} mirrors {parent}"
                )


def test_shipping_profiles_share_one_memory_bounded_codegen_policy() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]

    assert profiles["release-fast"]["inherits"] == "release"
    assert profiles["release-size"]["inherits"] == "release-output"
    assert profiles["wasm-release"]["inherits"] == "release-size"
    release_fast = _effective_profile(profiles, "release-fast")
    assert release_fast["lto"] == "off"
    assert release_fast["codegen-units"] == 256
    assert release_fast["debug"] == 0
    assert release_fast["panic"] == "unwind"

    shipping_policy = {
        "opt-level": "z",
        "lto": "thin",
        "codegen-units": 16,
        "debug": 0,
        "panic": "abort",
        "strip": "symbols",
    }
    for profile_name in ("release-output", "release-size", "wasm-release"):
        shipping_profile = _effective_profile(profiles, profile_name)
        assert {
            key: shipping_profile[key] for key in shipping_policy
        } == shipping_policy

    assert "wasm-release-fallback" not in profiles

    for profile_name, profile in profiles.items():
        for package_name, package_policy in profile.get("package", {}).items():
            assert "codegen-units" not in package_policy, (
                f"{profile_name}.{package_name} duplicates profile-owned "
                "codegen partitioning"
            )

    hot_crates = {
        "molt-runtime",
        "molt-runtime-core",
        "molt-lang-obj-model",
        "molt-runtime-collections",
    }
    for name, level in (
        ("release-output", 3),
        ("release-size", "s"),
        ("wasm-release", "s"),
    ):
        packages = _effective_profile(profiles, name)["package"]
        assert all(packages[crate]["opt-level"] == level for crate in hot_crates)

    dev_release = _effective_profile(profiles, "dev-release")
    assert dev_release["debug"] == 1
    assert dev_release["strip"] == "none"


def test_runtime_wasm_shipping_has_no_fallback_compiler_authority() -> None:
    runtime_cli = ROOT / "src" / "molt" / "cli"
    runtime_sources = "\n".join(
        (runtime_cli / name).read_text(encoding="utf-8")
        for name in (
            "runtime_build.py",
            "runtime_wasm_build.py",
            "runtime_wasm_build_policy.py",
            "runtime_wasm_build_spec.py",
            "runtime_wasm_build_support.py",
            "runtime_wasm_pair_build.py",
        )
    )
    runtime_wasm_build = (runtime_cli / "runtime_wasm_build.py").read_text(
        encoding="utf-8"
    )
    non_native_output = (
        ROOT / "src" / "molt" / "cli" / "non_native_output.py"
    ).read_text(encoding="utf-8")

    for deleted_authority in (
        "MOLT_WASM_RUNTIME_FALLBACK_PROFILE",
        "MOLT_RUNTIME_WASM_DUAL_COMPILE",
        "MOLT_RUNTIME_WASM_SINGLE_COMPILE",
        "_wasm_runtime_recovery_target_root",
        "_app_split_runtime_dual_compile_forced",
    ):
        assert deleted_authority not in runtime_sources
        assert deleted_authority not in non_native_output

    assert "def _ensure_runtime_wasm(" not in runtime_wasm_build


@pytest.mark.parametrize("failure", [None, "combined", "shared", "reloc"])
def test_runtime_wasm_pair_materializes_only_from_one_combined_observation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str | None
) -> None:
    from molt.cli import runtime_wasm_pair_build as pair
    from molt.cli.models import _RuntimeArtifactState
    from molt.process_environment import ProcessEnvironment
    from tests.runtime_build_identity_helper import (
        runtime_build_identity,
        runtime_cargo_environment_inputs,
    )

    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    environment = ProcessEnvironment({"MOLT_TEST_RUNTIME_INPUT": "pair-source"})
    observed = runtime_cargo_environment_inputs(environment)
    shared = runtime_build_identity("shared", "manifest-pair-contract")
    reloc = runtime_build_identity("reloc", "manifest-pair-contract")
    # This tests orchestration, not Cargo-plan resolution. The real pair owner
    # stages members, records failures and threads the typed compile observation.
    shared_spec = SimpleNamespace(cargo_plan=SimpleNamespace(environment=environment))
    reloc_spec = SimpleNamespace()
    ctx = pair._RuntimeWasmPairBuild(
        runtime_state=_RuntimeArtifactState(),
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
        runtime_wasm=tmp_path / "molt_runtime.wasm",
        runtime_reloc_wasm=tmp_path / "molt_runtime_reloc.wasm",
        shared_spec=shared_spec,
        reloc_spec=reloc_spec,
        toolchain_manifest_path=tmp_path / "toolchain.json",
        generation_manifest=tmp_path / "molt_runtime.generation.json",
        pre_identity=pair._RuntimeWasmPairIdentity(
            shared.toolchain_manifest, shared, reloc
        ),
    )
    ctx.runtime_wasm.write_bytes(b"published shared")
    ctx.runtime_reloc_wasm.write_bytes(b"published reloc")
    events: list[str] = []

    def combined(**kwargs):
        events.append("combined")
        assert kwargs["shared_spec"] is shared_spec
        assert kwargs["reloc_spec"] is reloc_spec
        assert kwargs["force_build"] is False
        return None if failure == "combined" else observed

    def materialize(path, *, reloc, spec, cargo_environment, **_kwargs):
        member = "reloc" if reloc else "shared"
        events.append(member)
        assert cargo_environment is observed
        assert spec is (reloc_spec if reloc else shared_spec)
        assert path == ctx.staging_member(reloc=reloc)
        assert path not in (ctx.runtime_wasm, ctx.runtime_reloc_wasm)
        if failure == member:
            return False
        path.write_bytes(member.encode())
        return True

    monkeypatch.setattr(pair, "_build_state_root", lambda _root: tmp_path / "state")
    monkeypatch.setattr(
        pair, "admit_runtime_wasm_pair_from_shared_cache", lambda **_k: None
    )
    monkeypatch.setattr(pair, "_prepopulate_combined_runtime_wasm_target", combined)
    monkeypatch.setattr(
        pair, "_materialize_runtime_wasm_member_from_target", materialize
    )
    monkeypatch.setattr(
        pair,
        "_run_runtime_wasm_cargo_build",
        lambda **_k: pytest.fail("pair attempted an independent fallback compile"),
    )
    try:
        outcome = pair._materialize_runtime_wasm_pair(ctx)
        assert outcome is (
            pair._PairBuildOutcome.BUILT
            if failure is None
            else pair._PairBuildOutcome.FAILED
        )
        expected = ["combined"]
        if failure != "combined":
            expected.append("shared")
        if failure not in ("combined", "shared"):
            expected.append("reloc")
        assert events == expected
        assert ctx.cargo_environment is (None if failure == "combined" else observed)
        assert ctx.runtime_wasm.read_bytes() == b"published shared"
        assert ctx.runtime_reloc_wasm.read_bytes() == b"published reloc"
        assert not ctx.generation_manifest.exists()
    finally:
        ctx.cleanup_staging()


def test_runtime_manifest_uses_flate2_zip_deflate_only() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    zip_dependency = runtime_manifest["dependencies"]["zip"]

    assert zip_dependency["default-features"] is False
    assert zip_dependency["features"] == ["deflate"]


def test_runtime_net_cfgs_name_one_provider_per_target() -> None:
    """stdlib_net selects a networking provider; nothing stands in for one.

    `molt_has_net_io` is the native socket ABI (unix), `molt_wasm_net` the WASM
    host-call ABI, `molt_net` either. A target without a provider (Windows)
    defines no networking symbol: the intrinsic manifest declares those symbols
    `not(target_os = "windows")`, so reachability refuses there at build time
    instead of linking a stub that fails at run time (HF-487).
    """
    build_rs = (ROOT / "runtime" / "molt-runtime" / "build.rs").read_text(
        encoding="utf-8"
    )
    for cfg in ("molt_has_net_io", "molt_wasm_net", "molt_net"):
        assert f'println!("cargo:rustc-check-cfg=cfg({cfg})");' in build_rs
        assert f'println!("cargo:rustc-cfg={cfg}");' in build_rs
    assert 'env::var("CARGO_FEATURE_STDLIB_NET").is_ok()' in build_rs
    assert 'family == "unix"' in build_rs
    assert not (
        ROOT / "runtime" / "molt-runtime" / "src" / "async_rt" / "net_stubs.rs"
    ).exists()


def test_runtime_manifest_uses_minimal_rustpython_parser_features() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    parser_dependency = runtime_manifest["dependencies"]["rustpython-parser"]

    assert parser_dependency["default-features"] is False
    assert set(parser_dependency["features"]) == {"location", "num-bigint"}


def test_runtime_manifest_dedupes_unicode_names2_version() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    text_manifest_path = ROOT / "runtime" / "molt-stdlib-text" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    with text_manifest_path.open("rb") as handle:
        text_manifest = tomllib.load(handle)

    runtime_dep = runtime_manifest["dependencies"]["unicode_names2"]
    text_dep = text_manifest["dependencies"]["unicode_names2"]
    runtime_version = (
        runtime_dep["version"] if isinstance(runtime_dep, dict) else runtime_dep
    )
    text_version = text_dep["version"] if isinstance(text_dep, dict) else text_dep

    assert runtime_version == text_version
    # These direct consumers must share one resolved table. Other transitive
    # dependency families can legitimately select an older table generation.
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    selected_edges = []
    for name in ("molt-runtime", "molt-stdlib-text"):
        owners = [p for p in lock["package"] if p["name"] == name]
        assert len(owners) == 1
        edges = [
            dep
            for dep in owners[0]["dependencies"]
            if dep.split()[0] == "unicode_names2"
        ]
        assert len(edges) == 1
        selected_edges.append(edges[0])
    assert selected_edges[0] == selected_edges[1]
    selector = selected_edges[0].split()
    resolved = [
        p
        for p in lock["package"]
        if p["name"] == selector[0]
        and (len(selector) == 1 or p["version"] == selector[1])
    ]
    assert len(resolved) == 1


def test_runtime_manifest_declares_vfs_bundle_tar_feature() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    assert "vfs_bundle_tar" in runtime_manifest["features"]


def test_runtime_tk_native_feature_is_owned_by_leaf_crate() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    tk_manifest_path = ROOT / "runtime" / "molt-runtime-tk" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    with tk_manifest_path.open("rb") as handle:
        tk_manifest = tomllib.load(handle)

    runtime_features = runtime_manifest["features"]
    tk_dependency = runtime_manifest["dependencies"]["molt-runtime-tk"]
    native_target_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))'][
        "dependencies"
    ]

    assert tk_dependency["default-features"] is False
    assert native_target_deps["libloading"]["optional"] is True
    assert runtime_features["source_extension_loader"] == ["dep:libloading"]
    assert runtime_features["stdlib_tk"] == [
        "dep:molt-runtime-tk",
        "molt-runtime-tk/tk",
    ]
    assert runtime_features["molt_tk_native"] == [
        "stdlib_tk",
        "molt-runtime-tk/native-tcl",
    ]
    # The native Tcl loader sets the process environment through the
    # runtime-platform crate (1afa7afe4).
    assert tk_manifest["features"]["native-tcl"] == [
        "tk",
        "dep:libloading",
        "dep:molt-runtime-platform",
    ]


def test_runtime_micro_profile_includes_core_non_network_intrinsics() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    micro_features = runtime_manifest["features"]["stdlib_micro"]

    assert micro_features == [
        "stdlib_asyncio",
        "stdlib_collections",
        "stdlib_fs_extra",
        "stdlib_logging_ext",
    ]
    assert "stdlib_net" not in micro_features


def test_cli_micro_base_mirror_does_not_drift_from_cargo_stdlib_micro() -> None:
    """The CLI profile-availability mirror must equal Cargo.toml ``stdlib_micro``.

    ``_MICRO_BASE_RUNTIME_FEATURES`` is a hand-maintained Python mirror of the
    Cargo ``stdlib_micro`` feature list, which is the always-linked base of every
    profile (strict superset chain micro→edge→standard→server→full).  When the
    mirror omits a feature ``stdlib_micro`` pulls in, the compile-time
    profile-availability gate falsely refuses any import graph that statically
    reaches that feature's intrinsics.  That exact drift (the mirror omitted
    ``stdlib_collections``) silently broke ``import pprint`` / ``import asyncio``
    the moment P0 #50 made class-body control flow execute.  This guard turns
    the drift into a CI failure instead of a latent silent refusal (task #85).
    """
    from molt.cli.runtime_features import _MICRO_BASE_RUNTIME_FEATURES

    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    micro_features = runtime_manifest["features"]["stdlib_micro"]

    assert set(_MICRO_BASE_RUNTIME_FEATURES) == set(micro_features), (
        "CLI _MICRO_BASE_RUNTIME_FEATURES drifted from Cargo.toml stdlib_micro: "
        f"cli={sorted(_MICRO_BASE_RUNTIME_FEATURES)} cargo={sorted(micro_features)}"
    )


def test_cli_profile_availability_covers_every_always_linked_micro_feature() -> None:
    """Every profile/target enabled set must cover the always-linked micro base.

    ``stdlib_micro`` is linked into EVERY profile archive, so the
    profile-availability gate must never exclude a micro-base feature for any
    profile or target — doing so falsely refuses builds whose import graph
    reaches those intrinsics.  This pins the invariant across the three branches
    of ``_runtime_builtin_features_for_profile`` (non-micro / micro-wasm /
    micro-native).
    """
    from molt.cli.runtime_features import (
        _MICRO_BASE_RUNTIME_FEATURES,
        _runtime_builtin_features_for_profile,
    )

    micro_base = set(_MICRO_BASE_RUNTIME_FEATURES)
    for profile in (None, "full", "server", "micro"):
        for target in (None, "aarch64-apple-darwin", "wasm32-unknown-unknown"):
            enabled = set(
                _runtime_builtin_features_for_profile(profile, target_triple=target)
            )
            assert micro_base <= enabled, (
                f"profile={profile!r} target={target!r} omits always-linked "
                f"micro-base feature(s) {sorted(micro_base - enabled)}"
            )


def test_runtime_manifest_defaults_to_dependency_only_rlib() -> None:
    """Final artifact producers must opt into their exact external crate types."""
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    crate_types = runtime_manifest["lib"]["crate-type"]

    assert crate_types == ["rlib"]


def test_backend_manifest_gates_loop_continue_to_native_backend() -> None:
    manifest = _load_backend_manifest()
    tests = manifest.get("test", [])
    loop_continue = next(test for test in tests if test["name"] == "loop_continue")
    assert loop_continue["required-features"] == ["native-backend"]


def test_runtime_manifest_avoids_url_compile_graph_for_websocket_client() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    native_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))'][
        "dependencies"
    ]

    assert "url" not in native_deps
    assert "url" not in native_deps["tungstenite"]["features"]


def test_backend_ir_model_and_passes_are_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]

    assert "molt-ir" in dependencies
    assert "molt-tir" in dependencies
    assert "pub use molt_ir::" in lib_rs
    assert "pub use molt_tir::{passes, representation_plan, tir};" in lib_rs
    assert "pub struct SimpleIR" not in lib_rs
    assert "pub fn validate_simple_ir" not in lib_rs


def test_backend_native_trampoline_identity_is_split_out_of_lib_rs() -> None:
    native_backend_mod_path = (
        ROOT / "runtime" / "molt-backend-native" / "src" / "native_backend" / "mod.rs"
    )
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )

    assert native_backend_mod_path.exists()
    # The god-file split extracted trampoline identity OUT of the lib.rs facade
    # and into the native_backend module (struct TrampolineKey lives in mod.rs,
    # not a standalone trampolines.rs).  Assert the real current home so the
    # guard pins the actual structure rather than a renamed-away filename.
    assert "struct TrampolineKey" not in lib_rs
    assert "struct TrampolineKey" in native_backend_mod_path.read_text(encoding="utf-8")


def test_backend_native_compile_func_is_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text(
        encoding="utf-8"
    )
    function_compiler_rs = (
        ROOT
        / "runtime"
        / "molt-backend-native"
        / "src"
        / "native_backend"
        / "function_compiler.rs"
    )

    assert function_compiler_rs.exists()
    assert "fn compile_func(" not in lib_rs


def test_native_backend_codegen_failures_are_fail_closed() -> None:
    native_sources = [
        ROOT / "runtime" / "molt-backend-native" / "src" / "lib.rs",
        ROOT
        / "runtime"
        / "molt-backend-native"
        / "src"
        / "native_backend"
        / "function_compiler.rs",
    ]
    combined = "\n".join(path.read_text(encoding="utf-8") for path in native_sources)

    assert "catch_unwind" not in combined
    assert "emit_trap_stub" not in combined
    assert "trap_stub_names" not in combined
    assert "emitting trap stub" not in combined
    assert "will retry at opt_level=none" not in combined
