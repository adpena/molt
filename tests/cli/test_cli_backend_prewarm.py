"""`internal-backend-build` prewarms exactly the backend compiler builds admit.

The build side of each comparison runs `molt build`'s own stages (dispatch,
preamble, roots, config, output layout, backend setup) and the prewarm runs the
hidden command; both are observed at the admission boundary they share
(`_ensure_backend_binary`), so any divergence in host profile, Cargo profile,
feature lane, binary path, Cargo timeout, or Rust CPU policy fails here.
"""

from __future__ import annotations

from molt.compilation_target import Target, TargetKind, admit_target

from molt.cli.cache_keys import FunctionDigests

from collections.abc import Mapping
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import pytest

from molt.backend_environment import CodegenSelection
from tests.cli.backend_generation_test_support import selected_backend
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_cache_setup as cli_backend_cache_setup
from molt.cli import backend_compile as cli_backend_compile
from molt.cli import build_inputs as cli_build_inputs
from molt.cli import entrypoint_dispatch, entrypoint_parser
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.build_output_layout import _resolve_build_output_layout
from molt.cli.build_request import BuildRequest
from molt.cli.models import _BackendCacheSetup
from molt.cli.project_roots import _find_project_root
from molt.cli.runtime_paths import _cargo_profile_dir, _cargo_target_root
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.exact_json import canonical_json_sha256
from molt.host_tool_products import HostToolProduct
from molt.source_root import compiler_source_root
from tests.compiler_identity_helper import (
    compiler_build_admission,
    compiler_cargo_stdout,
    write_compiler_lock,
    write_compiler_source,
)

_EXE = ".exe" if os.name == "nt" else ""
_MOLT_ROOT_MARKERS = (
    "Cargo.toml",
    "runtime/molt-runtime/Cargo.toml",
    "runtime/molt-backend/Cargo.toml",
    "src/molt/cli/__init__.py",
)
# Ambient inputs that backend selection, admission, or the layout read.
_AMBIENT_ENV = (
    "CARGO_TARGET_DIR",
    "MOLT_SESSION_ID",
    "MOLT_SESSION_ID_GENERATED",
    "MOLT_BUILD_STATE_DIR",
    "MOLT_EXT_ROOT",
    "MOLT_SOURCE_ROOT",
    "MOLT_PROJECT_ROOT",
    "MOLT_BUNDLE_ROOT",
    "MOLT_BACKEND",
    "MOLT_BACKEND_PROFILE",
    "MOLT_DEV_BACKEND_CARGO_PROFILE",
    "MOLT_RELEASE_BACKEND_CARGO_PROFILE",
    "MOLT_CARGO_TIMEOUT",
    "MOLT_PERF_PROFILE",
    "MOLT_SKIP_RUNTIME_REBUILD",
    "MOLT_SYSROOT",
    "MOLT_MODULE_CHUNK_OPS",
    "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
    "MOLT_PREFER_EXTERNAL_ARTIFACTS",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
)
# Every target whose build admits the molt-backend compiler (MLIR does not).
_TARGETS = (
    "native",
    "llvm",
    "wasm",
    "wasm-freestanding",
    "wasm32-wasip1",
    "wasm32-unknown-unknown",
    "luau",
    "rust",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
)
_ENVIRONMENTS: dict[str, dict[str, str]] = {
    "defaults": {},
    "dev-host": {"MOLT_BACKEND_PROFILE": "dev"},
    "release-cargo-override": {
        "MOLT_RELEASE_BACKEND_CARGO_PROFILE": "backend-prod",
        "MOLT_CARGO_TIMEOUT": "321",
    },
    "session-dev-cargo-override": {
        "MOLT_BACKEND_PROFILE": "dev",
        "MOLT_DEV_BACKEND_CARGO_PROFILE": "backend-iter",
        "MOLT_SESSION_ID": "wasm-ci",
    },
    "native-arch-rustflags": {
        "MOLT_PERF_PROFILE": "native-arch",
        "RUSTFLAGS": "-C debuginfo=1",
    },
    # The process environment never selects the codegen backend (HF-60).
    "ambient-backend-ignored": {"MOLT_BACKEND": "llvm"},
}


def _isolated_molt_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, **env: str
) -> Path:
    root = tmp_path / "molt"
    for marker in _MOLT_ROOT_MARKERS:
        (root / marker).parent.mkdir(parents=True, exist_ok=True)
        (root / marker).write_text("", encoding="utf-8")
    for name in _AMBIENT_ENV:
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOLT_USE_SCCACHE", "0")
    # The real Cargo capacity boundary stays in the path, but how full the
    # host disk is must not decide a unit test; capacity tests set their own.
    monkeypatch.setenv("MOLT_DISK_GUARD_HIGH_WATER_GB", "1")
    for name, value in env.items():
        monkeypatch.setenv(name, value)
    write_compiler_lock(root)
    monkeypatch.setenv("MOLT_SOURCE_ROOT", os.fspath(root))
    monkeypatch.chdir(root)
    # Compiler inputs follow explicit source selection, independently of cwd.
    return compiler_source_root()


def _dispatch(
    argv: list[str],
    *,
    build_fn: Any = None,
    build_cfg: dict[str, Any] | None = None,
) -> int:
    def unexpected_build(_request: BuildRequest) -> int:
        raise AssertionError("the prewarm must not build a program")

    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(argv)
    return entrypoint_dispatch._dispatch_entrypoint_command(
        args,
        parser=parser,
        build_fn=build_fn or unexpected_build,
        # Same config-root authority as `molt.cli.entrypoint.main`.
        config_root=_find_project_root(Path.cwd()),
        config={},
        build_cfg=build_cfg or {},
        run_cfg={},
        compare_cfg={},
        test_cfg={},
        diff_cfg={},
        extension_cfg={},
        publish_cfg={},
        cfg_capabilities=None,
    )


def _record_backend_admission(
    monkeypatch: pytest.MonkeyPatch,
) -> list[dict[str, object]]:
    admissions: list[dict[str, object]] = []

    def record(
        backend_bin: Path,
        *,
        cargo_timeout: float | None,
        json_output: bool,
        cargo_profile: str,
        project_root: Path,
        backend_features: tuple[str, ...],
        rust_cpu: RustCpuPolicy,
        stage_timings_ms: dict[str, float] | None = None,
        product: HostToolProduct = HostToolProduct.BACKEND,
        environment: Mapping[str, str] | None = None,
    ) -> cli_backend_binary._BackendBinaryEnsureResult:
        del json_output, stage_timings_ms, environment
        assert product is HostToolProduct.BACKEND
        admissions.append(
            {
                "binary": backend_bin,
                "product": product,
                "cargo_profile": cargo_profile,
                "features": backend_features,
                "project_root": project_root,
                "cargo_timeout": cargo_timeout,
                "rust_cpu": rust_cpu,
                # Ambient Rust flags are the caller's; neither command writes them.
                "rustflags": os.environ.get("RUSTFLAGS"),
            }
        )
        return cli_backend_binary._BackendBinaryEnsureResult(
            ok=False, detail="admission recorded", phase="recorded"
        )

    monkeypatch.setattr(cli_backend_binary, "_ensure_backend_binary", record)
    return admissions


def _build_layout(target: Target, tmp_path: Path, project_root: Path) -> Any:
    return _resolve_build_output_layout(
        target=target,
        trusted=False,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        artifacts_root=tmp_path / "artifacts",
        bin_root=tmp_path / "bin",
        output_root=tmp_path / "dist",
        output_base="app",
        out_dir_path=None,
        project_root=project_root,
    )


def _build_backend_setup(
    target: Target, tmp_path: Path, codegen_backend: str
) -> tuple[Any, Any]:
    """Run `molt build --build-profile dev` from its preamble to backend setup."""
    codegen = CodegenSelection(backend=codegen_backend)  # type: ignore[arg-type]
    preamble, error = cli_build_inputs._prepare_build_preamble(
        diagnostics=None,
        diagnostics_file=None,
        diagnostics_verbosity=None,
        json_output=True,
        target=target,
    )
    assert error is None and preamble is not None
    roots, error = cli_build_inputs._prepare_build_roots(
        file_path=None,
        json_output=True,
        warnings=preamble.warnings,
        deterministic=False,
        deterministic_warn=False,
        sysroot=None,
    )
    assert error is None and roots is not None
    config, error = cli_build_inputs._prepare_build_config(
        project_root=roots.project_root,
        warnings=preamble.warnings,
        json_output=True,
        target=target,
        profile="dev",
        pgo_profile=None,
        runtime_feedback=None,
        capabilities=None,
        codegen=codegen,
    )
    assert error is None and config is not None
    layout = _build_layout(target, tmp_path, roots.project_root)
    return cli_backend_compile._prepare_backend_setup(
        function_digests=FunctionDigests(),
        split_runtime=False,
        is_rust_transpile=layout.is_rust_transpile,
        is_luau_transpile=layout.is_luau_transpile,
        is_wasm=layout.is_wasm,
        is_wasm_freestanding=layout.is_wasm_freestanding,
        emit_mode=layout.emit_mode,
        molt_root=roots.molt_root,
        runtime_cargo_profile=config.runtime_cargo_profile,
        target_triple=layout.target_triple,
        json_output=True,
        cargo_timeout=config.cargo_timeout,
        target=target,
        profile="dev",
        backend_cargo_profile=config.backend_cargo_profile,
        linked=layout.linked,
        project_root=roots.project_root,
        cache_dir=None,
        output_artifact=layout.output_artifact,
        warnings=preamble.warnings,
        cache=False,
        ir={"functions": []},
        entry_module="__main__",
        module_graph_metadata=object(),  # type: ignore[arg-type]
        target_python=config.target_python,
        codegen=config.codegen,
        rust_cpu=preamble.rust_cpu,
    )


def _fake_backend_toolchain(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[list[list[str]], list[list[str]]]:
    """Keep admission/receipts real around fixture plans and external processes."""
    from molt.cli import compiler_identity

    cargo_calls: list[list[str]] = []
    probe_calls: list[list[str]] = []

    def admission(
        root,
        features,
        profile,
        environment,
        *,
        rust_cpu,
        product=HostToolProduct.BACKEND,
    ):
        assert product is HostToolProduct.BACKEND
        write_compiler_source(root, product=product)
        selected_environment = {
            **environment,
            "CARGO_TARGET_DIR": os.fspath(_cargo_target_root(root)),
        }
        admitted = compiler_build_admission(
            features, profile, selected_environment, rust_cpu, product=product
        )
        admitted.plan.project_root = root
        return admitted

    def fingerprint(
        project_root: Path,
        *,
        cargo_profile: str,
        build_admission: Any,
        backend_features: tuple[str, ...],
        stored_fingerprint: object = None,
        product: HostToolProduct = HostToolProduct.BACKEND,
    ) -> dict[str, str]:
        assert product is HostToolProduct.BACKEND
        del project_root, stored_fingerprint
        return {
            "hash": canonical_json_sha256(
                [cargo_profile, build_admission.fingerprint, list(backend_features)]
            ),
            "rustc": "rustc-fixture",
            "inputs_digest": canonical_json_sha256("backend-inputs"),
            "meta_digest": canonical_json_sha256("backend-meta"),
        }

    def cargo(plan, **kwargs: Any) -> subprocess.CompletedProcess[str]:
        cmd = list(plan.command)
        cargo_calls.append(cmd)
        profile = cmd[cmd.index("--profile") + 1]
        output = (
            Path(plan.environment["CARGO_TARGET_DIR"])
            / _cargo_profile_dir(profile)
            / f"molt-backend{_EXE}"
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(f"backend build {len(cargo_calls)}".encode())
        output.chmod(0o755)
        return subprocess.CompletedProcess(
            cmd,
            0,
            compiler_cargo_stdout(plan.project_root, output, plan.environment),
            "",
        )

    def probe(cmd: list[str], **_kwargs: Any) -> subprocess.CompletedProcess[bytes]:
        probe_calls.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(cli_backend_binary, "backend_build_admission", admission)
    monkeypatch.setattr(compiler_identity, "backend_build_admission", admission)
    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", fingerprint)

    # The fixture has one crate; the live source-generation fence still
    # captures it and Cargo.lock before/after every modeled build, and the
    # build-input coverage check sees the main.rs its Cargo transcript reads.
    def source_paths(root, _features, *, product=HostToolProduct.BACKEND):
        assert product is HostToolProduct.BACKEND
        return [root / "runtime/molt-backend"]

    monkeypatch.setattr(cli_backend_binary, "_backend_source_paths", source_paths)
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", cargo)
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", probe
    )
    return cargo_calls, probe_calls


def _stub_backend_cache_setup(
    monkeypatch: pytest.MonkeyPatch, target: Target
) -> list[dict[str, Any]]:
    calls: list[dict[str, Any]] = []
    cache_setup = _BackendCacheSetup(
        backend_environment={},
        artifact_contract=resolve_backend_artifact_contract(
            target=target, emit_mode="bin"
        ),
        cache_enabled=False,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        cache_candidates=(),
        cache_hit=False,
        cache_hit_tier=None,
    )

    def prepare(**kwargs: Any) -> _BackendCacheSetup:
        # Cache keys are downstream of admission; record what they bind to.
        calls.append(kwargs)
        return cache_setup

    monkeypatch.setattr(
        cli_backend_cache_setup, "_prepare_backend_cache_setup", prepare
    )
    return calls


@pytest.mark.parametrize("environment", sorted(_ENVIRONMENTS))
@pytest.mark.parametrize("backend", ["auto", "cranelift", "llvm"])
@pytest.mark.parametrize("target", _TARGETS)
def test_prewarm_admits_the_backend_the_build_selects(
    target: str,
    backend: str,
    environment: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    env = _ENVIRONMENTS[environment]
    _isolated_molt_root(tmp_path, monkeypatch, **env)
    admissions = _record_backend_admission(monkeypatch)
    # The native runtime staticlib is its own admission, outside this contract.
    monkeypatch.setattr(
        cli_backend_compile,
        "_stage_runtime_callable_symbols_for_native_codegen",
        lambda *_args, **_kwargs: ("", None),
    )
    build_targets: list[Target] = []
    build_backends: list[str] = []

    def build_fn(request: BuildRequest) -> int:
        build_targets.append(request.target)
        build_backends.append(request.codegen_backend)
        return 0

    build_rc = _dispatch(
        ["build", "app.py", "--target", target, "--backend", backend, "--json"],
        build_fn=build_fn,
    )
    # The selection travels as a parameter; the process environment keeps
    # whatever the caller had.
    assert os.environ.get("MOLT_BACKEND") == env.get("MOLT_BACKEND")
    for build_target, build_backend in zip(build_targets, build_backends):
        _prepared, build_error = _build_backend_setup(
            build_target, tmp_path, build_backend
        )
        assert build_error is not None
    build_admissions = list(admissions)
    admissions.clear()
    # The build resolves its Rust CPU policy as a value (HF-60): the process
    # environment keeps the caller's Rust flags for the prewarm that follows.
    assert os.environ.get("RUSTFLAGS") == env.get("RUSTFLAGS")

    prewarm_argv = ["internal-backend-build", "--target", target, "--backend", backend]
    prewarm_rc = _dispatch([*prewarm_argv, "--json"])

    if target == "llvm" and backend == "cranelift":
        # Both commands reject the conflicting alias before selecting anything.
        assert build_rc == prewarm_rc == 2
        assert build_targets == []
        assert build_admissions == admissions == []
        return
    assert build_rc == 0
    assert prewarm_rc == 2
    assert len(build_admissions) == 1
    assert admissions == build_admissions
    # Only a native build compiles its Rust for the host CPU; `llvm` is the
    # native target with the LLVM backend.
    host_cpu = env.get("MOLT_PERF_PROFILE") == "native-arch" and target in {
        "native",
        "llvm",
    }
    assert admissions[0]["rust_cpu"] is (
        RustCpuPolicy.NATIVE if host_cpu else RustCpuPolicy.BASELINE
    )


@pytest.mark.parametrize(
    ("target", "binary_name", "features"),
    [
        ("wasm", "molt-backend.wasm_backend", ("wasm-backend",)),
        ("luau", "molt-backend.luau_backend", ("luau-backend",)),
        ("native", "molt-backend.native_backend", ("native-backend",)),
        ("llvm", "molt-backend.llvm_native_backend", ("native-backend", "llvm")),
    ],
)
def test_prewarm_selects_the_release_host_compiler_in_the_session_target(
    target: str,
    binary_name: str,
    features: tuple[str, ...],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = _isolated_molt_root(tmp_path, monkeypatch, MOLT_SESSION_ID="wasm-ci")
    admissions = _record_backend_admission(monkeypatch)

    assert _dispatch(["internal-backend-build", "--target", target, "--json"]) == 2

    # `--build-profile dev` guests still dispatch the release host compiler,
    # so a dev-fast bare Cargo prewarm can never satisfy them.
    session_release = root / "target" / "sessions" / "wasm-ci" / "release"
    assert admissions == [
        {
            "binary": session_release / f"{binary_name}{_EXE}",
            "product": HostToolProduct.BACKEND,
            "cargo_profile": "release",
            "features": features,
            "project_root": root,
            "cargo_timeout": None,
            "rust_cpu": RustCpuPolicy.BASELINE,
            "rustflags": None,
        }
    ]


def test_prewarm_defaults_to_the_configured_build_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _isolated_molt_root(tmp_path, monkeypatch)
    admissions = _record_backend_admission(monkeypatch)
    build_targets: list[Target] = []

    def build_fn(request: BuildRequest) -> int:
        build_targets.append(request.target)
        return 0

    build_cfg = {"target": "wasm"}
    build_rc = _dispatch(
        ["build", "app.py", "--json"], build_fn=build_fn, build_cfg=build_cfg
    )
    prewarm_rc = _dispatch(["internal-backend-build", "--json"], build_cfg=build_cfg)

    assert (build_rc, prewarm_rc) == (0, 2)
    assert build_targets == [TargetKind.WASM]
    [admission] = admissions
    assert admission["features"] == ("wasm-backend",)
    assert admission["binary"] == (
        root / "target" / "release" / f"molt-backend.wasm_backend{_EXE}"
    )


def test_prewarm_cargo_timeout_flag_overrides_environment(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _isolated_molt_root(tmp_path, monkeypatch, MOLT_CARGO_TIMEOUT="300")
    admissions = _record_backend_admission(monkeypatch)
    argv = ["internal-backend-build", "--target", "wasm", "--json"]

    assert _dispatch([*argv, "--cargo-timeout", "1200"]) == 2
    assert [admission["cargo_timeout"] for admission in admissions] == [1200.0]
    capsys.readouterr()

    assert _dispatch([*argv, "--cargo-timeout", "0"]) == 2
    assert len(admissions) == 1
    payload = json.loads(capsys.readouterr().out)
    assert payload["errors"] == ["--cargo-timeout must be greater than zero, not 0.0."]


@pytest.mark.parametrize("target", ["wasm64-unknown-unknown", "not-a-target"])
def test_prewarm_rejects_targets_the_build_layout_rejects(
    target: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = _isolated_molt_root(tmp_path, monkeypatch)
    admissions = _record_backend_admission(monkeypatch)
    with pytest.raises(ValueError):
        _build_layout(admit_target(target), tmp_path, root)

    assert _dispatch(["internal-backend-build", "--target", target, "--json"]) == 2

    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert payload["errors"] == [
        f"Native linking has no object-format policy for target {target!r}."
    ]
    assert admissions == []


def test_prewarm_rejects_mlir_which_never_admits_the_backend_compiler(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = _isolated_molt_root(tmp_path, monkeypatch)
    admissions = _record_backend_admission(monkeypatch)
    # The build routes MLIR layouts to molt-backend-mlir before backend setup.
    assert _build_layout(TargetKind.MLIR, tmp_path, root).is_mlir_emit

    assert _dispatch(["internal-backend-build", "--target", "mlir", "--json"]) == 2

    payload = json.loads(capsys.readouterr().out)
    assert "molt-backend-mlir" in payload["errors"][0]
    assert admissions == []


def test_prewarm_receipts_let_the_next_build_skip_cargo_until_removed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = _isolated_molt_root(tmp_path, monkeypatch, MOLT_SESSION_ID="wasm-ci")
    cargo_calls, probe_calls = _fake_backend_toolchain(monkeypatch)
    release_dir = root / "target" / "sessions" / "wasm-ci" / "release"
    coordinate = release_dir / f"molt-backend.luau_backend{_EXE}"

    assert _dispatch(["internal-backend-build", "--target", "luau", "--json"]) == 0

    payload = json.loads(capsys.readouterr().out)
    assert (payload["command"], payload["status"]) == ("internal-backend-build", "ok")
    data = payload["data"]
    compiler = data["compiler"]
    assert data["admission"] == "receipts"
    binary = selected_backend(root, coordinate, "release")
    assert compiler["coordinate"] == os.fspath(coordinate)
    assert Path(compiler["path"]).samefile(binary)
    assert compiler["sha256"] == hashlib.sha256(binary.read_bytes()).hexdigest()
    assert (compiler["cargo_profile"], compiler["features"]) == (
        "release",
        ["luau-backend"],
    )
    assert "backend_binary_cargo_build" in data["stage_timings_ms"]
    [cargo_cmd] = cargo_calls
    assert cargo_cmd[cargo_cmd.index("--profile") + 1] == "release"
    assert cargo_cmd[cargo_cmd.index("--features") + 1] == "luau-backend"
    receipts = data["receipts"]
    source_receipt = Path(receipts["source_content"]["path"])
    assert source_receipt.is_file()
    assert Path(receipts["feature_probe"]["path"]).is_file()
    assert receipts["feature_probe"]["probe_target"] == "luau"
    assert len(probe_calls) == 1

    # The next `--build-profile dev` build admits the prewarmed compiler from
    # those receipts: no Cargo, no re-probe, and its cache identity binds the
    # prewarmed bytes.
    cache_inputs = _stub_backend_cache_setup(monkeypatch, TargetKind.LUAU)
    prepared, error = _build_backend_setup(TargetKind.LUAU, tmp_path, "cranelift")
    assert error is None and prepared is not None
    assert prepared.backend_bin.samefile(binary)
    assert prepared.backend_compiler_fingerprint == compiler["fingerprint"]
    assert cache_inputs[-1]["backend_bin"].samefile(binary)
    assert cache_inputs[-1]["backend_compiler_fingerprint"] == compiler["fingerprint"]
    assert (len(cargo_calls), len(probe_calls)) == (1, 1)

    # The receipts, not a lucky cache, carry that admission: without them the
    # build must establish provenance with Cargo again.
    source_receipt.unlink()
    prepared, error = _build_backend_setup(TargetKind.LUAU, tmp_path, "cranelift")
    assert error is None and prepared is not None
    assert len(cargo_calls) == 2
    assert prepared.backend_bin.read_bytes() == b"backend build 2"
    # The generation the first build ran is unchanged beside the new one.
    assert binary.read_bytes() != b"backend build 2"


def test_prewarm_fails_closed_when_admission_publishes_no_receipt(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _isolated_molt_root(tmp_path, monkeypatch)
    cargo_calls, _probe_calls = _fake_backend_toolchain(monkeypatch)
    # A successful admission result cannot substitute for retained receipts.
    # Inject lost publication after valid source and executable admission.
    monkeypatch.setattr(
        cli_backend_binary, "atomic_write_bytes", lambda *_args, **_kwargs: None
    )

    rc = _dispatch(["internal-backend-build", "--target", "wasm", "--json"])

    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert rc == 2
    assert len(cargo_calls) == 1
    assert payload["status"] == "error"
    [message] = payload["errors"]
    assert "no store receipt selecting" in message
    assert message in captured.err


def test_prewarm_cargo_failure_keeps_json_framing_and_stderr_detail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(
            cmd, 101, "", "error: linking with `cc` failed"
        ),
    )

    rc = _dispatch(["internal-backend-build", "--target", "native", "--json"])

    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert rc == 2
    assert payload["status"] == "error"
    assert payload["data"]["failure"]["phase"] == "backend_cargo_build"
    assert payload["data"]["failure"]["returncode"] == 101
    [message] = payload["errors"]
    assert "linking with `cc` failed" in message
    assert message in captured.err


@pytest.mark.usefixtures("developer_host_context")
def test_backend_path_cache_tracks_session_changes(monkeypatch, tmp_path):
    from molt.cli import backend_execution

    monkeypatch.setattr(backend_execution, "installed_compiler", lambda root: None)
    sessions = tmp_path / "target" / "sessions"
    monkeypatch.setenv("MOLT_SESSION_ID", "first-session")
    first = backend_execution._backend_bin_path(tmp_path, "release")
    monkeypatch.setenv("MOLT_SESSION_ID", "second-session")
    second = backend_execution._backend_bin_path(tmp_path, "release")
    # A generated session never scopes the target (molt.dx).
    monkeypatch.setenv("MOLT_SESSION_ID_GENERATED", "1")
    generated = backend_execution._backend_bin_path(tmp_path, "release")
    assert first.parent.parent == sessions / "first-session"
    assert second.parent.parent == sessions / "second-session"
    assert generated.parent.parent == tmp_path / "target"


@pytest.mark.parametrize("profile", ["dev-fast", "release-fast", "release-output"])
@pytest.mark.parametrize("target", [None, "aarch64-unknown-linux-gnu", "wasm32-wasip1"])
@pytest.mark.parametrize("stdlib", ["micro", "full"])
@pytest.mark.usefixtures("developer_host_context")
def test_runtime_path_cache_tracks_session_changes(
    monkeypatch, tmp_path, profile, target, stdlib
):
    from molt.cli import runtime_paths

    sessions = tmp_path / "target" / "sessions"
    monkeypatch.setenv("MOLT_SESSION_ID", "first-session")
    first = runtime_paths._runtime_lib_path(tmp_path, profile, target, stdlib)
    monkeypatch.setenv("MOLT_SESSION_ID", "second-session")
    second = runtime_paths._runtime_lib_path(tmp_path, profile, target, stdlib)
    first.relative_to(sessions / "first-session")
    second.relative_to(sessions / "second-session")
    assert first.name == second.name
    assert first != second


def test_same_content_backend_metadata_refresh_preserves_admission(
    tmp_path, monkeypatch, capsys
):
    """New source timestamps refresh this checkout's memo, never the store.

    The store receipt names the product, so it stays byte-identical; the
    checkout's identity memo learns the new metadata, so the next build skips
    hashing the sources again.
    """
    root = _isolated_molt_root(tmp_path, monkeypatch)
    cargo_calls, probe_calls = _fake_backend_toolchain(monkeypatch)
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 0
    before = json.loads(capsys.readouterr().out)["data"]
    receipt = Path(before["receipts"]["source_content"]["path"])
    stored = receipt.read_bytes()
    assert "inputs_digest" not in json.loads(stored)
    memo = cli_backend_binary._backend_fingerprint_path(
        root, Path(before["compiler"]["coordinate"]), "release"
    )
    original = cli_backend_binary._backend_fingerprint
    touched_digest = canonical_json_sha256("new timestamps same content")

    def touched(*args, **kwargs):
        fingerprint = original(*args, **kwargs)
        fingerprint["inputs_digest"] = touched_digest
        return fingerprint

    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", touched)
    assert json.loads(memo.read_text(encoding="utf-8"))["inputs_digest"] != (
        touched_digest
    )
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 0
    after = json.loads(capsys.readouterr().out)["data"]
    assert before["compiler"]["fingerprint"] == after["compiler"]["fingerprint"]
    assert Path(after["receipts"]["source_content"]["path"]) == receipt
    assert receipt.read_bytes() == stored
    assert json.loads(memo.read_text(encoding="utf-8"))["inputs_digest"] == (
        touched_digest
    )
    assert len(cargo_calls) == 1
    assert len(probe_calls) == 1


def test_prewarm_rejects_compiler_bytes_changed_after_admission(
    tmp_path, monkeypatch, capsys
):
    _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    original = cli_backend_compile._ensure_selected_backend_binary

    def replaced(selection, **kwargs):
        result = original(selection, **kwargs)
        assert result.ok and result.binary is not None
        result.binary.write_bytes(b"different compiler after admission")
        return result

    monkeypatch.setattr(
        cli_backend_compile, "_ensure_selected_backend_binary", replaced
    )
    assert _dispatch(["internal-backend-build", "--target", "native", "--json"]) == 2
    # The admitted bytes live in the product store; changed bytes there are a
    # corrupt entry, which fails loud and names it.
    assert "is corrupt: its executable differs from its receipt" in (
        capsys.readouterr().err
    )


def test_receipt_verification_renews_the_entry_before_it_reads_it(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Every store read renews its entry first (design 56, 2.8).

    The entry was last used 30 s inside the idle window, where a publication
    elsewhere could evict it during the read; the receipt read renews it.
    """
    import time

    from molt.cli import backend_build, product_store
    from molt.cli.backend_compile import _BackendSelection
    from molt.toolchain_identity import executable_content_identity

    root = _isolated_molt_root(tmp_path, monkeypatch)
    _fake_backend_toolchain(monkeypatch)
    assert _dispatch(["internal-backend-build", "--target", "wasm", "--json"]) == 0
    compiler = json.loads(capsys.readouterr().out)["data"]["compiler"]
    binary = Path(compiler["path"])
    store = cli_backend_binary.host_tool_store(root)
    (entry,) = store.root.glob("[0-9a-f]" * 64)
    ledger_path = store.root / ".product-store.json"
    ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    aged = time.time() - product_store.PRODUCT_STORE_IDLE_SECONDS + 30
    ledger["last_used"][entry.name] = aged
    ledger_path.write_text(json.dumps(ledger), encoding="utf-8")

    selection = _BackendSelection(
        cargo_profile=compiler["cargo_profile"],
        features=tuple(compiler["features"]),
        binary=Path(compiler["coordinate"]),
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    receipts, error = backend_build._verified_backend_receipts(
        root,
        selection,
        binary,
        executable_content_identity(binary, label="prewarmed backend"),
        compiler["fingerprint"],
    )
    assert error is None and receipts is not None
    renewed = json.loads(ledger_path.read_bytes())["last_used"][entry.name]
    assert renewed > aged + 60
