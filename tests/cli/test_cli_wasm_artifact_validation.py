from __future__ import annotations

import contextlib
import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

import molt.cli as cli
import molt.wasm_artifact as wasm_artifact
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import entrypoint_dispatch, entrypoint_parser
from tests.cli.process_guard import run_cli_test_process

RUNTIME_FINGERPRINTS = importlib.import_module("molt.cli.runtime_fingerprints")
RUNTIME_BUILD = importlib.import_module("molt.cli.runtime_build")
RUNTIME_WASM_BUILD = importlib.import_module("molt.cli.runtime_wasm_build")
RUNTIME_WASM_BUILD_SPEC = importlib.import_module("molt.cli.runtime_wasm_build_spec")
RUNTIME_WASM_BUILD_SUPPORT = importlib.import_module(
    "molt.cli.runtime_wasm_build_support"
)
WASM_TOOLCHAIN = importlib.import_module("molt.cli.wasm_toolchain")
WASM_LINK_ARGS = importlib.import_module("molt.cli.wasm_link_args")

_TEST_RUNTIME_FINGERPRINT_HASH = "ab" * 32


def _test_runtime_fingerprint(seed: str = "ab") -> dict[str, object]:
    return {
        "hash": seed * 32,
        "inputs_digest": "cd" * 32,
        "meta_digest": "ef" * 32,
        "rustc": f"rustc fixture {seed}",
        "source_state": {"kind": "git-clean-head", "head": "12" * 32},
    }


def _valid_wasm_bytes(label: bytes = b"") -> bytes:
    if not label:
        return wasm_artifact._build_wasm_sections([])
    payload = wasm_artifact._write_wasm_string("molt.test") + label
    return wasm_artifact._build_wasm_sections([(0, payload)])


def _runtime_build_spec(
    tmp_path: Path, runtime_wasm: Path, fingerprint: dict[str, object]
):
    return RUNTIME_WASM_BUILD_SPEC._RuntimeWasmBuildSpec(
        requested_cargo_profile="dev-fast",
        cargo_profile="dev-fast",
        profile_dir="dev-fast",
        incremental_enabled=False,
        env={},
        runtime_exports="",
        link_flags="",
        cargo_link_response_path=None,
        cargo_rustflags="",
        fingerprint_rustflags="",
        no_default_features=False,
        wasm_cargo_features=(),
        fingerprint_features=(),
        fingerprint_path=tmp_path / "runtime.fingerprint",
        target_root=tmp_path / "target",
        stored_fingerprint=None,
        fingerprint=fingerprint,
        staticlib_fingerprint=fingerprint,
    )


@pytest.fixture(autouse=True)
def _isolated_runtime_wasm_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "runtime-cache"))
    from molt.cli import default_paths

    default_paths._default_molt_cache_cached.cache_clear()


def test_prebuild_runtime_wasm_routes_through_runtime_artifact_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime_root = tmp_path / "wasm-root"
    monkeypatch.setenv("MOLT_WASM_RUNTIME_DIR", str(runtime_root))
    calls: list[tuple[str, float | None, str | None, Path]] = []
    selected_shared = runtime_root / "molt_runtime.wasm.test.runtime-wasm-member"
    selected_reloc = runtime_root / "molt_runtime_reloc.wasm.test.runtime-wasm-member"
    generation = runtime_root / "molt_runtime.generation.json"

    def fake_ensure_runtime_wasm_both(
        runtime_state,
        *,
        json_output,
        cargo_profile,
        cargo_timeout,
        project_root,
        simd_enabled,
        freestanding,
        stdlib_profile,
        resolved_modules,
        required_exports,
    ) -> bool:
        del json_output, simd_enabled, freestanding, resolved_modules, required_exports
        assert runtime_state.runtime_wasm is not None
        assert runtime_state.runtime_reloc_wasm is not None
        runtime_state.runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
        selected_shared.write_bytes(_valid_wasm_bytes(b"shared"))
        selected_reloc.write_bytes(_valid_wasm_bytes(b"reloc"))
        generation.write_text("{}\n", encoding="utf-8")
        runtime_state.runtime_wasm_selected = selected_shared
        runtime_state.runtime_reloc_wasm_selected = selected_reloc
        runtime_state.runtime_wasm_generation = generation
        calls.append((cargo_profile, cargo_timeout, stdlib_profile, project_root))
        return True

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_ensure_runtime_wasm_both",
        fake_ensure_runtime_wasm_both,
        raising=True,
    )

    assert (
        RUNTIME_BUILD._prebuild_runtime_wasm(
            project_root=tmp_path,
            kind="shared",
            json_output=True,
            build_profile="dev",
            cargo_timeout=1200.0,
            simd_enabled=True,
            freestanding=False,
            stdlib_profile="micro",
        )
        == 0
    )

    assert calls == [("dev-fast", 1200.0, "micro", tmp_path)]
    payload = json.loads(capsys.readouterr().out)
    assert payload["artifacts"]["shared"] == str(selected_shared)
    assert payload["artifacts"]["generation"] == str(generation)


def test_prebuild_runtime_wasm_json_failure_is_machine_readable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_ensure_runtime_wasm_both",
        lambda *_args, **_kwargs: False,
        raising=True,
    )

    assert (
        RUNTIME_BUILD._prebuild_runtime_wasm(
            project_root=tmp_path,
            kind="shared",
            json_output=True,
            build_profile="dev",
            cargo_timeout=1200.0,
        )
        == 1
    )

    assert json.loads(capsys.readouterr().out) == {
        "error": "Runtime wasm pair prebuild failed.",
        "status": "error",
    }


def test_internal_runtime_wasm_build_cli_routes_to_runtime_prebuild(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, object]] = []

    def fake_prebuild_runtime_wasm(**kwargs: object) -> int:
        calls.append(kwargs)
        return 0

    monkeypatch.setattr(
        entrypoint_dispatch._runtime_build,
        "_prebuild_runtime_wasm",
        fake_prebuild_runtime_wasm,
        raising=True,
    )

    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(
        [
            "internal-runtime-wasm-build",
            "--build-profile",
            "dev",
            "--kind",
            "shared",
            "--cargo-timeout",
            "1200",
            "--json",
        ]
    )

    assert (
        entrypoint_dispatch._dispatch_entrypoint_command(
            args,
            build_fn=lambda **_: 0,
            config_root=tmp_path,
            config={},
            build_cfg={},
            run_cfg={},
            compare_cfg={},
            test_cfg={},
            diff_cfg={},
            extension_cfg={},
            publish_cfg={},
            cfg_capabilities=None,
        )
        == 0
    )
    assert calls == [
        {
            "project_root": tmp_path,
            "kind": "shared",
            "json_output": True,
            "build_profile": "dev",
            "cargo_timeout": 1200.0,
            "simd_enabled": True,
            "freestanding": False,
            "stdlib_profile": None,
            "verbose": False,
        }
    ]


def test_is_valid_wasm_binary_accepts_structural_empty_module(
    tmp_path: Path,
) -> None:
    artifact = tmp_path / "ok.wasm"
    artifact.write_bytes(_valid_wasm_bytes())
    assert wasm_artifact.inspect_wasm_binary(artifact) == "valid"
    assert wasm_artifact.is_valid_wasm_binary(artifact)


def test_is_valid_wasm_binary_rejects_trailing_junk(tmp_path: Path) -> None:
    artifact = tmp_path / "junk.wasm"
    artifact.write_bytes(b"\x00asm\x01\x00\x00\x00rest")
    assert wasm_artifact.inspect_wasm_binary(artifact) == "invalid"
    assert not wasm_artifact.is_valid_wasm_binary(artifact)


def test_is_valid_wasm_binary_rejects_zero_filled_file(tmp_path: Path) -> None:
    artifact = tmp_path / "bad.wasm"
    artifact.write_bytes(b"\x00" * 32)
    assert wasm_artifact.inspect_wasm_binary(artifact) == "invalid"
    assert not wasm_artifact.is_valid_wasm_binary(artifact)


def test_inspect_wasm_binary_reports_missing(tmp_path: Path) -> None:
    artifact = tmp_path / "missing.wasm"
    assert wasm_artifact.inspect_wasm_binary(artifact) == "missing"


@pytest.mark.slow
def test_ensure_runtime_reloc_wasm_exports_wasi_clock_ids(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo required")
    wasm_objdump = shutil.which("wasm-objdump")
    if wasm_objdump is None:
        pytest.skip("wasm-objdump required")

    project_root = Path(__file__).resolve().parents[2]
    runtime_reloc = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.setenv("MOLT_BACKEND_DAEMON", "0")

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_reloc,
        reloc=True,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=300.0,
        project_root=project_root,
    )

    result = run_cli_test_process(
        [wasm_objdump, "-x", str(runtime_reloc)],
        text=True,
        cwd=project_root,
        check=True,
    )
    exports = result.stdout or ""
    assert "D <_CLOCK_PROCESS_CPUTIME_ID> [ undefined" not in exports
    assert "D <_CLOCK_THREAD_CPUTIME_ID> [ undefined" not in exports
    assert "D <_CLOCK_PROCESS_CPUTIME_ID>" in exports
    assert "D <_CLOCK_THREAD_CPUTIME_ID>" in exports


def test_reloc_runtime_publication_preserves_linker_metadata_bytes() -> None:
    reloc = b"\0asm\x01\0\0\0linking-reloc-debug-metadata-must-remain-byte-identical"

    assert (
        RUNTIME_WASM_BUILD._runtime_publication_bytes(
            reloc, reloc=True, preserve_debug=False
        )
        == reloc
    )


def test_shared_runtime_publication_still_uses_final_artifact_strip(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: dict[str, object] = {}

    def fake_strip(data: bytes, **kwargs: object) -> bytes:
        seen.update(kwargs)
        return data + b"-stripped"

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "strip_wasm_publication_sections", fake_strip
    )

    assert (
        RUNTIME_WASM_BUILD._runtime_publication_bytes(
            b"shared", reloc=False, preserve_debug=False
        )
        == b"shared-stripped"
    )
    assert seen == {"final_artifact": True, "preserve_debug": False}


def test_ensure_runtime_wasm_fails_closed_on_invalid_cargo_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    primary_target = tmp_path / "target-primary"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(primary_target))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _test_runtime_fingerprint("01"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    seen_commands: list[list[str]] = []

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del env, cargo_timeout, json_output, artifact_kind
        target_root = target_root_override or cli._cargo_target_root(root)
        seen_commands.append(cmd)
        src = target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(b"\x00" * 64)
        return subprocess.CompletedProcess(cmd, 0, "", ""), src

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert not RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert len(seen_commands) == 1
    assert seen_commands[0][5] == "dev-fast"
    assert not runtime_wasm.exists()


def test_ensure_runtime_wasm_rebuilds_when_feature_shape_changes_even_if_artifact_is_newer(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo with spaces"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    target_root = tmp_path / "target with spaces"
    build_state_root = tmp_path / "build state with spaces"
    fingerprint_path = tmp_path / "fingerprint.json"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(_valid_wasm_bytes(b"old"))
    RUNTIME_FINGERPRINTS._write_runtime_fingerprint(
        fingerprint_path,
        {"hash": "old-shape"},
        artifact=runtime_wasm,
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_state_root",
        lambda _root: build_state_root,
        raising=True,
    )
    monkeypatch.setattr(
        WASM_LINK_ARGS,
        "_build_state_root",
        lambda _root: build_state_root,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _test_runtime_fingerprint("03"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_runtime_wasm_artifact",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    build_calls: list[tuple[tuple[str, ...], Path]] = []

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, profile_dir, json_output, artifact_kind
        target_root = target_root_override or cli._cargo_target_root(root)
        src = target_root / "wasm32-wasip1" / "dev-fast" / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(_valid_wasm_bytes(b"rebuilt"))
        build_calls.append((tuple(cmd), src))
        return subprocess.CompletedProcess(cmd, 0), src

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="micro",
        resolved_modules={"ssl"},
    )
    assert build_calls, "feature-shape changes must force a wasm runtime rebuild"
    cmd = build_calls[0][0]
    assert "--no-default-features" in cmd
    cmd_features = set(cmd[cmd.index("--features") + 1].split(","))
    assert "sqlite" not in cmd_features
    assert runtime_wasm.read_bytes() == _valid_wasm_bytes(b"rebuilt")


def test_ensure_runtime_wasm_rebuilds_prebuilt_missing_shared_import_abi(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    cargo_runtime = target_root / "wasm32-wasip1" / "dev-fast" / "molt_runtime.wasm"
    cargo_runtime.parent.mkdir(parents=True, exist_ok=True)
    cargo_runtime.write_bytes(_valid_wasm_bytes(b"owned-memory"))
    runtime_source = project_root / "runtime" / "molt-runtime" / "src" / "lib.rs"
    runtime_source.parent.mkdir(parents=True, exist_ok=True)
    runtime_source.write_text("// runtime source\n", encoding="utf-8")
    built_src = (
        target_root / "wasm32-wasip1" / "dev-fast" / "deps" / "molt_runtime-new.wasm"
    )
    built_src.parent.mkdir(parents=True, exist_ok=True)
    built_src.write_bytes(_valid_wasm_bytes(b"shared-imports"))

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _root, **_kwargs: [runtime_source],
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _test_runtime_fingerprint("04"),
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC, "_read_runtime_fingerprint", lambda path: None
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_artifact_newer_than_sources",
        lambda artifact, sources: Path(artifact) == cargo_runtime,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_inspect_wasm_binary", lambda path: "valid"
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_is_valid_runtime_wasm_artifact", lambda path: True
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda path: Path(path) == built_src,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_runtime_wasm_exports_satisfy",
        lambda path, required: True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_runtime_wasm_missing_exports",
        lambda path, required: set(),
    )
    # Shared-mode (reloc=False) export validation routes through the
    # split-runtime authorities.
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_split_runtime_wasm_exports_satisfy",
        lambda path, required: True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_split_runtime_wasm_missing_exports",
        lambda path, required: set(),
    )
    build_calls: list[list[str]] = []

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del root, env, cargo_timeout, profile_dir, target_root_override
        del json_output, artifact_kind
        build_calls.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0), built_src

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="micro",
        resolved_modules=None,
        required_exports={"molt_fast_list_append"},
    )

    assert build_calls, "prebuilt runtime without shared ABI must be rebuilt"
    assert runtime_wasm.read_bytes() == built_src.read_bytes()


def test_ensure_runtime_wasm_full_profile_fingerprint_matches_cargo_features(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    captured_fingerprint_features: list[tuple[str, ...]] = []
    build_cmds: list[list[str]] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    def fake_runtime_fingerprint(
        project_root: Path,
        **kwargs: object,
    ) -> dict[str, object]:
        del project_root
        captured_fingerprint_features.append(
            tuple(kwargs["runtime_features"])  # type: ignore[arg-type]
        )
        return _test_runtime_fingerprint("05")

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        fake_runtime_fingerprint,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_read_runtime_fingerprint",
        lambda path: None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_artifact_newer_than_sources",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda *args, **kwargs: True,
        raising=True,
    )

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del env, cargo_timeout, json_output, artifact_kind
        build_cmds.append(list(cmd))
        src_root = target_root_override or cli._cargo_target_root(root)
        src = src_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(_valid_wasm_bytes(b"full"))
        return subprocess.CompletedProcess(cmd, 0), src

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="full",
        resolved_modules={"ssl"},
    )

    assert build_cmds
    cmd = build_cmds[0]
    assert "--no-default-features" in cmd
    cmd_features = set(cmd[cmd.index("--features") + 1].split(","))
    fingerprint_features = set(captured_fingerprint_features[0])
    assert cmd_features <= fingerprint_features
    assert "no-default-features" in fingerprint_features
    assert {
        "stdlib_crypto",
        "stdlib_compression",
        "stdlib_logging_ext",
        "builtin_contextvars",
    } <= cmd_features
    assert "molt_gpu_primitives" not in cmd_features
    assert "stdlib_micro" in cmd_features
    assert "sqlite" not in cmd_features
    assert "sqlite" not in fingerprint_features


def test_run_subprocess_captured_to_tempfiles_respects_cwd(tmp_path: Path) -> None:
    workdir = tmp_path / "work"
    workdir.mkdir()
    result = cli._run_subprocess_captured_to_tempfiles(
        [
            sys.executable,
            "-c",
            "import os,sys; sys.stdout.write(os.getcwd())",
        ],
        cwd=workdir,
    )
    assert result.returncode == 0
    assert os.path.samefile(result.stdout.decode("utf-8"), workdir)


def test_runtime_fingerprint_recomputes_when_rustflags_change(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _root, **_kwargs: (),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_hash_source_tree_metadata",
        lambda *args, **kwargs: ("same-inputs", 0),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc test", raising=True
    )

    first = cli._runtime_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        target_triple="wasm32-wasip1",
        rustflags="-C link-arg=--export-if-defined=molt_a",
        runtime_features=("stdlib_micro",),
        stored_fingerprint=None,
    )
    assert first is not None

    second = cli._runtime_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        target_triple="wasm32-wasip1",
        rustflags="-C link-arg=--export-if-defined=molt_b",
        runtime_features=("stdlib_micro",),
        stored_fingerprint=first,
    )
    assert second is not None
    assert second["hash"] != first["hash"]


def test_backend_fingerprint_recomputes_when_rustflags_change(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()

    monkeypatch.setattr(
        cli_backend_binary, "_backend_source_paths", lambda *_args: (), raising=True
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_hash_source_tree_metadata",
        lambda *args, **kwargs: ("same-inputs", 0),
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_rustc_version", lambda: "rustc test", raising=True
    )

    first = cli_backend_binary._backend_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        rustflags="-C link-arg=--export-if-defined=molt_a",
        backend_features=("wasm-backend",),
        stored_fingerprint=None,
    )
    assert first is not None

    second = cli_backend_binary._backend_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        rustflags="-C link-arg=--export-if-defined=molt_b",
        backend_features=("wasm-backend",),
        stored_fingerprint=first,
    )
    assert second is not None
    assert second["hash"] != first["hash"]


def test_ensure_runtime_wasm_shared_uses_response_file_for_export_allowlist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path / "repo with spaces"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    target_root = tmp_path / "target with spaces"
    build_state_root = tmp_path / "build state with spaces"
    export_flags = (
        " -C link-arg=--export-if-defined=molt_required_export"
        " -C link-arg=--export-if-defined=molt_other_required_export"
    )
    export_link_arg_calls: list[frozenset[str] | None] = []

    def fake_wasm_runtime_shared_export_link_args(
        required_runtime_imports=None,
    ):  # type: ignore[no-untyped-def]
        export_link_arg_calls.append(
            None
            if required_runtime_imports is None
            else frozenset(required_runtime_imports),
        )
        return export_flags

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_state_root",
        lambda _root: build_state_root,
        raising=True,
    )
    monkeypatch.setattr(
        WASM_LINK_ARGS,
        "_build_state_root",
        lambda _root: build_state_root,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "wasm_runtime_shared_export_link_args",
        fake_wasm_runtime_shared_export_link_args,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_runtime_wasm_missing_exports",
        lambda path, required: set(),
        raising=True,
    )
    # Shared-mode (reloc=False) export validation routes through the
    # split-runtime authority.
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_split_runtime_wasm_missing_exports",
        lambda path, required: set(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda path: True,
        raising=True,
    )
    fingerprint_rustflags: list[str] = []

    def fake_runtime_fingerprint(*args, **kwargs):  # type: ignore[no-untyped-def]
        fingerprint_rustflags.append(kwargs["rustflags"])
        return _test_runtime_fingerprint("06")

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        fake_runtime_fingerprint,
        raising=True,
    )

    captured: dict[str, object] = {}

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, json_output, artifact_kind
        captured["cmd"] = list(cmd)
        captured["root"] = root
        captured["env"] = dict(env)
        effective_target_root = target_root_override or cli._cargo_target_root(root)
        artifact = (
            effective_target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        )
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(_valid_wasm_bytes(b"shared"))
        return subprocess.CompletedProcess(cmd, 0, "", ""), artifact

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="micro",
        resolved_modules={"__main__", "math", "sys", "builtins"},
        required_exports={"molt_required_export", "molt_other_required_export"},
    )

    assert export_link_arg_calls == [
        frozenset({"molt_required_export", "molt_other_required_export"})
    ]
    cargo_rustflags = captured["env"]["RUSTFLAGS"]
    assert "--export-if-defined=molt_required_export" not in cargo_rustflags
    assert "link-arg=@" not in cargo_rustflags
    cargo_cmd = captured["cmd"]
    response_arg = next(
        arg
        for arg in cargo_cmd
        if isinstance(arg, str) and arg.startswith("link-arg=@")
    )
    response_path = Path(response_arg.removeprefix("link-arg=@"))
    assert " " in str(response_path)
    assert response_path.parent == build_state_root / "wasm_link_args"
    response_text = response_path.read_text(encoding="utf-8")
    assert "--import-memory" in response_text
    assert "--import-table" in response_text
    assert "--growable-table" in response_text
    assert "--export-if-defined=molt_required_export" in response_text
    assert "--export-if-defined=molt_other_required_export" in response_text
    assert "--export-dynamic" not in response_text
    assert fingerprint_rustflags
    assert any(
        "--export-if-defined=molt_required_export" in rustflags
        for rustflags in fingerprint_rustflags
    )
    # The final-publication fingerprint includes the allowlist; the staticlib
    # compile identity intentionally excludes link-only export policy.
    assert "--export-if-defined=molt_required_export" not in fingerprint_rustflags[-1]
    assert "--export-dynamic" not in fingerprint_rustflags[-1]


def test_wasm_link_args_response_file_path_is_absolute(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)

    response_path = WASM_LINK_ARGS.write_wasm_link_args_response_file(
        Path("relative") / ".molt_link_args",
        label="molt runtime reloc",
        link_args=["--export-if-defined=molt_required_export"],
    )

    assert response_path.is_absolute()
    assert response_path.read_text(encoding="utf-8") == (
        "--export-if-defined=molt_required_export\n"
    )


def test_ensure_runtime_wasm_reloc_requests_staticlib_build(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    target_root = tmp_path / "target"
    export_flags = " -C link-arg=--export-if-defined=molt_reloc_required_export"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "wasm_runtime_export_link_args",
        lambda *args, **kwargs: export_flags,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _test_runtime_fingerprint("07"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    captured: dict[str, object] = {}

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, json_output
        captured["cmd"] = list(cmd)
        captured["root"] = root
        captured["env"] = dict(env)
        captured["profile_dir"] = profile_dir
        captured["artifact_kind"] = artifact_kind
        captured["target_root_override"] = target_root_override
        effective_target_root = target_root_override or cli._cargo_target_root(root)
        staticlib_path = (
            effective_target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"
        )
        staticlib_path.parent.mkdir(parents=True, exist_ok=True)
        staticlib_path.write_bytes(b"archive")
        return subprocess.CompletedProcess(cmd, 0, "", ""), staticlib_path

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
        export_link_args: str = "",
        long_double_required: bool = False,
    ) -> bool:
        del json_output, link_timeout, long_double_required
        captured["linked_staticlib_path"] = staticlib_path
        captured["export_link_args"] = export_link_args
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(_valid_wasm_bytes(b"reloc"))
        return True

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        runtime_wasm,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="micro",
        resolved_modules={"__main__", "math", "sys", "builtins"},
    )
    assert captured["artifact_kind"] == "staticlib"
    assert captured["profile_dir"] == "release-fast"
    cmd = captured["cmd"]
    assert cmd[:2] == ["cargo", "rustc"]
    assert "--lib" in cmd
    selector = cmd.index("--crate-type")
    assert cmd[selector : selector + 2] == ["--crate-type", "staticlib"]
    assert "--" not in cmd
    cargo_rustflags = captured["env"].get("RUSTFLAGS", "")
    assert "--export-if-defined=molt_reloc_required_export" not in cargo_rustflags
    assert "link-arg=@" not in cargo_rustflags
    assert not any(
        isinstance(arg, str) and arg.startswith("link-arg=@") for arg in captured["cmd"]
    )
    assert captured["export_link_args"] == export_flags
    assert captured["linked_staticlib_path"] == (
        target_root / "wasm32-wasip1" / "release-fast" / "libmolt_runtime.a"
    )
    assert runtime_wasm.read_bytes() == _valid_wasm_bytes(b"reloc")


def test_link_runtime_staticlib_to_reloc_wasm_uses_absolute_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)
    staticlib = Path("target") / "wasm32-wasip1" / "release" / "libmolt_runtime.a"
    libc = Path("toolchain") / "wasm32-wasip1" / "libc.a"
    staticlib.parent.mkdir(parents=True)
    libc.parent.mkdir(parents=True)
    staticlib.write_bytes(b"archive")
    libc.write_bytes(b"libc")
    captured: dict[str, object] = {}

    monkeypatch.setattr(
        WASM_TOOLCHAIN,
        "resolve_wasm_linker",
        lambda: WASM_TOOLCHAIN.WasmLinkerIdentity(
            Path("wasm-ld"), "22.1.8", None, "a" * 64
        ),
        raising=True,
    )
    monkeypatch.setattr(
        WASM_TOOLCHAIN, "wasm_wasi_libc_archive", lambda: libc, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_is_valid_runtime_wasm_artifact",
        lambda path: True,
        raising=True,
    )

    def fake_run_completed_command(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None,
        capture_output: bool,
        memory_guard_prefix: str | None = None,
        timeout: float | None = None,
    ) -> subprocess.CompletedProcess[str]:
        del env, capture_output, memory_guard_prefix, timeout
        captured["cmd"] = list(cmd)
        captured["cwd"] = cwd
        output_path = Path(cmd[cmd.index("-o") + 1])
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(_valid_wasm_bytes(b"reloc"))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_run_completed_command",
        fake_run_completed_command,
        raising=True,
    )

    output = Path("runtime") / "molt_runtime_reloc.wasm"
    assert RUNTIME_WASM_BUILD_SUPPORT._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=output,
        json_output=True,
        link_timeout=5.0,
        export_link_args="-C link-arg=--export-if-defined=molt_required",
    )

    cmd = captured["cmd"]
    response_arg = next(arg for arg in cmd if arg.startswith("@"))
    assert Path(response_arg[1:]).is_absolute()
    assert Path(cmd[cmd.index("-o") + 1]).is_absolute()
    assert Path(cmd[cmd.index("--whole-archive") + 1]).is_absolute()
    assert Path(cmd[cmd.index("--no-whole-archive") + 1]).is_absolute()
    assert captured["cwd"] == output.resolve(strict=False).parent
    assert output.exists()


def test_ensure_runtime_wasm_defaults_cargo_incremental_off_and_preserves_explicit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    monkeypatch.delenv("RUSTC_WRAPPER", raising=False)
    monkeypatch.setenv("MOLT_WASM_DISABLE_SCCACHE", "1")
    wasi_sysroot = tmp_path / "wasi-sysroot"
    monkeypatch.delenv("WASI_SYSROOT", raising=False)
    monkeypatch.delenv("MOLT_WASI_SYSROOT", raising=False)
    monkeypatch.setattr(
        WASM_TOOLCHAIN,
        "resolve_wasi_sysroot",
        lambda: wasi_sysroot,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _test_runtime_fingerprint(
            "09" if os.environ.get("CARGO_INCREMENTAL") == "1" else "08"
        ),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_runtime_wasm_missing_exports",
        lambda path, required: set(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_is_valid_shared_runtime_wasm_artifact",
        lambda path: True,
        raising=True,
    )
    captured_envs: list[dict[str, str]] = []

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, json_output, artifact_kind
        captured_envs.append(dict(env))
        effective_target_root = target_root_override or cli._cargo_target_root(root)
        artifact = (
            effective_target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        )
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(_valid_wasm_bytes(b"ok"))
        return subprocess.CompletedProcess(cmd, 0, "", ""), artifact

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        tmp_path / "wasm" / "default" / "molt_runtime.wasm",
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert captured_envs[-1]["CARGO_INCREMENTAL"] == "0"
    assert captured_envs[-1]["WASI_SYSROOT"] == str(wasi_sysroot)
    assert captured_envs[-1]["MOLT_WASI_SYSROOT"] == str(wasi_sysroot)

    monkeypatch.setenv("CARGO_INCREMENTAL", "1")
    assert RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        tmp_path / "wasm" / "explicit" / "molt_runtime.wasm",
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert captured_envs[-1]["CARGO_INCREMENTAL"] == "1"
    assert captured_envs[-1]["WASI_SYSROOT"] == str(wasi_sysroot)
    assert captured_envs[-1]["MOLT_WASI_SYSROOT"] == str(wasi_sysroot)


def test_runtime_wasm_json_build_failure_emits_cargo_detail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "diagnostic"},
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del root, env, cargo_timeout, profile_dir, target_root_override, json_output
        del artifact_kind
        return (
            subprocess.CompletedProcess(
                cmd,
                101,
                "",
                "error: wasi sysroot authority did not reach runtime build",
            ),
            tmp_path / "missing.wasm",
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert not RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        tmp_path / "wasm" / "molt_runtime.wasm",
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    captured = capsys.readouterr()
    assert "Runtime wasm build failed" in captured.err
    assert "wasi sysroot authority" in captured.err


def test_runtime_wasm_missing_rust_target_fails_before_cargo(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "missing-target"},
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD.wasm_toolchain,
        "rust_target_libdir",
        lambda target: None,
        raising=True,
    )

    def fail_if_cargo_runs(
        **_kwargs: object,
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        raise AssertionError("runtime WASM Cargo build should not run")

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD,
        "_run_runtime_wasm_cargo_build",
        fail_if_cargo_runs,
        raising=True,
    )

    assert not RUNTIME_WASM_BUILD._ensure_runtime_wasm(
        tmp_path / "wasm" / "molt_runtime.wasm",
        reloc=False,
        json_output=False,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    captured = capsys.readouterr()
    assert "Runtime wasm build requires Rust target wasm32-wasip1" in captured.err


def test_wasi_sysroot_python_resolver_accepts_distro_target_include_layout(
    tmp_path: Path,
) -> None:
    root = tmp_path / "usr"
    host_include = root / "include"
    target_include = host_include / "wasm32-wasi"
    target_lib = root / "lib" / "wasm32-wasi"
    host_include.mkdir(parents=True)
    target_include.mkdir(parents=True)
    target_lib.mkdir(parents=True)
    (host_include / "errno.h").write_text("#define HOST_ERRNO 1\n", encoding="utf-8")
    (target_include / "errno.h").write_text("#define WASI_ERRNO 1\n", encoding="utf-8")

    assert WASM_TOOLCHAIN.normalize_wasi_sysroot(root) == root.resolve(strict=False)
    assert WASM_TOOLCHAIN.normalize_wasi_sysroot(target_include) == root.resolve(
        strict=False
    )


def test_runtime_build_scripts_share_wasi_sysroot_authority() -> None:
    repo_root = Path(__file__).resolve().parents[2]
    shared = repo_root / "runtime" / "build_support" / "wasi_sysroot.rs"
    runtime_build = repo_root / "runtime" / "molt-runtime" / "build.rs"
    abi_build = repo_root / "runtime" / "molt-cpython-abi" / "build.rs"

    shared_text = shared.read_text(encoding="utf-8")
    runtime_text = runtime_build.read_text(encoding="utf-8")
    abi_text = abi_build.read_text(encoding="utf-8")

    assert "MOLT_WASI_SYSROOT" in shared_text
    assert "WASI_SDK_PREFIX" in shared_text
    assert "MOLT_TARGET_ROOT" in shared_text
    assert "/usr/share/wasi-sysroot" in shared_text
    assert "/usr/include/wasm32-wasi" in shared_text
    assert "wasm32-wasi" in shared_text
    assert "include_dir: Some" in shared_text
    assert "pub fn sysroot_flag(&self) -> String" in shared_text
    assert 'sysroot.lib_dir("wasm32-wasip1")' in runtime_text
    assert shared_text.index("target_include_layout(&root") < shared_text.index(
        'root.join("include").join("errno.h")'
    )
    python_wasm_toolchain = (
        repo_root / "src" / "molt" / "cli" / "wasm_toolchain.py"
    ).read_text(encoding="utf-8")
    assert "/usr/include/wasm32-wasi" in python_wasm_toolchain
    assert "WASI_SDK_PREFIX" in python_wasm_toolchain
    assert "mod wasi_sysroot" in runtime_text
    assert "mod wasi_sysroot" in abi_text
    assert "build.flag(sysroot.sysroot_flag())" in runtime_text
    assert "build.flag(sysroot.sysroot_flag())" in abi_text
    assert "build.include(include_dir)" in runtime_text
    assert "build.include(include_dir)" in abi_text
    assert "fn resolve_wasi_sysroot" not in runtime_text
    assert "fn resolve_wasi_sysroot" not in abi_text
    assert "wasi-libc/share/wasi-sysroot" not in runtime_text
    assert "wasi-libc/share/wasi-sysroot" not in abi_text


def test_link_runtime_staticlib_to_reloc_wasm_does_not_whole_archive_libc(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"archive")
    runtime_wasm = tmp_path / "molt_runtime_reloc.wasm"
    libc_archive = tmp_path / "libc.a"
    libc_archive.write_bytes(b"libc")
    export_link_args = (
        " -C link-arg=--export-if-defined=molt_reloc_required_export"
        " -C link-arg=--export-if-defined=molt_reloc_other_export"
    )
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        captured["kwargs"] = dict(kwargs)
        output = Path(cmd[cmd.index("-o") + 1])
        output.write_bytes(_valid_wasm_bytes(b"reloc"))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        WASM_TOOLCHAIN,
        "resolve_wasm_linker",
        lambda: WASM_TOOLCHAIN.WasmLinkerIdentity(
            Path("/usr/bin/wasm-ld"), "22.1.8", None, "a" * 64
        ),
    )
    monkeypatch.setattr(
        WASM_TOOLCHAIN,
        "wasm_wasi_libc_archive",
        lambda: libc_archive,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_completed_command", fake_run, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_is_valid_runtime_wasm_artifact",
        lambda path: True,
        raising=True,
    )

    assert RUNTIME_WASM_BUILD_SUPPORT._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=runtime_wasm,
        json_output=True,
        link_timeout=5.0,
        export_link_args=export_link_args,
    )

    cmd = captured["cmd"]
    assert cmd[:2] == [str(Path("/usr/bin/wasm-ld")), "-r"]
    assert cmd[2].startswith("@")
    response_text = Path(cmd[2].removeprefix("@")).read_text(encoding="utf-8")
    assert "--export-if-defined=molt_reloc_required_export" in response_text
    assert "--export-if-defined=molt_reloc_other_export" in response_text
    assert cmd[3:5] == ["--whole-archive", str(staticlib)]
    assert "--no-whole-archive" in cmd
    no_whole_index = cmd.index("--no-whole-archive")
    assert cmd[no_whole_index + 1] == str(libc_archive)
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_WASM_LINK"
