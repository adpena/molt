from __future__ import annotations

from functools import partial

import importlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from molt._wasm_runtime_exports import wasm_split_runtime_export_rename_map
from molt.cli import wasm_link_inputs
from molt.cli.models import _RuntimeArtifactState
from molt.cli import runtime_wasm_pair_build as RUNTIME_WASM_PAIR
from molt.cli import artifact_state as ARTIFACT_STATE
from molt.cli import runtime_build_identity as BUILD_IDENTITY
from tests.runtime_build_identity_helper import (
    RuntimeFixtureRoot,
    bind_runtime_wasm_specs,
    runtime_cargo_plan,
    runtime_wasm_link_inputs,
    runtime_build_identity as make_runtime_build_identity,
)

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
RUNTIME_WASM_FAILURE = importlib.import_module("molt.cli.runtime_wasm_failure")
WASM_TOOLCHAIN = importlib.import_module("molt.cli.wasm_toolchain")
WASM_LINK_ARGS = importlib.import_module("molt.cli.wasm_link_args")

_TEST_RUNTIME_FINGERPRINT_HASH = "ab" * 32


def _valid_wasm_bytes(label: bytes = b"") -> bytes:
    if not label:
        return wasm_artifact._build_wasm_sections([])
    payload = wasm_artifact._write_wasm_string("molt.test") + label
    return wasm_artifact._build_wasm_sections([(0, payload)])


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


def test_prebuild_runtime_wasm_json_preserves_exact_failure_authority(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def fail_with_evidence(runtime_state, **_kwargs):  # noqa: ANN001, ANN003
        return RUNTIME_WASM_FAILURE.record_runtime_wasm_failure(
            runtime_state,
            project_root=tmp_path,
            stage="identity-provisioning",
            summary="Runtime WASM identity provisioning failed: poisoned sysroot",
        )

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_ensure_runtime_wasm_both",
        fail_with_evidence,
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

    captured = capsys.readouterr()
    assert "poisoned sysroot" in captured.err
    payload = json.loads(captured.out)
    assert payload["status"] == "error"
    assert payload["failure"]["stage"] == "identity-provisioning"
    assert "poisoned sysroot" in payload["failure"]["summary"]
    evidence_path = Path(payload["failure"]["evidence_path"])
    assert json.loads(evidence_path.read_text(encoding="utf-8"))["schema"] == (
        "molt.runtime-wasm-build-failure.v1"
    )


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

    runtime_state = _RuntimeArtifactState(
        runtime_wasm=runtime_reloc.with_name("molt_runtime.wasm"),
        runtime_reloc_wasm=runtime_reloc,
    )
    assert RUNTIME_WASM_PAIR._ensure_runtime_wasm_both(
        runtime_state,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=300.0,
        project_root=project_root,
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="full",
        resolved_modules=None,
        required_exports=None,
    )
    runtime_reloc = runtime_state.runtime_reloc_wasm_selected
    assert runtime_reloc is not None

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


def test_link_runtime_staticlib_to_reloc_wasm_uses_absolute_paths(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
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
        wasm_link_inputs, "wasm_wasi_libc_archive", lambda: libc, raising=True
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
    inputs = runtime_wasm_link_inputs(runtime_fixture_root)
    assert RUNTIME_WASM_BUILD_SUPPORT._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=output,
        json_output=True,
        link_timeout=5.0,
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={},
            cargo_command=("cargo",),
        ),
        link_inputs=inputs,
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

    assert wasm_link_inputs.normalize_wasi_sysroot(root) == root.resolve(strict=False)
    assert wasm_link_inputs.normalize_wasi_sysroot(target_include) == root.resolve(
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
    python_wasm_link_inputs = (
        repo_root / "src" / "molt" / "cli" / "wasm_link_inputs.py"
    ).read_text(encoding="utf-8")
    assert "/usr/include/wasm32-wasi" in python_wasm_link_inputs
    assert "WASI_SDK_PREFIX" in python_wasm_link_inputs
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
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
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
        wasm_link_inputs,
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

    inputs = runtime_wasm_link_inputs(runtime_fixture_root)
    assert RUNTIME_WASM_BUILD_SUPPORT._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=runtime_wasm,
        json_output=True,
        link_timeout=5.0,
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={},
            cargo_command=("cargo",),
        ),
        link_inputs=inputs,
        export_link_args=export_link_args,
    )

    cmd = captured["cmd"]
    assert cmd[:2] == [str(inputs.linker.entrypoint), "-r"]
    assert cmd[2].startswith("@")
    response_text = Path(cmd[2].removeprefix("@")).read_text(encoding="utf-8")
    assert "--export-if-defined=molt_reloc_required_export" in response_text
    assert "--export-if-defined=molt_reloc_other_export" in response_text
    assert cmd[3:5] == ["--whole-archive", str(staticlib)]
    assert "--no-whole-archive" in cmd
    no_whole_index = cmd.index("--no-whole-archive")
    assert cmd[no_whole_index + 1] == str(inputs.libc.path)
    assert captured["kwargs"]["memory_guard_prefix"] == "MOLT_WASM_LINK"


@pytest.fixture
def validation_specs(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
):
    """Real feature/flag plans and exact family receipts, no tool probes/builds."""
    root = tmp_path / "repo with spaces"
    root.mkdir()
    target = tmp_path / "target with spaces"
    state_root = tmp_path / "state with spaces"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "resolve_runtime_wasm_link_inputs",
        lambda **kwargs: runtime_wasm_link_inputs(
            runtime_fixture_root, env=kwargs["env"]
        ),
    )
    monkeypatch.setattr(WASM_LINK_ARGS, "_build_state_root", lambda _root: state_root)
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        RUNTIME_WASM_PAIR, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_build_state_root", lambda _root: state_root
    )
    monkeypatch.setattr(
        RUNTIME_WASM_FAILURE, "_build_state_root", lambda _root: state_root
    )

    def specs(
        *,
        family_seed="family",
        compile_seed=None,
        required_exports=None,
        profile="dev-fast",
        resolved_modules=None,
    ):
        common = dict(
            cargo_profile=profile,
            simd_enabled=True,
            freestanding=False,
            stdlib_profile="full",
            resolved_modules=resolved_modules,
            required_link_features=frozenset(),
            required_exports=required_exports,
        )
        shared = RUNTIME_WASM_BUILD_SPEC._compute_runtime_wasm_build_spec(
            root, root / "molt_runtime.wasm", reloc=False, **common
        )
        reloc = RUNTIME_WASM_BUILD_SPEC._compute_runtime_wasm_build_spec(
            root, root / "molt_runtime_reloc.wasm", reloc=True, **common
        )
        shared, reloc = bind_runtime_wasm_specs(
            shared._replace(target_root=target),
            reloc._replace(target_root=target),
            root=root,
            family_seed=family_seed,
            compile_seed=compile_seed,
        )
        return root, target, state_root, shared, reloc

    return specs


def _combined_context(root, shared, reloc, state=None):
    return RUNTIME_WASM_PAIR._CombinedRuntimeWasmBuild(
        state or _RuntimeArtifactState(),
        shared,
        reloc,
        True,
        5.0,
        root,
        True,
        False,
    )


@pytest.mark.parametrize(
    "reloc,profile,preserve",
    [
        (True, "release-output", True),
        (False, "release-output", False),
        (False, "dev-fast", True),
    ],
)
def test_publication_finalizer_uses_target_and_resolved_profile_policy(
    validation_specs,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    reloc: bool,
    profile: str,
    preserve: bool,
) -> None:
    root, _target, _state, shared, reloc_spec = validation_specs(profile=profile)
    path = tmp_path / "runtime.wasm"
    path.write_bytes(_valid_wasm_bytes())
    seen = {}

    def transform(candidate, **kwargs):
        assert candidate == path
        seen.update(kwargs)
        return SimpleNamespace(
            input_bytes=8,
            output_bytes=8,
            scanned_bytes=8,
            written_bytes=0,
            max_buffer_bytes=8,
            changed=False,
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "transform_wasm_publication_file", transform
    )
    member = RUNTIME_WASM_BUILD._RuntimeWasmMemberFinalizer(
        path,
        reloc,
        True,
        5.0,
        root,
        frozenset({"PyLong_FromLong"}),
        reloc_spec if reloc else shared,
    )
    assert member.finalize_publication()
    assert seen["final_artifact"] is not reloc
    assert seen["preserve_debug"] is preserve
    if reloc:
        assert seen["rename_map"] == {}
    else:
        # An app's required subset must not shape the shared runtime ABI.
        rename_map = seen["rename_map"]
        assert rename_map == wasm_split_runtime_export_rename_map(None)
        assert rename_map["PyLong_FromLong"] == "molt_PyLong_FromLong"
        assert rename_map["PyType_Ready"] == "molt_PyType_Ready"


def test_reloc_runtime_publication_preserves_linker_metadata_bytes(
    validation_specs, tmp_path: Path
) -> None:
    root, _target, _state, _shared, reloc = validation_specs(profile="release-output")
    sections = [
        (0, wasm_artifact._write_wasm_string(name) + b"payload")
        for name in ("name", ".debug_info", "linking", "reloc.CODE")
    ]
    data = wasm_artifact._build_wasm_sections(sections)
    path = tmp_path / "reloc.wasm"
    path.write_bytes(data)
    member = RUNTIME_WASM_BUILD._RuntimeWasmMemberFinalizer(
        path, True, True, 5.0, root, None, reloc
    )
    assert member.finalize_publication()
    assert path.read_bytes() == data


@pytest.mark.parametrize(
    "mode", ["invalid-artifact", "cargo-failure", "missing-target", "timeout"]
)
def test_combined_build_failure_has_no_publication_and_exact_signal(
    validation_specs,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mode: str,
) -> None:
    root, target, _state, shared, reloc = validation_specs()
    state = _RuntimeArtifactState()
    calls = []
    monkeypatch.setattr(
        RUNTIME_WASM_PAIR,
        "_current_runtime_target_artifact",
        lambda *args, **kwargs: None,
    )
    if mode == "missing-target":
        assert shared.cargo_plan is not None
        shared.cargo_plan.rust_resources.files[0].identity.path.unlink()

    def build(*, cargo_plan, **kwargs):
        calls.append(cargo_plan.command)
        if mode == "missing-target":
            pytest.fail("missing target must not invoke Cargo")
        if mode == "timeout":
            raise subprocess.TimeoutExpired(
                cargo_plan.command,
                5.0,
                output=b"partial stdout",
                stderr=b"partial stderr",
            )
        runtime = target / "wasm32-wasip1" / shared.profile_dir / "molt_runtime.wasm"
        staticlib = runtime.with_name("libmolt_runtime.a")
        runtime.parent.mkdir(parents=True, exist_ok=True)
        runtime.write_bytes(b"\x00" * 64)
        staticlib.write_bytes(b"!<arch>\n")
        stdout = json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": "path+file:///runtime/molt-runtime#0.0.1",
                "target": {"name": "molt_runtime"},
                "filenames": [str(runtime), str(staticlib)],
            }
        )
        return subprocess.CompletedProcess(
            cargo_plan.command,
            101 if mode == "cargo-failure" else 0,
            stdout,
            "error: wasi sysroot authority did not reach runtime build"
            if mode == "cargo-failure"
            else "",
        ), runtime

    monkeypatch.setattr(RUNTIME_WASM_PAIR, "_run_runtime_wasm_cargo_build", build)
    assert not RUNTIME_WASM_PAIR._prepopulate_combined_runtime_wasm_target(
        runtime_state=state,
        shared_spec=shared,
        reloc_spec=reloc,
        json_output=True,
        cargo_timeout=5.0,
        project_root=root,
        simd_enabled=True,
        freestanding=False,
    )
    assert len(calls) == (0 if mode == "missing-target" else 1)
    assert state.runtime_wasm_build_failure is not None
    failure = state.runtime_wasm_build_failure
    assert (
        failure.stage
        == {
            "missing-target": "combined-cargo-admission",
            "cargo-failure": "combined-cargo",
            "timeout": "combined-cargo",
            "invalid-artifact": "combined-cdylib-validation",
        }[mode]
    )
    assert failure.evidence_path is not None
    evidence = json.loads(failure.evidence_path.read_text(encoding="utf-8"))
    assert evidence["stage"] == failure.stage
    if mode == "cargo-failure":
        assert "wasi sysroot authority" in evidence["stderr"]
        assert evidence["returncode"] == 101
        assert evidence["details"]["cargo_execution"]["attempts"]
    if mode == "timeout":
        assert evidence["timed_out"] is True
        assert evidence["stdout"] == "partial stdout"
        assert evidence["stderr"] == "partial stderr"
    assert not (root / "molt_runtime.wasm").exists()


@pytest.mark.parametrize("mismatch", ["feature-shape", "shared-import-abi"])
def test_target_pair_rejects_newer_but_semantically_incompatible_artifacts(
    validation_specs,
    monkeypatch: pytest.MonkeyPatch,
    mismatch: str,
) -> None:
    root, target, state_root, old_shared, old_reloc = validation_specs(
        family_seed="old"
    )
    if mismatch == "feature-shape":
        _, _, _, shared, reloc = validation_specs(family_seed="new")
    else:
        shared, reloc = old_shared, old_reloc
    runtime = target / "wasm32-wasip1" / shared.profile_dir / "molt_runtime.wasm"
    archive = runtime.with_name("libmolt_runtime.a")
    runtime.parent.mkdir(parents=True, exist_ok=True)
    runtime.write_bytes(_valid_wasm_bytes(b"owned-memory-not-shared-import-abi"))
    archive.write_bytes(b"!<arch>\n")
    for path, fingerprint in (
        (runtime, old_shared.fingerprint),
        (archive, old_reloc.staticlib_fingerprint),
    ):
        receipt = ARTIFACT_STATE._runtime_target_fingerprint_path(
            state_root,
            path,
            cargo_profile=shared.cargo_profile,
            target_label="wasm32-wasip1",
        )
        RUNTIME_FINGERPRINTS._write_runtime_fingerprint(
            receipt, fingerprint, artifact=path
        )
    assert not _combined_context(root, shared, reloc).target_pair_is_current()


def test_full_profile_feature_receipt_matches_exact_combined_cargo_command(
    validation_specs,
) -> None:
    _root, _target, _state, shared, reloc = validation_specs(resolved_modules={"ssl"})
    assert shared.cargo_plan is reloc.cargo_plan
    command = shared.cargo_plan.command
    assert "--no-default-features" in command
    features = set(command[command.index("--features") + 1].split(","))
    assert features <= set(shared.fingerprint_features)
    assert "no-default-features" in shared.fingerprint_features
    assert {
        "stdlib_crypto",
        "stdlib_compression",
        "stdlib_logging_ext",
        "builtin_contextvars",
        "stdlib_micro",
    } <= features
    assert not {"molt_gpu_primitives", "sqlite"} & features
    assert "sqlite" not in shared.fingerprint_features
    selector = command.index("--crate-type")
    assert command[selector + 1] == "staticlib,cdylib"
    assert selector < command.index("--")


def test_shared_allowlist_is_response_content_not_compile_rustflags(
    validation_specs,
) -> None:
    _root, _target, state_root, shared, _reloc = validation_specs(
        required_exports={"add", "abc_abstractmethod_check"}
    )
    plan = shared.cargo_plan
    assert plan is not None
    assert not any(
        "--export-if-defined" in flag or "link-arg=@" in flag for flag in plan.rustflags
    )
    argument = next(value for value in plan.command if value.startswith("link-arg=@"))
    path = Path(argument.removeprefix("link-arg=@"))
    assert path.is_absolute() and " " in str(path)
    assert path.parent == state_root / "wasm_link_args"
    text = path.read_text(encoding="utf-8")
    for required in (
        "--import-memory",
        "--import-table",
        "--growable-table",
        "--export-if-defined=molt_add",
        "--export-if-defined=molt_abc_abstractmethod_check",
    ):
        assert required in text
    assert "--export-dynamic" not in text
    assert argument not in plan.partition_command()[0]


@pytest.mark.parametrize("explicit", [None, "0", "1"])
def test_runtime_wasm_incremental_policy_survives_plan_resolution(
    validation_specs, monkeypatch: pytest.MonkeyPatch, explicit: str | None
) -> None:
    monkeypatch.delenv("RUSTC_WRAPPER", raising=False)
    monkeypatch.setenv("MOLT_USE_SCCACHE", "0")
    if explicit is None:
        monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    else:
        monkeypatch.setenv("CARGO_INCREMENTAL", explicit)
    _root, _target, _state, shared, _reloc = validation_specs()
    assert shared.cargo_plan.environment["CARGO_INCREMENTAL"] == (explicit or "0")
    assert (
        shared.cargo_plan.environment["WASI_SYSROOT"]
        == shared.cargo_plan.environment["MOLT_WASI_SYSROOT"]
    )


def test_runtime_fingerprint_recomputes_when_rustflags_change() -> None:
    original = make_runtime_build_identity("shared")
    payload = original.to_dict()
    family = payload["payload"]["family"]
    family["compile"]["common_config"]["base_rustflags"] = ["-C", "panic=abort"]
    family["compile_digest"] = BUILD_IDENTITY._digest(family["compile"])
    changed = BUILD_IDENTITY.RuntimeBuildIdentity(
        BUILD_IDENTITY._digest(payload["payload"]),
        family["compile_digest"],
        BUILD_IDENTITY._digest(family),
        payload["payload"],
    )
    assert (
        BUILD_IDENTITY.runtime_build_fingerprint(original)["hash"]
        != BUILD_IDENTITY.runtime_build_fingerprint(changed)["hash"]
    )
