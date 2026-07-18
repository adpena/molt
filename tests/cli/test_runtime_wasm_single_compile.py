"""V1 single-compile split-runtime unit tests (no real cargo build).

Verifies the combined-compile design invariants that make the dedup correct:

* the reloc and shared specs share one compile identity (same target dir, same
  cargo profile, same feature plan) but keep DISTINCT content fingerprints, so a
  single compile can serve both while the artifacts stay independently cached;
* the combined cargo invocation passes NO ``--crate-type`` override (so rustc
  emits every declared crate-type in one codegen) and routes the shared cdylib
  link args through a ``-C link-arg=@response`` file (not RUSTFLAGS);
* the ``MOLT_RUNTIME_WASM_SINGLE_COMPILE`` kill switch is honoured.
"""

from __future__ import annotations

import subprocess
import json
from pathlib import Path

import pytest

import molt.cli.runtime_build as rb
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.models import (
    _ExternalNativeAbiSymbol,
    _ExternalNativeCapiSymbol,
    _ExternalPackageNativeArtifactPlan,
)
from molt.cli.runtime_wasm_build_timings import (
    _reset_runtime_wasm_build_timings,
    _runtime_wasm_build_timings_snapshot,
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


def _specs(root: Path):
    shared = rb._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime.wasm", reloc=False, **_COMMON
    )
    reloc = rb._compute_runtime_wasm_build_spec(
        root, root / "wasm" / "molt_runtime_reloc.wasm", reloc=True, **_COMMON
    )
    return shared, reloc


def test_kill_switch(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    assert rb._single_compile_split_runtime_enabled() is True
    for off in ("0", "false", "no", "off", "OFF"):
        monkeypatch.setenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", off)
        assert rb._single_compile_split_runtime_enabled() is False
    monkeypatch.setenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", "1")
    assert rb._single_compile_split_runtime_enabled() is True


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
    # ...but the artifacts have distinct content identities (link args differ).
    assert shared.fingerprint["meta_digest"] != reloc.fingerprint["meta_digest"]
    # Shared link flags carry the split-runtime import ABI.
    for flag in ("--import-memory", "--import-table", "--growable-table"):
        assert flag in shared.link_flags


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
    early = rb._compute_runtime_wasm_build_spec(
        root,
        tmp_path / "early_reloc.wasm",
        reloc=True,
        **{**common, "required_exports": {"add"}},
    )
    final = rb._compute_runtime_wasm_build_spec(
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
    staticlib = rb._wasm_runtime_staticlib_path(target_root, final.profile_dir)
    staticlib.parent.mkdir(parents=True, exist_ok=True)
    staticlib.write_bytes(b"!<arch>\n")
    state_root = target_root / ".molt_state"
    staticlib_sidecar = rb._runtime_target_fingerprint_path(
        state_root,
        staticlib,
        cargo_profile=final.cargo_profile,
        target_label="wasm32-wasip1",
    )
    staticlib_sidecar.parent.mkdir(parents=True, exist_ok=True)
    rb._write_runtime_fingerprint(
        staticlib_sidecar,
        early.staticlib_fingerprint,
        artifact=staticlib,
    )

    linked: list[tuple[Path, str]] = []
    monkeypatch.setattr(rb, "_compute_runtime_wasm_build_spec", lambda *a, **k: final)
    monkeypatch.setattr(rb, "_build_state_root", lambda _root: state_root)
    monkeypatch.setattr(rb, "_hydrate_runtime_wasm_from_shared_cache", lambda **k: False)
    monkeypatch.setattr(rb, "_build_reuse_compatible_enabled", lambda: False)
    monkeypatch.setattr(
        rb,
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
        output_path.write_bytes(b"\0asm\x01\0\0\0relinked")
        return True

    monkeypatch.setattr(rb, "_link_runtime_staticlib_to_reloc_wasm", _relink)
    monkeypatch.setattr(
        rb,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path, *, integrity_key: None,
    )
    output = tmp_path / "molt_runtime_reloc.wasm"
    assert rb._ensure_runtime_wasm(
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


def test_cross_export_prepopulation_reports_one_cargo_compile(
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
    }

    def _pair(label: str, required_exports: set[str]):
        shared = rb._compute_runtime_wasm_build_spec(
            root,
            tmp_path / f"{label}_shared.wasm",
            reloc=False,
            **{**common, "required_exports": required_exports},
        )
        reloc = rb._compute_runtime_wasm_build_spec(
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
        "final", {"add", "abc_abstractmethod_check"}
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

    monkeypatch.setattr(rb, "_run_runtime_wasm_cargo_build", _fake_build)
    monkeypatch.setattr(rb, "_build_state_root", lambda _root: state_root)
    monkeypatch.setattr(rb, "_inspect_wasm_binary", lambda _path: "valid")
    monkeypatch.setattr(
        rb, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
    monkeypatch.setattr(
        rb.wasm_toolchain, "rust_target_libdir", lambda *a, **k: tmp_path
    )

    _reset_runtime_wasm_build_timings()
    try:
        assert rb._prepopulate_combined_runtime_wasm_target(
            shared_spec=early_shared,
            reloc_spec=early_reloc,
            json_output=True,
            cargo_timeout=None,
            project_root=root,
            simd_enabled=True,
            freestanding=False,
        )
        assert rb._prepopulate_combined_runtime_wasm_target(
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
    shared = rb._compute_runtime_wasm_build_spec(
        root,
        root / "wasm" / "molt_runtime.wasm",
        reloc=False,
        **{
            **_COMMON,
            "stdlib_profile": "micro",
            "required_exports": required_exports,
        },
    )
    reloc = rb._compute_runtime_wasm_build_spec(
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


def test_combined_cargo_cmd_has_no_crate_type_override_and_uses_response_file(
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

    monkeypatch.setattr(rb, "_run_runtime_wasm_cargo_build", _fake_build)
    # Force a cold target dir so the fast-path reuse check misses and we build.
    monkeypatch.setattr(rb, "_current_runtime_target_artifact", lambda *a, **k: None)
    monkeypatch.setattr(
        rb.wasm_toolchain, "rust_target_libdir", lambda *a, **k: Path(tmp_path)
    )

    ok = rb._prepopulate_combined_runtime_wasm_target(
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
    # No crate-type override anywhere: rustc emits all declared crate-types.
    assert not any("crate-type" in str(tok) for tok in cmd)
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
    monkeypatch.setattr(rb, "_run_runtime_wasm_cargo_build", _fake_build)
    monkeypatch.setattr(rb, "_current_runtime_target_artifact", lambda *a, **k: None)
    monkeypatch.setattr(rb, "_build_state_root", lambda _root: state_root)
    monkeypatch.setattr(rb, "_inspect_wasm_binary", lambda _path: "valid")
    monkeypatch.setattr(
        rb, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
    monkeypatch.setattr(
        rb.wasm_toolchain, "rust_target_libdir", lambda *a, **k: tmp_path
    )

    assert (
        rb._prepopulate_combined_runtime_wasm_target(
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
        return rb._runtime_target_fingerprint_path(
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
# build -- while honouring the dual-compile kill switch.
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
    reloc_ret: bool,
    shared_ret: bool,
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
        runtime_wasm=runtime_shared_missing,
        runtime_reloc_wasm=runtime_reloc,
        ensure_runtime_wasm_shared=_record_ensure("shared", shared_ret, log),
        ensure_runtime_wasm_reloc=_record_ensure("reloc", reloc_ret, log),
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
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=True,
        reloc_ret=True,
        shared_ret=True,
        both_ret=True,
    )
    # ONE combined ensure; the standalone reloc/shared ensures are NOT invoked.
    assert log == ["both"]


def test_app_path_dual_compile_kill_switch_forces_two_ensures(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    monkeypatch.setenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", "1")
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=True,
        reloc_ret=True,
        shared_ret=True,
        both_ret=True,
    )
    # Kill switch -> separate reloc + shared ensures; combined is NOT invoked.
    assert log == ["reloc", "shared"]


def test_app_path_single_compile_off_forces_two_ensures(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    monkeypatch.setenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", "0")
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=True,
        reloc_ret=True,
        shared_ret=True,
        both_ret=True,
    )
    assert log == ["reloc", "shared"]


def test_app_path_legacy_fallback_when_combined_absent(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=False,
        both_callable=False,
        reloc_ret=True,
        shared_ret=True,
        both_ret=True,
    )
    # No combined callable supplied -> proven sequential reloc + shared.
    assert log == ["reloc", "shared"]


def test_app_path_freestanding_routes_reloc_only(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    log = _run_app_ensure_routing(
        monkeypatch,
        tmp_path,
        freestanding=True,
        both_callable=True,
        reloc_ret=False,  # False -> _fail immediately after the reloc ensure
        shared_ret=True,
        both_ret=True,
    )
    # Freestanding needs only the reloc runtime -- never the combined/shared.
    assert log == ["reloc"]


def test_app_dual_compile_forced_env_parsing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", raising=False)
    assert nno._app_split_runtime_dual_compile_forced() is False
    for on in ("1", "true", "yes", "on", "ON"):
        monkeypatch.setenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", on)
        assert nno._app_split_runtime_dual_compile_forced() is True
    monkeypatch.delenv("MOLT_RUNTIME_WASM_DUAL_COMPILE", raising=False)
    # The landed single-compile authority OFF also forces the dual route.
    for off in ("0", "false", "no", "off"):
        monkeypatch.setenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", off)
        assert nno._app_split_runtime_dual_compile_forced() is True
    monkeypatch.setenv("MOLT_RUNTIME_WASM_SINGLE_COMPILE", "1")
    assert nno._app_split_runtime_dual_compile_forced() is False
