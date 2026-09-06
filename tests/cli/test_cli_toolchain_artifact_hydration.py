from __future__ import annotations

import contextlib
import json
import os
import shutil
import subprocess
from pathlib import Path
from molt.cli.runtime_cargo_plan import RuntimeCargoPlan
from tests.runtime_build_identity_helper import runtime_cargo_plan
from typing import cast

import pytest

import molt.wasm_artifact as wasm_artifact
from molt import cli
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import runtime_native_build as RUNTIME_NATIVE_BUILD
from molt.cli import runtime_paths as RUNTIME_PATHS
from molt.cli import runtime_wasm_build as RUNTIME_WASM_BUILD
from molt.cli import runtime_wasm_build_support as RUNTIME_WASM_BUILD_SUPPORT
from molt.cli import runtime_wasm_build_spec as RUNTIME_WASM_BUILD_SPEC
from molt.cli.native_link_manifest import (
    native_link_flags_from_manifest,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_artifact_selection import RuntimeCrateType
from molt.cli.runtime_build_identity import runtime_build_fingerprint
from molt.cli.static_archive_identity import artifact_content_identity
from tests.cli.native_link_test_support import static_archive_bytes
from tests.runtime_build_identity_helper import (
    native_runtime_staticlib_identity,
)

_FAKE_STATICLIB = static_archive_bytes(b"fake-staticlib")
_NATIVE_RUNTIME_BUILD_IDENTITY = native_runtime_staticlib_identity(
    cargo_profile="dev-fast",
    target_triple=None,
    family_seed="native-artifact-hydration",
)

# Fixture metadata digest used by runtime fingerprint hydration tests.
_TEST_RUNTIME_META_DIGEST = "ab" * 32
_TEST_RUNTIME_HASH_DIGEST = "cd" * 32
_TEST_RUNTIME_INPUTS_DIGEST = "ef" * 32


@pytest.fixture(autouse=True)
def _native_cargo_plan_authority(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD, "resolve_runtime_cargo_plan", runtime_cargo_plan
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


def _cargo_cpython_abi_artifact_stdout(path: Path) -> bytes:
    return (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": "path+file:///repo/runtime/molt-lang-cpython-abi#0.0.1",
                "target": {"name": "molt_cpython_abi"},
                "filenames": [str(path)],
                "fresh": True,
            }
        )
        + "\n"
    ).encode("utf-8")


def test_runtime_wasm_cargo_build_preserves_stale_candidates_and_uses_reported_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
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
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
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
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
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
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("release-fast")
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


@pytest.mark.parametrize("report_artifact", [True, False])
def test_cpython_abi_build_requires_and_fingerprints_only_reported_staticlib(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    report_artifact: bool,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    primary = RUNTIME_WASM_BUILD_SUPPORT._wasm_cpython_abi_staticlib_path(
        target_root, profile_dir
    )
    reported = (
        RUNTIME_WASM_BUILD_SUPPORT._wasm_runtime_deps_dir(target_root, profile_dir)
        / "libmolt_cpython_abi-feedface.a"
    )
    for path, payload in (
        (primary, b"stale-primary"),
        (reported, b"reported-staticlib"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        stdout = (
            _cargo_cpython_abi_artifact_stdout(reported)
            if report_artifact
            else b'{"reason":"build-finished","success":true}\n'
        )
        return subprocess.CompletedProcess(cmd, 0, stdout, b"")

    identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", target_triple="wasm32-wasip1"
    )
    state_root = target_root / ".molt_state"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "resolve_wasm_cpython_abi_build_identity",
        lambda *a, **k: identity,
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "resolve_runtime_cargo_plan", runtime_cargo_plan
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "runtime_build_tooling_authority", lambda _root: {}
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT,
        "_cargo_build_env",
        lambda: {
            "CARGO_TARGET_DIR": str(target_root),
            "MOLT_WASI_SYSROOT": str(tmp_path / "sysroot"),
        },
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_configure_wasm_cc_env", lambda _env: None
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_configure_wasi_sysroot_env", lambda _env: None
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_build_slot", lambda: contextlib.nullcontext(None)
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SUPPORT, "_run_subprocess_captured_to_tempfiles", fake_run
    )

    provider = RUNTIME_WASM_BUILD_SUPPORT._ensure_wasm_cpython_abi_staticlib(
        project_root=tmp_path,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
    )
    assert provider == (reported if report_artifact else None)
    assert primary.read_bytes() == b"stale-primary"
    reported_fp = cli._runtime_target_fingerprint_path(
        state_root,
        reported,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1.cpython-abi",
    )
    primary_fp = cli._runtime_target_fingerprint_path(
        state_root,
        primary,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1.cpython-abi",
    )
    assert reported_fp.exists() is report_artifact
    assert primary_fp.exists() is report_artifact
    if report_artifact:
        # The canonical requested-output sidecar may be named for the primary,
        # but its artifact identity must attest the exact Cargo-reported path.
        stored = cli._read_runtime_fingerprint(primary_fp)
        assert stored["build_identity"] == identity.to_dict()
        assert stored["artifact_content_identity"] == artifact_content_identity(
            reported
        )


@pytest.mark.parametrize("with_state", [True, False])
@pytest.mark.parametrize(
    "failure, expected_stage",
    [
        ("no_sysroot", "effective-configuration"),
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
        ("metadata_publish", "metadata-publication"),
    ],
)
def test_cpython_abi_failures_publish_consumable_evidence_in_json_mode(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failure: str,
    expected_stage: str,
    with_state: bool,
) -> None:
    from molt.cli import runtime_wasm_failure
    from molt.cli.cargo_execution import CargoExecutionResult, CargoPlanExecutionError
    from molt.cli.models import _RuntimeArtifactState

    support = RUNTIME_WASM_BUILD_SUPPORT
    target_root = tmp_path / "target"
    evidence_root = tmp_path / "state"
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
    monkeypatch.setattr(support, "_build_lock", lambda *_a: contextlib.nullcontext())
    monkeypatch.setattr(support, "_build_slot", lambda: contextlib.nullcontext())
    monkeypatch.setattr(support, "_configure_wasm_cc_env", lambda _env: None)
    monkeypatch.setattr(support, "_configure_wasi_sysroot_env", lambda _env: None)
    monkeypatch.setattr(
        support,
        "_cargo_build_env",
        lambda: (
            {}
            if failure == "no_sysroot"
            else {"MOLT_WASI_SYSROOT": str(tmp_path / "sysroot")}
        ),
    )
    monkeypatch.setattr(support, "resolve_runtime_cargo_plan", runtime_cargo_plan)
    monkeypatch.setattr(support, "runtime_build_tooling_authority", lambda _root: {})
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

    if failure == "cargo_plan":
        monkeypatch.setattr(support, "resolve_runtime_cargo_plan", reject)
    if failure == "metadata_read":
        monkeypatch.setattr(support, "_read_runtime_fingerprint", reject)
    if failure == "target_identity":
        monkeypatch.setattr(
            support,
            "_current_runtime_target_artifact",
            lambda *_a, **_k: (provider, {}),
        )
    if failure in {"artifact_identity", "metadata_refresh"}:
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
            else _cargo_cpython_abi_artifact_stdout(provider).decode("utf-8")
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
        if failure == "execute_drift":
            raise CargoPlanExecutionError("injected configuration drift", result)
        return result

    monkeypatch.setattr(support, "_run_resolved_cargo_plan", execute)
    assert (
        support._ensure_wasm_cpython_abi_staticlib(
            project_root=tmp_path,
            json_output=True,
            cargo_profile="dev-fast",
            cargo_timeout=1.0,
            runtime_state=state,
        )
        is None
    )
    paths = list((evidence_root / "build_failures").glob("runtime-wasm-*.json"))
    assert len(paths) == 1
    evidence = json.loads(paths[0].read_text(encoding="utf-8"))
    assert evidence["stage"] == f"cpython-abi-{expected_stage}"
    assert evidence["cwd"] == str(tmp_path)
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
            == "molt.cargo-execution.v1"
        )
        assert evidence["returncode"] == (7 if failure == "build_failure" else 0)
    output = capsys.readouterr()
    assert output.out == ""
    assert str(paths[0]) in output.err


def test_ensure_backend_binary_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_backend = canonical_target / "dev-fast" / "molt-backend"
    isolated_backend = isolated_target / "dev-fast" / "molt-backend"
    canonical_backend.parent.mkdir(parents=True, exist_ok=True)
    canonical_backend.write_text(
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
    )
    canonical_backend.chmod(0o755)

    fingerprint = {
        "hash": _TEST_RUNTIME_HASH_DIGEST,
        "rustc": "rustc",
        "inputs_digest": _TEST_RUNTIME_INPUTS_DIGEST,
        "meta_digest": _TEST_RUNTIME_META_DIGEST,
    }
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_backend,
        subdir="backend_fingerprints",
        stem_suffix="dev-fast",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_backend
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
        isolated_backend,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
    )
    assert isolated_backend.read_text() == canonical_backend.read_text()
    assert os.access(isolated_backend, os.X_OK)


def test_ensure_runtime_lib_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_runtime = canonical_target / "dev-fast" / "libmolt_runtime.a"
    isolated_runtime = isolated_target / "dev-fast" / "libmolt_runtime.a"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_bytes(_FAKE_STATICLIB)

    fingerprint = runtime_build_fingerprint(_NATIVE_RUNTIME_BUILD_IDENTITY)
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix="dev-fast.native",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_runtime
    )
    write_native_link_dependency_manifest(
        json.dumps(
            {
                "reason": "compiler-message",
                "message": {
                    "message": "native-static-libs: ",
                    "level": "note",
                },
            }
        ),
        runtime_lib=canonical_runtime,
        cargo_profile="dev-fast",
        target_triple=None,
        runtime_build_identity=_NATIVE_RUNTIME_BUILD_IDENTITY,
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "_runtime_build_identity_for_plan",
        lambda *args, **kwargs: _NATIVE_RUNTIME_BUILD_IDENTITY,
    )
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "_run_resolved_cargo_plan",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert RUNTIME_NATIVE_BUILD._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert isolated_runtime.read_bytes() == _FAKE_STATICLIB


def test_native_runtime_hydration_carries_portable_dependency_custody(
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
    fingerprint = runtime_build_fingerprint(build_identity)
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix=f"dev-fast.{target_triple}",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(
        canonical_fp,
        fingerprint,
        artifact=canonical_runtime,
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
    write_native_link_dependency_manifest(
        cargo_stdout,
        cargo_stderr="note: native-static-libs: -lportable\n",
        runtime_lib=canonical_runtime,
        cargo_profile="dev-fast",
        target_triple=target_triple,
        runtime_build_identity=build_identity,
    )
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

    assert RUNTIME_NATIVE_BUILD._ensure_runtime_lib(
        isolated_runtime,
        target_triple,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    manifest = read_native_link_dependency_manifest(
        isolated_runtime,
        target_triple=target_triple,
        cargo_profile="dev-fast",
        runtime_build_identity=build_identity,
    )
    flags = native_link_flags_from_manifest(
        manifest,
        object_format="elf",
        runtime_lib=isolated_runtime,
    )
    assert flags[-1] == "-lportable"
    custody_directory = Path(flags[-2][2:])
    assert custody_directory != library_dir
    assert (custody_directory / "libportable.a").read_bytes() == (
        b"portable dependency"
    )


def test_ensure_runtime_lib_hydration_requires_artifact_digest_match(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_runtime = canonical_target / "dev-fast" / "libmolt_runtime.a"
    isolated_runtime = isolated_target / "dev-fast" / "libmolt_runtime.a"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_bytes(static_archive_bytes(b"stale"))

    fingerprint = runtime_build_fingerprint(_NATIVE_RUNTIME_BUILD_IDENTITY)
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix="dev-fast.native",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_runtime
    )
    canonical_runtime.write_bytes(static_archive_bytes(b"mutated"))
    cargo_runs: list[list[str]] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD,
        "_runtime_build_identity_for_plan",
        lambda *args, **kwargs: _NATIVE_RUNTIME_BUILD_IDENTITY,
    )

    def fake_run_cargo(
        plan: RuntimeCargoPlan,
        *,
        timeout: float | None,
        json_output: bool,
        label: str,
    ) -> subprocess.CompletedProcess[str]:
        del timeout, json_output, label
        cmd = list(plan.command)
        cargo_runs.append(list(cmd))
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(
            isolated_runtime, None
        )
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(_FAKE_STATICLIB)
        cargo_note = json.dumps(
            {
                "reason": "compiler-message",
                "message": {
                    "message": "native-static-libs: ",
                    "level": "note",
                },
            }
        )
        return subprocess.CompletedProcess(cmd, 0, cargo_note + "\n", "")

    monkeypatch.setattr(
        RUNTIME_NATIVE_BUILD, "_run_resolved_cargo_plan", fake_run_cargo
    )

    assert RUNTIME_NATIVE_BUILD._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert cargo_runs
    assert isolated_runtime.read_bytes() == _FAKE_STATICLIB


@pytest.mark.parametrize("reloc", (False, True), ids=("shared", "reloc"))
@pytest.mark.parametrize("placement", ("primary", "deps", "hashed", "reported"))
def test_runtime_member_hydration_selects_attested_target_and_replays_without_cargo(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    reloc: bool,
    placement: str,
) -> None:
    from molt.cli import runtime_wasm_pair_build as pair_build
    from molt.cli.models import _RuntimeArtifactState
    from tests.runtime_build_identity_helper import (
        bind_runtime_wasm_specs,
        runtime_wasm_link_inputs,
    )

    target = tmp_path / "target"
    state = target / ".molt_state"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "_cargo_build_env",
        lambda: {"CARGO_TARGET_DIR": str(target)},
    )
    for name in (
        "_configure_wasm_cc_env",
        "_configure_wasi_sysroot_env",
        "_configure_wasm_long_double_env",
    ):
        monkeypatch.setattr(RUNTIME_WASM_BUILD_SPEC, name, lambda _env: None)
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC, "resolve_runtime_cargo_plan", runtime_cargo_plan
    )
    monkeypatch.setattr(
        RUNTIME_WASM_BUILD_SPEC,
        "resolve_runtime_wasm_link_inputs",
        lambda **kwargs: runtime_wasm_link_inputs(tmp_path, env=kwargs["env"]),
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
        tmp_path, tmp_path / "out/molt_runtime.wasm", reloc=False, **common
    )
    relative = RUNTIME_WASM_BUILD_SPEC._compute_runtime_wasm_build_spec(
        tmp_path, tmp_path / "out/molt_runtime_reloc.wasm", reloc=True, **common
    )
    shared, relative = bind_runtime_wasm_specs(shared, relative, root=tmp_path)
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
    if placement in {"hashed", "reported"}:
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
    selected_fp = cli._runtime_target_fingerprint_path(
        state, selected, cargo_profile=spec.cargo_profile, target_label="wasm32-wasip1"
    )
    if placement == "reported":
        other = primary_shared if reloc else primary_reloc
        other.write_bytes(_valid_wasm_bytes() if reloc else _FAKE_STATICLIB)
        # The combined producer, not mtime/candidate ordering, chooses both files.
        stdout = (
            _cargo_runtime_artifact_stdout(selected)
            + _cargo_runtime_artifact_stdout(other)
        ).decode()
        ctx = pair_build._CombinedRuntimeWasmBuild(
            _RuntimeArtifactState(), shared, relative, True, 1.0, tmp_path, True, False
        )
        assert pair_build._publish_combined_runtime_wasm_target(
            ctx,
            subprocess.CompletedProcess(["cargo"], 0, stdout, ""),
            other if reloc else selected,
        )
    else:
        selected_fp.parent.mkdir(parents=True, exist_ok=True)
        cli._write_runtime_fingerprint(selected_fp, fingerprint, artifact=selected)
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
        tmp_path
        / "hydrated"
        / ("molt_runtime_reloc.wasm" if reloc else "molt_runtime.wasm")
    )
    for _ in range(2):
        assert RUNTIME_WASM_BUILD._materialize_runtime_wasm_member_from_target(
            destination,
            reloc=reloc,
            json_output=True,
            cargo_timeout=1.0,
            project_root=tmp_path,
            required_exports=None,
            resolved_modules=None,
            spec=spec,
        )
        assert wasm_artifact.inspect_wasm_binary(destination) == "valid"
        destination.unlink()
    assert linked == ([selected, selected] if reloc else [])
    assert selected.read_bytes() == new_bytes
    if selected != primary:
        assert primary.read_bytes() == old_bytes
    assert cli._read_runtime_fingerprint(selected_fp)[
        "artifact_content_identity"
    ] == artifact_content_identity(selected)
