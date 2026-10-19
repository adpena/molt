from __future__ import annotations

from molt.cli.runtime_identity_schema import runtime_build_fingerprint

from tests.runtime_build_identity_helper import (
    runtime_cargo_environment_inputs,
    runtime_cargo_stdout,
)

from tests.compiler_identity_helper import compiler_environment_inputs

from functools import partial
from tests.cli.backend_generation_test_support import publish_backend
from tests.compiler_identity_helper import stub_compiler_admission

import contextlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from tests.runtime_build_identity_helper import RuntimeFixtureRoot, runtime_cargo_plan
from typing import cast

import pytest

import molt.wasm_artifact as wasm_artifact
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import runtime_native_build as RUNTIME_NATIVE_BUILD
from molt.cli.models import _RuntimeArtifactState
from molt.cli.runtime_native_generation import publish_native_runtime_generation
from molt.cli import runtime_wasm_build as RUNTIME_WASM_BUILD
from molt.cli import runtime_wasm_build_support as RUNTIME_WASM_BUILD_SUPPORT
from molt.cli import runtime_wasm_build_spec as RUNTIME_WASM_BUILD_SPEC
from molt.cli.native_link_manifest import (
    native_link_flags_from_manifest,
    read_native_link_dependency_manifest,
)
from molt.cli.runtime_artifact_selection import RuntimeCrateType
from molt.cli.static_archive_identity import artifact_content_identity
from molt.cli.rust_cpu_policy import RustCpuPolicy
from tests.cli.native_link_test_support import static_archive_bytes
from tests.runtime_build_identity_helper import (
    native_runtime_staticlib_identity,
)
from molt.cli import artifact_state as cli_artifact_state
from molt.cli import runtime_paths as cli_runtime_paths
from molt.cli import runtime_fingerprints as cli_runtime_fingerprints
from molt.wasi_sdk_identity import wasi_c_abi_projection

_FAKE_STATICLIB = static_archive_bytes(b"fake-staticlib")

# Fixture metadata digest used by runtime fingerprint hydration tests.
_TEST_RUNTIME_META_DIGEST = "ab" * 32
_TEST_RUNTIME_HASH_DIGEST = "cd" * 32
_TEST_RUNTIME_INPUTS_DIGEST = "ef" * 32


# Every Cargo run here is a fake; host free space must not gate it.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


@pytest.fixture(autouse=True)
def _native_cargo_plan_authority(
    runtime_fixture_root: RuntimeFixtureRoot, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )


def _valid_wasm_bytes(label: bytes = b"") -> bytes:
    """Structurally valid wasm module bytes, distinguishable via ``label``.

    Hydration candidacy runs the real ``_artifact_content_looks_valid``
    check, which rejects magic-plus-garbage fixtures.
    """
    if not label:
        return wasm_artifact._build_wasm_sections([])
    payload = wasm_artifact._write_wasm_string("molt.test") + label
    return wasm_artifact._build_wasm_sections([(0, payload)])


def _cargo_runtime_artifact_stdout(path: Path) -> bytes:
    return (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": "path+file:///repo/runtime/molt-runtime#0.0.1",
                "target": {"name": "molt_runtime"},
                "filenames": [str(path)],
                "fresh": True,
            }
        )
        + "\n"
    ).encode("utf-8")


def _cargo_wasi_declared_inputs_stdout(
    stdout: str,
    *,
    root: Path,
    output: Path,
    environment,
    undeclared_sdk_sibling: bool = False,
) -> str:
    """Represent the Rust C-ABI declarations in Cargo build-script metadata."""
    wasi = wasi_c_abi_projection(environment)
    assert wasi is not None
    # The SDK must be external to the source root: otherwise even an omitted
    # declaration is correctly admitted as a repository source by the reader.
    assert not wasi.sdk.resolve().is_relative_to(root.resolve())
    assert not root.resolve().is_relative_to(wasi.sdk.resolve())
    # runtime/build_support/wasi_sysroot.rs declares all five members and the
    # include directory. This fixture does not ask the publication helper which
    # paths it intends to admit: omitting any declaration must fail the reader.
    paths = [path for _role, path, _size, _digest in wasi.files]
    assert len(paths) == 5
    paths.append(wasi.include)
    if undeclared_sdk_sibling:
        sibling = wasi.sdk / "VERSION"
        assert sibling.is_file()
        paths.append(sibling)
    out_dir = output.parent / "build-input-fixture" / "out"
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir.parent / "output").write_text(
        "".join(f"cargo:rerun-if-changed={path}\n" for path in paths),
        encoding="utf-8",
    )
    messages = [json.loads(line) for line in stdout.split("\n") if line]
    messages.insert(
        -1,
        {
            "reason": "build-script-executed",
            "package_id": messages[0]["package_id"],
            "out_dir": os.fspath(out_dir),
            "env": [],
        },
    )
    return "".join(json.dumps(message) + "\n" for message in messages)


def _cargo_cpython_abi_artifact_stdout(
    path: Path, *, root: Path, environment, undeclared_sdk_sibling: bool = False
) -> bytes:
    stdout = runtime_cargo_stdout(
        root, (path,), environment, package="molt-lang-cpython-abi"
    )
    return _cargo_wasi_declared_inputs_stdout(
        stdout,
        root=root,
        output=path,
        environment=environment,
        undeclared_sdk_sibling=undeclared_sdk_sibling,
    ).encode("utf-8")


def test_runtime_wasm_cargo_build_preserves_stale_candidates_and_uses_reported_artifact(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli_runtime_paths._cargo_profile_dir("dev-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_artifact_path(
        target_root, profile_dir
    )
    deps_primary = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime.wasm"
    )
    stale_hashed = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime-deadbeef.wasm"
    )
    reported = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime-feedface.wasm"
    )
    for path, payload in (
        (primary, b"old-primary"),
        (deps_primary, b"old-deps"),
        (stale_hashed, b"old-hashed"),
        (reported, b"new-reported"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    seen: dict[str, object] = {}

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        seen["cmd"] = cmd
        seen["env"] = kwargs["env"]
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(reported),
            b"",
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    build, src = RUNTIME_WASM_BUILD_SUPPORT._run_runtime_wasm_cargo_build(
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={"CARGO_TARGET_DIR": str(target_root)},
            requested_target="wasm32-wasip1",
            cargo_command=RUNTIME_WASM_BUILD_SUPPORT._cargo_cmd_with_json_artifact_messages(
                [
                    "cargo",
                    "rustc",
                    "--package",
                    "molt-runtime",
                    "--profile",
                    "dev-fast",
                    "--target",
                    "wasm32-wasip1",
                    "--lib",
                    "--crate-type",
                    "cdylib",
                ]
            ),
        ),
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind=RuntimeCrateType.CDYLIB,
    )

    assert build.returncode == 0
    assert src == reported
    assert primary.read_bytes() == b"old-primary"
    assert deps_primary.read_bytes() == b"old-deps"
    assert stale_hashed.read_bytes() == b"old-hashed"
    assert "--message-format=json-render-diagnostics" in cast(list[str], seen["cmd"])
    assert cast(dict[str, str], seen["env"])["CARGO_TARGET_DIR"] == str(target_root)


def test_runtime_wasm_cargo_build_does_not_fallback_to_old_artifact_without_report(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli_runtime_paths._cargo_profile_dir("dev-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_artifact_path(
        target_root, profile_dir
    )
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"old-valid")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            b'{"reason":"build-finished","success":true}\n',
            b"",
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    build, src = RUNTIME_WASM_BUILD_SUPPORT._run_runtime_wasm_cargo_build(
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={"CARGO_TARGET_DIR": str(target_root)},
            requested_target="wasm32-wasip1",
            cargo_command=RUNTIME_WASM_BUILD_SUPPORT._cargo_cmd_with_json_artifact_messages(
                [
                    "cargo",
                    "rustc",
                    "--package",
                    "molt-runtime",
                    "--profile",
                    "dev-fast",
                    "--target",
                    "wasm32-wasip1",
                    "--lib",
                    "--crate-type",
                    "cdylib",
                ]
            ),
        ),
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind=RuntimeCrateType.CDYLIB,
    )

    assert build.returncode == 0
    assert src != primary
    assert src.name == ".molt_runtime.cargo-report-missing.wasm"
    assert not src.exists()
    assert primary.read_bytes() == b"old-valid"


def test_runtime_wasm_cargo_build_accepts_cargo_fresh_primary_artifact(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli_runtime_paths._cargo_profile_dir("dev-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_artifact_path(
        target_root, profile_dir
    )
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"fresh-primary")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(primary),
            b"",
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    _build, src = RUNTIME_WASM_BUILD_SUPPORT._run_runtime_wasm_cargo_build(
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={"CARGO_TARGET_DIR": str(target_root)},
            requested_target="wasm32-wasip1",
            cargo_command=RUNTIME_WASM_BUILD_SUPPORT._cargo_cmd_with_json_artifact_messages(
                [
                    "cargo",
                    "rustc",
                    "--package",
                    "molt-runtime",
                    "--profile",
                    "dev-fast",
                    "--target",
                    "wasm32-wasip1",
                    "--lib",
                    "--crate-type",
                    "cdylib",
                ]
            ),
        ),
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind=RuntimeCrateType.CDYLIB,
    )

    assert src == primary
    assert primary.read_bytes() == b"fresh-primary"


def test_runtime_wasm_cargo_build_preserves_staticlibs_and_uses_reported_staticlib(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli_runtime_paths._cargo_profile_dir("release-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_staticlib_path(
        target_root, profile_dir
    )
    reported = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "libmolt_runtime-feedface.a"
    )
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"old-staticlib")
    reported.parent.mkdir(parents=True, exist_ok=True)
    reported.write_bytes(b"new-staticlib")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(reported),
            b"",
        )

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    _build, src = RUNTIME_WASM_BUILD_SUPPORT._run_runtime_wasm_cargo_build(
        cargo_plan=runtime_cargo_plan(
            tmp_path,
            fixture_root=runtime_fixture_root,
            env={"CARGO_TARGET_DIR": str(target_root)},
            requested_target="wasm32-wasip1",
            cargo_command=RUNTIME_WASM_BUILD_SUPPORT._cargo_cmd_with_json_artifact_messages(
                [
                    "cargo",
                    "rustc",
                    "--package",
                    "molt-runtime",
                    "--profile",
                    "release-fast",
                    "--target",
                    "wasm32-wasip1",
                    "--lib",
                    "--crate-type",
                    "staticlib",
                ]
            ),
        ),
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind=RuntimeCrateType.STATICLIB,
    )

    assert src == reported
    assert primary.read_bytes() == b"old-staticlib"
    assert reported.read_bytes() == b"new-staticlib"


@pytest.mark.parametrize(
    "report_artifact,undeclared_sdk_sibling",
    [(True, False), (False, False), (True, True)],
)
def test_cpython_abi_build_requires_and_fingerprints_only_reported_staticlib(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    report_artifact: bool,
    undeclared_sdk_sibling: bool,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    source = root / "runtime/molt-cpython-abi/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn provider() {}\n", encoding="utf-8")
    target_root = root / "target"
    profile_dir = cli_runtime_paths._cargo_profile_dir("dev-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_cpython_abi_staticlib_path(
        target_root, profile_dir
    )
    reported = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "libmolt_cpython_abi-feedface.a"
    )
    for path, payload in (
        (primary, b"stale-primary"),
        (reported, _FAKE_STATICLIB),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
    products_before = {
        path: (path.read_bytes(), path.stat().st_mtime_ns)
        for path in (primary, reported)
    }

    cargo_calls = []
    monkeypatch.setenv("MOLT_TEST_RUNTIME_INPUT", "first")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        cargo_calls.append(list(cmd))
        stdout = (
            _cargo_cpython_abi_artifact_stdout(
                reported,
                root=root,
                environment=kwargs["env"],
                undeclared_sdk_sibling=undeclared_sdk_sibling,
            )
            if report_artifact
            else b'{"reason":"build-finished","success":true}\n'
        )
        return subprocess.CompletedProcess(cmd, 0, stdout, b"")

    identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", target_triple="wasm32-wasip1"
    )
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    state_root = RUNTIME_WASM_BUILD_SUPPORT._build_state_root(root)
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "resolve_wasm_cpython_abi_build_identity",
        lambda *a, **k: identity,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_cargo_build_env",
        lambda: {
            "CARGO_TARGET_DIR": str(target_root),
            "MOLT_BUILD_PYTHON": sys.executable,
            "MOLT_WASI_SYSROOT": str(root / "sysroot"),
            "MOLT_TEST_RUNTIME_INPUT": os.environ["MOLT_TEST_RUNTIME_INPUT"],
        },
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    runtime_state = _RuntimeArtifactState()
    provider = RUNTIME_WASM_BUILD_SUPPORT._ensure_wasm_cpython_abi_staticlib(
        project_root=root,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        runtime_state=runtime_state,
    )
    published = report_artifact and not undeclared_sdk_sibling
    assert provider == (reported if published else None)
    if undeclared_sdk_sibling:
        failure = runtime_state.runtime_wasm_build_failure
        assert failure is not None
        assert failure.stage == "cpython-abi-cargo-input-observation"
        assert "outside" in failure.summary and "VERSION" in failure.summary
    assert primary.read_bytes() == b"stale-primary"
    reported_fp = cli_artifact_state._runtime_target_fingerprint_path(
        state_root,
        reported,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1.cpython-abi",
    )
    primary_fp = cli_artifact_state._runtime_target_fingerprint_path(
        state_root,
        primary,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1.cpython-abi",
    )
    assert reported_fp.exists() is published
    assert primary_fp.exists() is published
    assert {
        path: (path.read_bytes(), path.stat().st_mtime_ns)
        for path in (primary, reported)
    } == products_before
    if published:
        # The canonical requested-output sidecar may be named for the primary,
        # but its artifact identity must attest the exact Cargo-reported path.
        stored = cli_runtime_fingerprints._read_runtime_fingerprint(primary_fp)
        assert stored["build_identity"] == identity.to_dict()
        assert stored["artifact_content_identity"] == artifact_content_identity(
            reported
        )

        assert (
            stored["cargo_environment"] == runtime_cargo_environment_inputs().to_dict()
        )
        before = (reported_fp.read_bytes(), reported_fp.stat().st_mtime_ns)
        monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
        assert (
            RUNTIME_WASM_BUILD_SUPPORT._ensure_wasm_cpython_abi_staticlib(
                project_root=root,
                json_output=True,
                cargo_profile="dev-fast",
                cargo_timeout=1.0,
            )
            == reported
        )
        assert len(cargo_calls) == 1
        assert (reported_fp.read_bytes(), reported_fp.stat().st_mtime_ns) == before
        monkeypatch.setenv("MOLT_TEST_RUNTIME_INPUT", "second")
        assert (
            RUNTIME_WASM_BUILD_SUPPORT._ensure_wasm_cpython_abi_staticlib(
                project_root=root,
                json_output=True,
                cargo_profile="dev-fast",
                cargo_timeout=1.0,
            )
            is None
        )
        assert len(cargo_calls) == 1
        assert (reported_fp.read_bytes(), reported_fp.stat().st_mtime_ns) == before


@pytest.mark.parametrize("with_state", [True, False])
@pytest.mark.parametrize(
    "failure, expected_stage",
    [
        ("missing_sdk", "cargo-plan"),
        ("cargo_plan", "cargo-plan"),
        ("pre_identity", "pre-build-identity"),
        ("metadata_read", "metadata-admission"),
        ("target_identity", "target-admission"),
        ("artifact_identity", "artifact-admission"),
        ("metadata_refresh", "metadata-refresh"),
        ("rebuild_disabled", "rebuild-policy"),
        ("execute_os", "cargo-execution"),
        ("execute_drift", "cargo-execution"),
        ("execute_timeout", "cargo-execution"),
        ("build_failure", "cargo-execution"),
        ("missing_artifact", "cargo-artifact"),
        ("post_identity", "post-build-identity"),
        ("input_plan_drift", "cargo-input-observation"),
        ("metadata_publish", "metadata-publication"),
    ],
)
def test_cpython_abi_failures_publish_consumable_evidence_in_json_mode(
    runtime_fixture_root: RuntimeFixtureRoot,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure: str,
    expected_stage: str,
    with_state: bool,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    from molt.cli import runtime_wasm_failure
    from molt.cli.cargo_execution import CargoExecutionResult, CargoPlanExecutionError
    from molt.cli.models import _RuntimeArtifactState

    support = RUNTIME_WASM_BUILD_SUPPORT
    source = root / "runtime/molt-cpython-abi/src/lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn provider() {}\n", encoding="utf-8")
    target_root = root / "target"
    evidence_root = root / "state"
    provider = target_root / "wasm32-wasip1" / "dev-fast" / "libmolt_cpython_abi.a"
    provider.parent.mkdir(parents=True)
    provider.write_bytes(_FAKE_STATICLIB)
    state = _RuntimeArtifactState() if with_state else None
    identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", target_triple="wasm32-wasip1"
    )
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    monkeypatch.setattr(
        runtime_wasm_failure, "_build_state_root", lambda _root: evidence_root
    )
    monkeypatch.setattr(support, "_cargo_target_root", lambda _root: target_root)
    monkeypatch.setattr(support, "_build_state_root", lambda _root: evidence_root)
    monkeypatch.setattr(
        support, "_build_lock", lambda *_a, **_kw: contextlib.nullcontext()
    )
    monkeypatch.setattr(support, "_build_slot", lambda: contextlib.nullcontext())
    # The plan's environment selects the runtime build Python (c9b4c0aa2).
    monkeypatch.setattr(
        support,
        "_cargo_build_env",
        lambda: (
            {"MOLT_BUILD_PYTHON": sys.executable}
            if failure == "missing_sdk"
            else {
                "MOLT_BUILD_PYTHON": sys.executable,
                "MOLT_WASI_SYSROOT": str(root / "sysroot"),
            }
        ),
    )
    monkeypatch.setattr(
        support,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )
    monkeypatch.setattr(support, "_read_runtime_fingerprint", lambda _path: None)
    monkeypatch.setattr(
        support, "_current_runtime_target_artifact", lambda *_a, **_k: None
    )
    monkeypatch.setattr(
        support, "_runtime_artifact_fingerprint_matches", lambda *_a, **_k: False
    )
    monkeypatch.setattr(
        support, "_runtime_fingerprint_metadata_needs_refresh", lambda *_a: False
    )
    identity_calls = 0

    def resolve_identity(*_args, **_kwargs):
        nonlocal identity_calls
        identity_calls += 1
        if failure == "pre_identity" or (
            failure in {"target_identity", "artifact_identity", "post_identity"}
            and identity_calls > 1
        ):
            raise ValueError("injected identity rejection")
        return identity

    monkeypatch.setattr(
        support, "resolve_wasm_cpython_abi_build_identity", resolve_identity
    )

    def reject(*_args, **_kwargs):
        raise OSError("injected filesystem rejection")

    if failure == "missing_sdk":
        from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan

        def missing_sdk_plan(root, *, env, **kwargs):
            # Exercise the actual plan admission; no Cargo/rustc execution occurs.
            return resolve_runtime_cargo_plan(
                root,
                env={
                    **env,
                    "RUSTC": sys.executable,
                    "CARGO_HOME": str(root / "empty-cargo-home"),
                    "WASI_SDK_PATH": str(root / "absent-sdk"),
                },
                host_target="x86_64-unknown-linux-gnu",
                **kwargs,
            )

        monkeypatch.setattr(support, "resolve_runtime_cargo_plan", missing_sdk_plan)
    if failure == "cargo_plan":
        monkeypatch.setattr(support, "resolve_runtime_cargo_plan", reject)
    if failure == "metadata_read":
        monkeypatch.setattr(support, "_read_runtime_fingerprint", reject)
    if failure == "target_identity":
        monkeypatch.setattr(
            support,
            "_current_runtime_target_artifact",
            lambda *_a, **_k: (
                provider,
                provider.with_suffix(".fingerprint"),
                runtime_cargo_environment_inputs(),
            ),
        )
    if failure in {"artifact_identity", "metadata_refresh"}:
        admitted = {
            "version": 4,
            **runtime_build_fingerprint(identity),
            "cargo_environment": runtime_cargo_environment_inputs().to_dict(),
        }
        monkeypatch.setattr(
            support, "_read_runtime_fingerprint", lambda _path: admitted
        )
        monkeypatch.setattr(
            support, "_runtime_artifact_fingerprint_matches", lambda *_a, **_k: True
        )
    if failure == "metadata_refresh":
        monkeypatch.setattr(
            support, "_runtime_fingerprint_metadata_needs_refresh", lambda *_a: True
        )
        monkeypatch.setattr(support, "_refresh_runtime_fingerprint_metadata", reject)
    if failure == "rebuild_disabled":
        monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    if failure == "metadata_publish":
        monkeypatch.setattr(support, "_write_runtime_fingerprint", reject)
    commands: list[list[str]] = []

    def execute(plan, **_kwargs):
        commands.append(list(plan.command))
        if failure == "execute_os":
            raise OSError("injected process launch rejection")
        if failure == "execute_timeout":
            raise subprocess.TimeoutExpired(
                plan.command, 1.0, output=b"timeout stdout", stderr=b"timeout stderr"
            )
        stdout = (
            "unreported stdout"
            if failure == "missing_artifact"
            else _cargo_cpython_abi_artifact_stdout(
                provider, root=root, environment=plan.environment
            ).decode("utf-8")
        )
        result = CargoExecutionResult(
            subprocess.CompletedProcess(
                plan.command,
                7 if failure == "build_failure" else 0,
                stdout,
                "retained cargo stderr",
            ),
            attempts=(),
            retry_reason=None,
        )
        if failure == "input_plan_drift":
            config = root / ".cargo" / "config.toml"
            config.parent.mkdir(exist_ok=True)
            config.write_text("[build]\njobs = 2\n", encoding="utf-8")
        if failure == "execute_drift":
            raise CargoPlanExecutionError("injected configuration drift", result)
        return result

    monkeypatch.setattr(support, "_run_resolved_cargo_plan", execute)
    assert (
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=root,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1.0,
            runtime_state=state,
        )
        is None
    )
    if failure == "missing_sdk":
        assert commands == [], "missing SDK admission must precede Cargo execution"
    paths = list((evidence_root / "build_failures").glob("runtime-wasm-*.json"))
    assert len(paths) == 1
    evidence = json.loads(paths[0].read_text(encoding="utf-8"))
    assert evidence["stage"] == f"cpython-abi-{expected_stage}"
    assert evidence["cwd"] == str(root)
    if state is not None:
        assert state.runtime_wasm_build_failure is not None
        assert state.runtime_wasm_build_failure.evidence_path == paths[0]
    if commands:
        assert evidence["command"] == commands[0]
    if failure == "execute_timeout":
        assert evidence["timed_out"] is True
        assert evidence["stdout"] == "timeout stdout"
        assert evidence["stderr"] == "timeout stderr"
    if failure in {
        "execute_drift",
        "build_failure",
        "missing_artifact",
        "post_identity",
        "metadata_publish",
    }:
        assert evidence["stderr"] == "retained cargo stderr"
        assert evidence["stdout"]
        assert (
            evidence["details"]["cargo_execution"]["schema"]
            == "molt.cargo-execution.v2"
        )
        assert evidence["returncode"] == (7 if failure == "build_failure" else 0)
    output = capsys.readouterr()
    assert output.out == ""
    assert str(paths[0]) in output.err


def test_ensure_backend_binary_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    stub_compiler_admission(monkeypatch)
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_backend = canonical_target / "dev-fast" / "molt-backend.native_backend"
    isolated_backend = isolated_target / "dev-fast" / "molt-backend.native_backend"
    image = (
        "#!/bin/sh\n"
        'out=""\n'
        "while [ $# -gt 0 ]; do\n"
        '  if [ "$1" = "--output" ]; then\n'
        "    shift\n"
        '    out="$1"\n'
        "  fi\n"
        "  shift\n"
        "done\n"
        "printf 'ok' > \"$out\"\n"
    ).encode()

    fingerprint = {
        "hash": _TEST_RUNTIME_HASH_DIGEST,
        "rustc": "rustc",
        "inputs_digest": _TEST_RUNTIME_INPUTS_DIGEST,
        "meta_digest": _TEST_RUNTIME_META_DIGEST,
    }
    canonical_fp = cli_artifact_state._artifact_state_path_for_build_state_root(
        cli_artifact_state._canonical_build_state_root(project_root),
        canonical_backend,
        subdir="backend_fingerprints",
        stem_suffix="dev-fast",
        extension="fingerprint",
    )
    canonical_generation = publish_backend(
        canonical_backend,
        image,
        receipt=canonical_fp,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    result = cli_backend_binary._ensure_backend_binary(
        isolated_backend,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    assert result, result.message
    assert result.binary is not None
    # The isolated target publishes its own generation of the canonical bytes.
    assert result.binary.is_relative_to(isolated_target.resolve())
    assert result.binary.read_bytes() == canonical_generation.read_bytes() == image
    assert os.access(result.binary, os.X_OK)
    assert not isolated_backend.exists()


def test_canonical_native_generation_keeps_portable_dependency_custody(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    target_triple = "x86_64-unknown-linux-gnu"
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_runtime = (
        canonical_target / target_triple / "dev-fast" / "libmolt_runtime.a"
    )
    isolated_runtime = (
        isolated_target / target_triple / "dev-fast" / "libmolt_runtime.a"
    )
    canonical_runtime.parent.mkdir(parents=True)
    canonical_runtime.write_bytes(_FAKE_STATICLIB)
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=target_triple,
        family_seed="portable-native-hydration",
    )
    producer = tmp_path / "producer"
    out_dir = producer / "out"
    library_dir = producer / "lib"
    out_dir.mkdir(parents=True)
    library_dir.mkdir()
    (library_dir / "libportable.a").write_bytes(b"portable dependency")
    cargo_stdout = json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": "registry+https://example.invalid#portable-sys@1.0.0",
            "linked_libs": ["static=portable"],
            "linked_paths": [f"native={library_dir}"],
            "cfgs": [],
            "env": [],
            "out_dir": str(out_dir),
        }
    )
    generation = publish_native_runtime_generation(
        canonical_runtime,
        source_archive=canonical_runtime,
        cargo_stdout=cargo_stdout,
        cargo_stderr="note: native-static-libs: -lportable\n",
        cargo_profile="dev-fast",
        target_triple=target_triple,
        build_identity=build_identity,
        inputs_are_current=lambda: True,
        cargo_environment=runtime_cargo_environment_inputs(),
    )
    assert generation is not None
    shutil.rmtree(producer)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "_runtime_build_identity_for_plan",
        lambda *args, **kwargs: build_identity,
    )
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "_run_resolved_cargo_plan",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    state = _RuntimeArtifactState(runtime_lib=isolated_runtime)
    assert RUNTIME_NATIVE_BUILD._ensure_runtime_lib(
        isolated_runtime,
        target_triple,
        True,
        "dev-fast",
        project_root,
        1.0,
        runtime_state=state,
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    assert state.runtime_lib == generation.runtime_lib
    assert not isolated_runtime.exists()
    manifest = read_native_link_dependency_manifest(
        state.runtime_lib,
        target_triple=target_triple,
        cargo_profile="dev-fast",
        runtime_build_identity=build_identity,
    )
    flags = native_link_flags_from_manifest(
        manifest,
        object_format="elf",
        runtime_lib=state.runtime_lib,
    )
    assert flags[-1] == "-lportable"
    custody_directory = Path(flags[-2][2:])
    assert custody_directory != library_dir
    assert (custody_directory / "libportable.a").read_bytes() == (
        b"portable dependency"
    )


@pytest.mark.parametrize("reloc", (False, True), ids=("shared", "reloc"))
@pytest.mark.parametrize(
    "placement",
    (
        "primary",
        "deps",
        "hashed",
        "reported",
        "reported-undeclared-sdk",
        "reported-plan-drift",
    ),
)
def test_runtime_member_hydration_selects_attested_target_and_replays_without_cargo(
    runtime_fixture_root: RuntimeFixtureRoot,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    reloc: bool,
    placement: str,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    from molt.cli import runtime_wasm_pair_build as pair_build
    from molt.cli.models import _RuntimeArtifactState
    from tests.runtime_build_identity_helper import (
        bind_runtime_wasm_specs,
        write_runtime_crate,
    )

    write_runtime_crate(root)
    target = root / "target"
    state = target / ".molt_state"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_cargo_build_env",
        lambda: {"CARGO_TARGET_DIR": str(target), "MOLT_BUILD_PYTHON": sys.executable},
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "resolve_runtime_cargo_plan",
        partial(runtime_cargo_plan, fixture_root=runtime_fixture_root),
    )
    monkeypatch.setattr(RUNTIME_WASM_BUILD, "_build_state_root", lambda _root: state)
    monkeypatch.setattr(pair_build, "_build_state_root", lambda _root: state)
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
    monkeypatch.setattr(
        pair_build, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_runtime_missing_exports_for_mode", lambda *_a, **_k: set()
    )
    common = dict(
        cargo_profile="dev-fast",
        simd_enabled=True,
        freestanding=False,
        stdlib_profile="micro",
        resolved_modules=None,
        required_exports=None,
        required_link_features=frozenset(),
    )
    shared = RUNTIME_WASM_BUILD_SPEC._compute_runtime_wasm_build_spec(
        root, root / "out/molt_runtime.wasm", reloc=False, **common
    )
    relative = RUNTIME_WASM_BUILD_SPEC._compute_runtime_wasm_build_spec(
        root, root / "out/molt_runtime_reloc.wasm", reloc=True, **common
    )
    shared, relative = bind_runtime_wasm_specs(shared, relative, root=root)
    spec = relative if reloc else shared
    primary_shared = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_artifact_path(
        target, spec.profile_dir
    )
    primary_reloc = RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_staticlib_path(
        target, spec.profile_dir
    )
    primary = primary_reloc if reloc else primary_shared
    selected = primary
    if placement != "primary":
        selected = primary.parent / "deps" / primary.name
    if placement == "hashed" or placement.startswith("reported"):
        selected = selected.with_name(
            "libmolt_runtime-feedface.a" if reloc else "molt_runtime-feedface.wasm"
        )
    primary.parent.mkdir(parents=True, exist_ok=True)
    selected.parent.mkdir(parents=True, exist_ok=True)
    old_bytes = static_archive_bytes(b"stale") if reloc else _valid_wasm_bytes(b"stale")
    new_bytes = (
        static_archive_bytes(b"selected") if reloc else _valid_wasm_bytes(b"selected")
    )
    primary.write_bytes(old_bytes)
    selected.write_bytes(new_bytes)
    fingerprint = spec.staticlib_fingerprint if reloc else spec.fingerprint
    assert fingerprint is not None
    selected_fp = cli_artifact_state._runtime_target_fingerprint_path(
        state, selected, cargo_profile=spec.cargo_profile, target_label="wasm32-wasip1"
    )
    if placement.startswith("reported"):
        other = primary_shared if reloc else primary_reloc
        other.write_bytes(_valid_wasm_bytes() if reloc else _FAKE_STATICLIB)
        # The combined producer, not mtime/candidate ordering, chooses both files.
        assert shared.cargo_plan is not None
        stdout = runtime_cargo_stdout(
            root, (selected, other), shared.cargo_plan.environment
        )
        stdout = _cargo_wasi_declared_inputs_stdout(
            stdout,
            root=root,
            output=selected,
            environment=shared.cargo_plan.environment,
            undeclared_sdk_sibling=placement == "reported-undeclared-sdk",
        )
        ctx = pair_build._CombinedRuntimeWasmBuild(
            _RuntimeArtifactState(), shared, relative, True, 1.0, root, True, False
        )
        if placement == "reported-plan-drift":
            observe = pair_build.cargo_build_reads

            def observe_then_change_configuration(*args, **kwargs):
                result = observe(*args, **kwargs)
                config = root / ".cargo" / "config.toml"
                config.parent.mkdir(exist_ok=True)
                config.write_text("[build]\njobs = 2\n", encoding="utf-8")
                return result

            monkeypatch.setattr(
                pair_build, "cargo_build_reads", observe_then_change_configuration
            )
        published = pair_build._publish_combined_runtime_wasm_target(
            ctx,
            subprocess.CompletedProcess(["cargo"], 0, stdout, ""),
            other if reloc else selected,
        )
        if placement != "reported":
            assert published is None
            assert ctx.runtime_state.runtime_wasm_build_failure is not None
            assert (
                ctx.runtime_state.runtime_wasm_build_failure.stage
                == "combined-target-fingerprint-publication"
            )
            if placement == "reported-undeclared-sdk":
                summary = ctx.runtime_state.runtime_wasm_build_failure.summary
                assert "outside" in summary and "VERSION" in summary
            assert not selected_fp.exists()
            assert not cli_artifact_state._runtime_target_fingerprint_path(
                state,
                other,
                cargo_profile=spec.cargo_profile,
                target_label="wasm32-wasip1",
            ).exists()
            assert selected.read_bytes() == new_bytes
            assert primary.read_bytes() == old_bytes
            return
        assert published is not None
    else:
        selected_fp.parent.mkdir(parents=True, exist_ok=True)
        cli_runtime_fingerprints._write_runtime_fingerprint(
            selected_fp,
            fingerprint,
            artifact=selected,
            cargo_environment=runtime_cargo_environment_inputs(
                spec.cargo_plan.environment
            ),
        )
    linked: list[Path] = []

    def relink(*, staticlib_path: Path, output_path: Path, **_kwargs: object) -> bool:
        linked.append(staticlib_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(_valid_wasm_bytes(b"relinked"))
        return True

    monkeypatch.setattr(
        RUNTIME_WASM_BUILD, "_link_runtime_staticlib_to_reloc_wasm", relink
    )

    def forbidden_cargo(**_kwargs: object):
        raise AssertionError("attested target hydration must not invoke Cargo")

    monkeypatch.setattr(pair_build, "_run_runtime_wasm_cargo_build", forbidden_cargo)
    destination = (
        root
        / "hydrated"
        / ("molt_runtime_reloc.wasm" if reloc else "molt_runtime.wasm")
    )
    for _ in range(2):
        assert RUNTIME_WASM_BUILD._materialize_runtime_wasm_member_from_target(
            destination,
            reloc=reloc,
            json_output=True,
            cargo_timeout=1.0,
            project_root=root,
            required_exports=None,
            resolved_modules=None,
            spec=spec,
            cargo_environment=runtime_cargo_environment_inputs(
                spec.cargo_plan.environment
            ),
        )
        assert wasm_artifact.inspect_wasm_binary(destination) == "valid"
        destination.unlink()
    assert linked == ([selected, selected] if reloc else [])
    assert selected.read_bytes() == new_bytes
    if selected != primary:
        assert primary.read_bytes() == old_bytes
    assert cli_runtime_fingerprints._read_runtime_fingerprint(selected_fp)[
        "artifact_content_identity"
    ] == artifact_content_identity(selected)
