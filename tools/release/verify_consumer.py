#!/usr/bin/env python3
"""Verify one immutable release candidate as a clean external consumer."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import time
import zipfile

from .release_model import sha256_file, write_json
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)


EXPECTED_OUTPUT = "MOLT_RELEASE_CONSUMER_OK"


def _safe_destination(root: Path, member: str) -> Path:
    destination = (root / member).resolve()
    if destination != root and root not in destination.parents:
        raise ValueError(f"release archive path escapes extraction root: {member}")
    return destination


def _extract(archive: Path, output: Path) -> None:
    output.mkdir(parents=True)
    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as handle:
            for member in handle.getmembers():
                _safe_destination(output, member.name)
                if not (member.isfile() or member.isdir()):
                    raise ValueError(
                        f"release archive contains a special file: {member.name}"
                    )
            handle.extractall(output, filter="data")
    elif archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                _safe_destination(output, member.filename)
            handle.extractall(output)
    else:
        raise ValueError(f"unsupported release archive: {archive}")


def _venv_python(root: Path) -> Path:
    return root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def _venv_molt(root: Path) -> Path:
    return root / ("Scripts/molt.exe" if os.name == "nt" else "bin/molt")


def _run(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int,
    expected_output: str | None = None,
) -> dict[str, object]:
    started = time.monotonic()
    result = _COMMANDS.run(
        argv,
        cwd=cwd,
        env=env,
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    duration = round(time.monotonic() - started, 6)
    if result.returncode != 0:
        raise RuntimeError(
            f"consumer command failed ({result.returncode}): {argv!r}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    if expected_output is not None and result.stdout.strip() != expected_output:
        raise RuntimeError(
            f"consumer command returned {result.stdout!r}; expected {expected_output!r}"
        )
    return {
        "argv": argv,
        "returncode": result.returncode,
        "duration_seconds": duration,
        "stdout_sha256": hashlib.sha256(result.stdout.encode("utf-8")).hexdigest(),
        "stderr_sha256": hashlib.sha256(result.stderr.encode("utf-8")).hexdigest(),
    }


def verify(candidate_dir: Path, receipt: Path) -> dict[str, object]:
    candidate_path = candidate_dir / "candidate.json"
    candidate = json.loads(candidate_path.read_text(encoding="utf-8"))
    artifacts = candidate.get("artifacts", [])
    if not isinstance(artifacts, list):
        raise ValueError("candidate artifacts must be a list")
    for record in artifacts:
        artifact = candidate_dir / str(record["filename"])
        if artifact.stat().st_size != record["size"]:
            raise ValueError(f"candidate artifact size mismatch: {artifact}")
        if sha256_file(artifact) != record["sha256"]:
            raise ValueError(f"candidate artifact digest mismatch: {artifact}")
    molt_records = [record for record in artifacts if record.get("kind") == "molt"]
    if len(molt_records) != 1:
        raise ValueError("candidate must contain exactly one Molt bundle")
    bundle = candidate_dir / str(molt_records[0]["filename"])

    with tempfile.TemporaryDirectory(prefix="molt-release-consumer-") as temporary:
        root = Path(temporary).resolve()
        extracted = root / "bundle"
        _extract(bundle, extracted)
        bundle_roots = [path for path in extracted.iterdir() if path.is_dir()]
        if len(bundle_roots) != 1:
            raise ValueError(
                "release bundle must contain exactly one top-level directory"
            )
        bundle_root = bundle_roots[0]
        wheels = sorted((bundle_root / "share" / "molt" / "wheels").glob("*.whl"))
        if len(wheels) != 1:
            raise ValueError("release bundle must contain exactly one wheel")
        if sha256_file(wheels[0]) != candidate["wheel"]["sha256"]:
            raise ValueError("bundled wheel does not match the canonical wheel")
        worker_name = "molt-worker.exe" if os.name == "nt" else "molt-worker"
        worker = bundle_root / "bin" / worker_name
        if not worker.is_file() or worker.stat().st_size == 0:
            raise ValueError(f"release bundle is missing {worker_name}")

        venv_root = root / "venv"
        clean_env = os.environ.copy()
        for name in (
            "PYTHONPATH",
            "PYTHONHOME",
            "VIRTUAL_ENV",
            "MOLT_BUNDLE_ROOT",
            "MOLT_VENV",
        ):
            clean_env.pop(name, None)
        clean_env["MOLT_HOME"] = str(root / "molt-home")
        clean_env["MOLT_PROJECT_ROOT"] = str(root / "project")
        clean_env["PIP_DISABLE_PIP_VERSION_CHECK"] = "1"
        (root / "project").mkdir()
        commands: list[dict[str, object]] = []
        commands.append(
            _run(
                [sys.executable, "-m", "venv", str(venv_root)],
                cwd=root,
                env=clean_env,
                timeout=120,
            )
        )
        python = _venv_python(venv_root)
        import_probe = _COMMANDS.run(
            [str(python), "-c", "import molt"],
            cwd=root,
            env=clean_env,
            capture_output=True,
            timeout=30,
            check=False,
        )
        if import_probe.returncode == 0:
            raise RuntimeError("clean consumer environment already imports Molt")
        commands.append(
            _run(
                [
                    str(python),
                    "-m",
                    "pip",
                    "install",
                    "--no-input",
                    str(wheels[0]),
                ],
                cwd=root,
                env=clean_env,
                timeout=600,
            )
        )
        molt = _venv_molt(venv_root)
        commands.append(
            _run([str(molt), "--help"], cwd=root, env=clean_env, timeout=60)
        )
        commands.append(
            _run([str(worker), "--help"], cwd=root, env=clean_env, timeout=60)
        )
        source = root / "project" / "release_consumer.py"
        source.write_text(f'print("{EXPECTED_OUTPUT}")\n', encoding="utf-8")
        executable = (
            root
            / "project"
            / ("release_consumer.exe" if os.name == "nt" else "release_consumer")
        )
        commands.append(
            _run(
                [
                    str(molt),
                    "build",
                    "--target",
                    "native",
                    "--output",
                    str(executable),
                    str(source),
                ],
                cwd=root / "project",
                env=clean_env,
                timeout=1800,
            )
        )
        if not executable.is_file():
            raise RuntimeError(
                f"Molt did not produce the requested binary: {executable}"
            )
        runtime_env = clean_env.copy()
        runtime_env.pop("PYTHON", None)
        commands.append(
            _run(
                [str(executable)],
                cwd=root / "project",
                env=runtime_env,
                timeout=60,
                expected_output=EXPECTED_OUTPUT,
            )
        )
        commands.append(
            _run(
                [str(python), "-m", "pip", "uninstall", "--yes", "molt"],
                cwd=root,
                env=clean_env,
                timeout=120,
            )
        )
        removed_probe = _COMMANDS.run(
            [str(python), "-c", "import molt"],
            cwd=root,
            env=clean_env,
            capture_output=True,
            timeout=30,
            check=False,
        )
        if removed_probe.returncode == 0 or molt.exists():
            raise RuntimeError("Molt remained importable or executable after uninstall")

        payload: dict[str, object] = {
            "schema": "molt.release-consumer-proof.v1",
            "candidate": candidate_path.name,
            "target": candidate["target"],
            "source_sha": candidate["source_sha"],
            "selected": 1,
            "executed": 1,
            "passed": 1,
            "failed": 0,
            "errors": 0,
            "commands": commands,
            "standalone_native_output": EXPECTED_OUTPUT,
            "uninstall_verified": True,
        }
        write_json(receipt, payload)
        return payload


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    payload = verify(args.candidate, args.receipt)
    print(json.dumps(payload, sort_keys=True))


if __name__ == "__main__":
    main()
