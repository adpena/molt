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
from pathlib import Path

import pytest

import molt.cli.runtime_build as rb
from molt.cli.compiler_metadata import _compiler_root


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


def test_final_required_export_abi_closes_the_cargo_feature_plan() -> None:
    root = _compiler_root()
    required_exports = {
        "molt_ast_parse",
        "molt_ctypes_sizeof",
        "molt_math_sqrt",
        "molt_re_compile",
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

    def _fake_build(*, cmd, root, env, cargo_timeout, profile_dir,
                    target_root_override, json_output, artifact_kind):
        captured["cmd"] = list(cmd)
        captured["env"] = dict(env)
        # Return a non-zero build so _prepopulate returns without touching disk.
        return (
            subprocess.CompletedProcess(cmd, 1, "", "stopped-for-test"),
            Path("unused"),
        )

    monkeypatch.setattr(rb, "_run_runtime_wasm_cargo_build", _fake_build)
    # Force a cold target dir so the fast-path reuse check misses and we build.
    monkeypatch.setattr(
        rb, "_current_runtime_target_artifact", lambda *a, **k: None
    )
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
    monkeypatch.setattr(
        nno, "_staged_artifact_runtime_export_symbols", lambda *a, **k: set()
    )

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
