from __future__ import annotations

import functools
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
