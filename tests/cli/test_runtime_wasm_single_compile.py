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

import json
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli import artifact_state as runtime_artifact_state
from molt.cli import (
    runtime_build_identity,
    runtime_fingerprints,
    runtime_wasm_build,
    runtime_wasm_build_spec,
    runtime_wasm_build_support,
    runtime_wasm_pair_build,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.models import (
    _ExternalNativeAbiSymbol,
    _ExternalNativeCapiSymbol,
    _ExternalPackageNativeArtifactPlan,
    _RuntimeArtifactState,
)
from molt.cli.runtime_artifact_selection import (
    RUNTIME_CDYLIB_ARTIFACTS,
    RUNTIME_STATICLIB_ARTIFACTS,
)
from molt.cli.runtime_wasm_build_timings import (
    _reset_runtime_wasm_build_timings,
    _runtime_wasm_build_timings_snapshot,
)
from molt.cli.wasm_toolchain import WasmLinkerIdentity

_COMMON = dict(
    cargo_profile="release",
    simd_enabled=True,
    freestanding=False,
    stdlib_profile="full",
    resolved_modules=None,
    required_link_features=frozenset(),
    required_exports=None,
)


def _specs(root: Path):
    shared = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime.wasm", reloc=False, **_COMMON
    )
    reloc = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime_reloc.wasm", reloc=True, **_COMMON
    )
    return shared, reloc


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
    assert shared.fingerprint["meta_digest"] != reloc.fingerprint["meta_digest"]
    # Shared link flags carry the split-runtime import ABI.
    for flag in ("--import-memory", "--import-table", "--growable-table"):
        assert flag in shared.link_flags


def test_reloc_cache_identity_tracks_exact_wasm_linker_binary() -> None:
    root = _compiler_root()
    first = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        root / "wasm" / "molt_runtime_reloc.wasm",
        reloc=True,
        wasm_linker_identity=WasmLinkerIdentity(
            Path("/llvm/bin/wasm-ld"), "22.1.8", "22.1.8", "a" * 64
        ),
        **_COMMON,
    )
    second = runtime_wasm_build_spec._compute_runtime_wasm_build_spec(
        root,
        root / "wasm" / "molt_runtime_reloc.wasm",
        reloc=True,
        wasm_linker_identity=WasmLinkerIdentity(
            Path("/llvm/bin/wasm-ld"), "22.1.8", "22.1.8", "b" * 64
        ),
        **_COMMON,
    )

    assert first.fingerprint is not None and second.fingerprint is not None
    assert first.fingerprint["meta_digest"] != second.fingerprint["meta_digest"]
    assert first.staticlib_fingerprint == second.staticlib_fingerprint


def test_native_plan_is_the_pre_staging_runtime_export_authority() -> None:
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
        {"PyTuple_New", "PyExc_TypeError"}
    )


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
        **{**common, "required_exports": {"add", "abc_abstractmethod_check"}},
    )
    assert early.fingerprint is not None and final.fingerprint is not None
    assert early.staticlib_fingerprint is not None
    assert early.fingerprint["meta_digest"] != final.fingerprint["meta_digest"]
    assert early.staticlib_fingerprint == final.staticlib_fingerprint

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
        runtime_wasm_build, "_compute_runtime_wasm_build_spec", lambda *a, **k: final
    )
    monkeypatch.setattr(
        runtime_wasm_build, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        runtime_wasm_build,
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
        long_double_required: bool,
    ) -> bool:
        del json_output, link_timeout, long_double_required
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
    monkeypatch.setattr(
        runtime_wasm_build, "_is_valid_runtime_wasm_artifact", lambda _path: True
    )
    output = tmp_path / "molt_runtime_reloc.wasm"
    assert runtime_wasm_build._ensure_runtime_wasm(
        output,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=root,
        stdlib_profile="full",
        required_exports={"add", "abc_abstractmethod_check"},
    )
    assert linked and linked[0][0] == staticlib
    assert "--export-if-defined=molt_abc_abstractmethod_check" in linked[0][1]


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
        return (
            shared._replace(target_root=target_root),
            reloc._replace(target_root=target_root),
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
    assert early_shared.fingerprint == final_shared.fingerprint
    assert early_reloc.fingerprint != final_reloc.fingerprint
    assert early_reloc.staticlib_fingerprint == final_reloc.staticlib_fingerprint

    profile_root = target_root / "wasm32-wasip1" / early_shared.profile_dir
    cdylib = profile_root / "deps" / "molt_runtime-feedface.wasm"
    staticlib = profile_root / "deps" / "libmolt_runtime-feedface.a"
    cargo_calls: list[list[str]] = []

    def _fake_build(**kwargs):  # noqa: ANN003
        cmd = list(kwargs["cmd"])
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
    monkeypatch.setattr(
        runtime_wasm_pair_build.wasm_toolchain,
        "rust_target_libdir",
        lambda *a, **k: tmp_path,
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
    pair_payload = {"schema": "molt.runtime-build-pair.v2", "test": "pair"}
    payload = {"pair": pair_payload}
    pair_digest = runtime_build_identity._digest(pair_payload)
    member = runtime_build_identity.RuntimeBuildIdentity(
        digest=runtime_build_identity._digest(payload),
        pair_digest=pair_digest,
        payload=payload,
    )
    toolchain_payload = {"test": "toolchain"}
    toolchain = runtime_build_identity.RuntimeToolchainContentManifest(
        digest=runtime_build_identity._digest(toolchain_payload),
        payload=toolchain_payload,
    )
    return runtime_wasm_pair_build._RuntimeWasmPairIdentity(
        toolchain=toolchain,
        shared=member,
        reloc=member,
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
        generation_manifest=canonical_shared.with_name(
            "molt_runtime.generation.json"
        ),
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
    assert first_root.parent.name == identity.shared.pair_digest
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


def test_reloc_pair_acceptance_uses_linking_definitions_with_fallback_semantics(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    ctx = _test_pair_context(tmp_path, required_exports={"add"})
    observed: list[dict[str, str]] = []

    def defined_names(_path: Path, expected: dict[str, str]) -> frozenset[str]:
        observed.append(expected)
        return frozenset({"molt_add"})

    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "wasm_linking_defined_names",
        defined_names,
    )
    assert ctx.reloc_missing_required_symbols(tmp_path / "reloc-member") == set()
    assert observed == [{"molt_add": "function"}]

    monkeypatch.setattr(
        runtime_wasm_pair_build,
        "wasm_linking_defined_names",
        lambda _path, _expected: frozenset(),
    )
    assert ctx.reloc_missing_required_symbols(tmp_path / "reloc-member") == {
        "molt_add"
    }


def test_pair_target_materialization_keeps_canonical_spec_without_output_sidecar(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    shared, _reloc = _specs(_compiler_root())
    destination = tmp_path / "staging" / "molt_runtime.wasm"
    observed: list[tuple[Path, object, bool]] = []

    def reuse(ctx, *, persist_output_fingerprint: bool):  # noqa: ANN001
        observed.append(
            (ctx.runtime_wasm, ctx.spec, persist_output_fingerprint)
        )
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
        cmd,
        root,
        env,
        cargo_timeout,
        profile_dir,
        target_root_override,
        json_output,
        artifact_kind,
    ):
        captured["cmd"] = list(cmd)
        captured["env"] = dict(env)
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
    monkeypatch.setattr(
        runtime_wasm_pair_build.wasm_toolchain,
        "rust_target_libdir",
        lambda *a, **k: Path(tmp_path),
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
        cmd = kwargs["cmd"]
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
    monkeypatch.setattr(
        runtime_wasm_pair_build.wasm_toolchain,
        "rust_target_libdir",
        lambda *a, **k: tmp_path,
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
