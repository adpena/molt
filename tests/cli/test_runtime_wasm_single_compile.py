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
