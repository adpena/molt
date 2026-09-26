from __future__ import annotations

import functools
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from tests.process_guard_common import run_custody_subject_process
from tools.proof_queue_pkg import (
    execution_custody,
    process_image_capture,
    supervisor_custody,
)


def test_supervisor_sources_follow_local_dependencies_and_workspace_inheritance(
    tmp_path: Path,
) -> None:
    source = tmp_path / "tools" / "proof_supervisor"
    shared = tmp_path / "runtime" / "shared"
    for crate in (source, shared):
        (crate / "src").mkdir(parents=True)
        (crate / "src" / "lib.rs").write_text("// authority\n", encoding="utf-8")
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers=["runtime/shared"]\n'
        '[workspace.package]\nedition="2024"\n',
        encoding="utf-8",
    )
    (source / "Cargo.toml").write_text(
        '[package]\nname="supervisor"\nversion="0.1.0"\n'
        "[workspace]\nmembers=[]\n"
        "[target.'cfg(windows)'.dependencies]\n"
        'shared={path="../../runtime/shared"}\n',
        encoding="utf-8",
    )
    (shared / "Cargo.toml").write_text(
        '[package]\nname="shared"\nversion="0.1.0"\nedition.workspace=true\n',
        encoding="utf-8",
    )
    for name in ("build.py", "Cargo.lock"):
        (source / name).write_text("", encoding="utf-8")
    shared_asset = shared / "src" / "schema.json"
    shared_asset.write_text("{}", encoding="utf-8")
    unrelated = tmp_path / "src" / "unrelated.rs"
    unrelated.parent.mkdir()
    unrelated.write_text("// not a supervisor crate\n", encoding="utf-8")

    paths = supervisor_custody.source_authority_paths(tmp_path)

    assert paths == tuple(sorted(set(paths)))
    assert (tmp_path / "Cargo.toml").resolve() in paths
    assert (shared / "Cargo.toml").resolve() in paths
    assert (shared / "src" / "lib.rs").resolve() in paths
    assert shared_asset.resolve() in paths
    assert unrelated.resolve() not in paths
    (shared / "Cargo.toml").unlink()
    with pytest.raises(ValueError, match="Cargo manifest"):
        supervisor_custody.source_authority_paths(tmp_path)


@pytest.mark.parametrize(
    "payload",
    [
        '{"root_exit_code":1,"root_exit_code":0}',
        '{"value":NaN}',
        '{"value":1e999}',
        "{",
    ],
)
def test_verified_supervisor_receipt_still_requires_exact_json(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    payload: str,
) -> None:
    receipt = tmp_path / "receipt.json"
    receipt.write_text(payload, encoding="utf-8")
    monkeypatch.setattr(
        supervisor_custody.command_identity,
        "_run_captured",
        lambda *a, **kw: subprocess.CompletedProcess([], 0, "", ""),
    )
    with pytest.raises(ValueError, match="no readable receipt"):
        supervisor_custody._validated_supervisor_receipt(
            binary=tmp_path / "never-executed",
            policy_path=tmp_path / "policy.json",
            receipt_path=receipt,
            cwd=tmp_path,
            env={},
        )


def test_supervisor_policy_cannot_publish_nonfinite_json(tmp_path: Path) -> None:
    policy = tmp_path / "policy.json"
    with pytest.raises(ValueError):
        supervisor_custody._atomic_json(policy, {"timeout": float("inf")})
    assert not policy.exists()


@functools.lru_cache(maxsize=1)
def _test_proof_supervisor_binary() -> Path:
    build = (
        Path(supervisor_custody.__file__).resolve().parents[1]
        / "proof_supervisor"
        / "build.py"
    )
    completed = run_custody_subject_process(
        [sys.executable, str(build), "--release"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return Path(completed.stdout.splitlines()[-1]).resolve(strict=True)


def test_supervisor_admits_exact_platform_image_without_directory_authority(
    tmp_path: Path,
) -> None:
    root = Path(sys.executable).resolve(strict=True)
    broker = tmp_path / "conhost.exe"
    shutil.copy2(root, broker)
    platform_image = process_image_capture.capture_image(
        "windows-console-broker", broker, root_exit_disposition="terminate"
    )

    root_role, images = supervisor_custody._supervisor_fixed_images(
        {}, {}, [str(root)], [platform_image]
    )
    derived = supervisor_custody._supervisor_derived_roots(
        descendants="declared-toolchains", env={}
    )

    assert root_role == "root-command"
    assert [row for row in images if row["role"] == "windows-console-broker"] == [
        {
            "role": "windows-console-broker",
            "path": str(broker),
            "sha256": platform_image["sha256"],
            "root_exit_disposition": "terminate",
        }
    ]
    assert derived == []


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"},
    reason="lossless process inventory is available on Windows and Linux",
)
def test_process_image_inventory_captures_distinct_runtime_and_projects_once(
    tmp_path: Path,
) -> None:
    supervisor = _test_proof_supervisor_binary()
    runtime = tmp_path / ("runtime.exe" if sys.platform == "win32" else "runtime")
    shutil.copy2(supervisor, runtime)

    images, telemetry = supervisor_custody.capture_process_image_inventory(
        binary=supervisor,
        role="fixture",
        executable=supervisor,
        probe_args=["fixture-child", "spawn-and-wait", str(runtime)],
        cwd=tmp_path,
        env=os.environ,
    )

    assert telemetry["schema"] == "molt.proof-process-image-inventory.v1"
    assert telemetry["observed_image_count"] == 2
    assert {Path(str(image["path"])).resolve() for image in images} == {
        supervisor.resolve(),
        runtime.resolve(),
    }
    launcher = next(image for image in images if image["role"] == "fixture-launcher")
    identity = {
        "path": str(supervisor),
        "launcher_sha256": launcher["sha256"],
        "content_path": str(supervisor),
        "executable_sha256": launcher["sha256"],
        "process_images": images,
    }
    envelope = {
        "process_closure": {
            "kind": "registered-toolchain",
            "descendants": "declared-toolchains",
            "toolchains": ["fixture"],
        }
    }
    child = execution_custody.child_policy(envelope, {"fixture": identity})
    _root_role, fixed = supervisor_custody._supervisor_fixed_images(
        {"fixture": identity}, {}, [str(supervisor)]
    )
    child_images = {
        (row["path"], row["sha256"])
        for row in child["allowed"]
        if row["toolchain"] == "fixture"
    }
    supervisor_images = {
        (execution_custody._norm(row["path"]), row["sha256"])
        for row in fixed
        if row["role"].startswith("fixture-")
    }
    assert child_images == supervisor_images


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"},
    reason="lossless process supervision is available on Windows and Linux",
)
def test_python_generated_child_matches_native_execution_identity(tmp_path: Path):
    supervisor = _test_proof_supervisor_binary()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    source = tmp_path / "source"
    source.mkdir()
    receipt_path = tmp_path / "native-receipt.json"
    environment = dict(os.environ)
    environment.pop("CARGO_TARGET_DIR", None)
    environment[supervisor_custody.PROOF_SCRATCH_ROOT_ENV] = str(scratch)
    required = supervisor_custody.required_execution_environment(
        binary=supervisor,
        mode="declared-tree",
        cwd=tmp_path,
        env=environment,
    )
    environment = supervisor_custody.bind_required_environment(environment, required)
    roots = supervisor_custody._derived_root_provenance(
        descendants="declared-toolchains",
        env=environment,
        source_root=source,
        result_path=receipt_path,
    )
    envelope = {"process_closure": {"descendants": "declared-toolchains"}}
    child_policy = execution_custody.child_policy(envelope, {}, derived_roots=roots)
    generated = scratch / supervisor.name
    # Materialize the candidate after admission, then launch through the actual
    # Python hook. The OS supervisor must observe the same path and bytes.
    payload = (
        "import shutil,subprocess; "
        f"shutil.copy2({str(supervisor)!r},{str(generated)!r}); "
        f"subprocess.run([{str(generated)!r},'fixture-child','exit','0'],check=True)"
    )
    bootstrap = Path(execution_custody.__file__).with_name(
        "python_custody_bootstrap.py"
    )
    command = [
        str(Path(sys._base_executable).resolve()),
        str(bootstrap),
        "command",
        "0",
        payload,
    ]
    server = execution_custody.ChildCustodyEventServer("python", child_policy)
    environment.update(server.environment())
    environment[execution_custody.CHILD_POLICY_ENV] = json.dumps(child_policy)
    policy_path = tmp_path / "native-policy.json"
    native_policy = supervisor_custody._supervisor_policy(
        envelope=envelope,
        execution_command=command,
        execution_env=environment,
        cwd=tmp_path,
        nonce="a" * 64,
        toolchains={},
        environment_executables={},
        platform_process_images=process_image_capture.platform_auxiliary_images(
            "declared-toolchains"
        ),
    )
    assert native_policy["derived_roots"] == child_policy["derived_roots"]
    supervisor_custody._atomic_json(policy_path, native_policy)
    with server:
        completed = run_custody_subject_process(
            [
                str(supervisor),
                "run",
                "--policy",
                str(policy_path),
                "--receipt",
                str(receipt_path),
            ],
            cwd=tmp_path,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    assert completed.returncode == 0, completed.stderr
    native = supervisor_custody._validated_supervisor_receipt(
        binary=supervisor,
        policy_path=policy_path,
        receipt_path=receipt_path,
        cwd=tmp_path,
        env=environment,
    )
    assert native["complete"] is True
    assert native["root_exit_code"] == 0
    assert native["violations"] == []
    child = server.receipt()
    assert execution_custody.child_receipt_is_admitted(child)
    decisions = [
        event for event in child["events"] if event.get("event") == "child-process"
    ]
    assert len(decisions) == 1
    assert decisions[0]["derived_role"] == supervisor_custody.SCRATCH_OUTPUT_ROLE
    assert decisions[0]["resolved"] == str(generated.resolve())
    event_log = receipt_path.with_name(native["event_log"]["file"])
    execution_custody.require_derived_child_image_bindings(child, event_log)


def test_process_image_authority_rejects_mutation_and_conflicting_identity(
    tmp_path: Path,
) -> None:
    executable = tmp_path / "tool.exe"
    executable.write_bytes(b"before")
    captured = process_image_capture.capture_image("tool", executable)
    selection = process_image_capture.capture_image(
        "tool", executable, preserve_path=True
    )
    conflict = {**captured, "role": "tool-runtime", "sha256": "0" * 64}

    assert process_image_capture.canonical_images([captured, selection]) == [selection]
    with pytest.raises(ValueError, match="conflicting identities"):
        process_image_capture.canonical_images([captured, conflict])

    executable.write_bytes(b"after")
    with pytest.raises(ValueError, match="changed while live custody armed"):
        process_image_capture.revalidate_images([captured])


@pytest.mark.skipif(
    sys.platform not in {"win32", "linux"} or shutil.which("git") is None,
    reason="real Git inventory requires a lossless native backend and Git",
)
def test_real_git_launcher_runtime_closure_is_kernel_observed(tmp_path: Path) -> None:
    git = Path(str(shutil.which("git"))).resolve(strict=True)

    images, telemetry = supervisor_custody.capture_process_image_inventory(
        binary=_test_proof_supervisor_binary(),
        role="git",
        executable=git,
        probe_args=["--version"],
        cwd=tmp_path,
        env=os.environ,
    )

    assert telemetry["observed_image_count"] == len(images)
    assert any(Path(str(image["path"])).samefile(git) for image in images)
    assert process_image_capture.revalidate_images(images) == images
