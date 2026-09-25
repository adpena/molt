#!/usr/bin/env python3
"""Verify one immutable release candidate as a clean external consumer."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tarfile
import tempfile
import time

from tools.command_execution import CommandExecutor

from .archive import extract_zip_strict
from .build_bundle import RELEASE_BUNDLE_ARCHIVE_POLICY
from .release_authority import (
    CONSUMER_EXPECTED_OUTPUT,
    CONSUMER_SCHEMA,
    consumer_python_policy,
    _load_candidate,
    validate_consumer_proof,
)
from molt.compiler_distribution import InstalledCompiler, installed_compiler
from molt.exact_json import canonical_json_sha256, loads_exact
from molt.file_publication import durable_remove_path
from molt.python_interpreter import probe_python_command
from molt.toolchain_identity import executable_content_identity
from molt.verified_subset import current_host_coordinate, host_coordinate
from .release_model import sha256_file, write_json

_COMMANDS = CommandExecutor.for_file(__file__)


EXPECTED_OUTPUT = CONSUMER_EXPECTED_OUTPUT
_HOST_PROBE = (
    "import json,platform,struct,sysconfig;"
    "print(json.dumps({'system':platform.system(),'machine':platform.machine(),"
    "'pointer_bits':struct.calcsize('P')*8,"
    "'gil_disabled':bool(sysconfig.get_config_var('Py_GIL_DISABLED') or 0)}))"
)


def _safe_destination(root: Path, member: str) -> Path:
    destination = (root / member).resolve()
    if destination != root and root not in destination.parents:
        raise ValueError(f"release archive path escapes extraction root: {member}")
    return destination


def _extract(archive: Path, output: Path) -> None:
    if archive.name.endswith(".tar.gz"):
        output.mkdir(parents=True)
        with tarfile.open(archive, "r:gz") as handle:
            for member in handle.getmembers():
                _safe_destination(output, member.name)
                if not (member.isfile() or member.isdir()):
                    raise ValueError(
                        f"release archive contains a special file: {member.name}"
                    )
            handle.extractall(output, filter="data")
    elif archive.suffix == ".zip":
        extract_zip_strict(archive, output, policy=RELEASE_BUNDLE_ARCHIVE_POLICY)
    else:
        raise ValueError(f"unsupported release archive: {archive}")


def _venv_python(root: Path) -> Path:
    return root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def _launcher(bundle_root: Path) -> list[str]:
    return [str(bundle_root / "bin" / ("molt.exe" if os.name == "nt" else "molt"))]


def _run(
    argv: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: int,
    role: str,
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
    if expected_output is not None and result.stdout != expected_output + "\n":
        raise RuntimeError(
            f"consumer command returned {result.stdout!r}; expected {expected_output!r}"
        )
    return {
        "role": role,
        "argv": argv,
        "returncode": result.returncode,
        "duration_seconds": duration,
        "stdout_sha256": hashlib.sha256(result.stdout.encode("utf-8")).hexdigest(),
        "stderr_sha256": hashlib.sha256(result.stderr.encode("utf-8")).hexdigest(),
    }


def _capture_python_execution(
    python: Path,
    *,
    env: dict[str, str],
    cwd: Path,
    target: dict[str, object],
    reference_python: str,
) -> dict[str, object]:
    interpreter = probe_python_command((str(python), "-I", "-B"), env=env, cwd=cwd)
    result = _COMMANDS.run(
        [str(python), "-I", "-B", "-c", _HOST_PROBE],
        env=env,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=30,
        check=True,
    )
    observed = loads_exact(result.stdout)
    if (
        not isinstance(observed, dict)
        or set(observed) != {"system", "machine", "pointer_bits", "gil_disabled"}
        or not isinstance(observed["system"], str)
        or not isinstance(observed["machine"], str)
        or type(observed["pointer_bits"]) is not int
        or observed["pointer_bits"] != 64
        or observed["gil_disabled"] is not False
        or interpreter.version != reference_python
        or Path(interpreter.executable).absolute() != python.absolute()
    ):
        raise ValueError("release consumer selected Python differs from its coordinate")
    platform, arch = host_coordinate(observed["system"], observed["machine"])
    if (platform, arch) != (target["platform"], target["arch"]):
        raise ValueError("release consumer selected Python host differs from candidate")
    identity = executable_content_identity(python, label="release consumer Python")
    return {
        "host": {"platform": platform, "arch": arch, "pointer_bits": 64},
        "python": {
            "implementation": interpreter.implementation,
            "version": interpreter.version,
            "executable": str(python),
            "sha256": identity["sha256"],
            "size": identity["size"],
            "gil_disabled": False,
        },
    }


def _absent_probe(python: Path, *, env: dict[str, str], cwd: Path) -> None:
    _COMMANDS.run(
        [
            str(python),
            "-I",
            "-c",
            "import importlib.util; assert importlib.util.find_spec('molt') is None",
        ],
        cwd=cwd,
        env=env,
        capture_output=True,
        timeout=30,
        check=True,
    )


def _consumer_environment(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    for name in (
        "PYTHONPATH",
        "PYTHONHOME",
        "VIRTUAL_ENV",
        "MOLT_BUNDLE_ROOT",
        "MOLT_SOURCE_ROOT",
        "MOLT_BACKEND_PROFILE",
        "MOLT_DEV_BACKEND_CARGO_PROFILE",
        "MOLT_RELEASE_BACKEND_CARGO_PROFILE",
        "MOLT_DEV_CARGO_PROFILE",
        "MOLT_RELEASE_CARGO_PROFILE",
        "MOLT_SKIP_RUNTIME_REBUILD",
    ):
        env.pop(name, None)
    env["MOLT_HOME"] = str(root / "molt-home")
    env.pop("MOLT_PROJECT_ROOT", None)
    env["PIP_DISABLE_PIP_VERSION_CHECK"] = "1"
    return env


def _verify_python_coordinate(
    *,
    root: Path,
    bundle_root: Path,
    worker: Path,
    compiler: InstalledCompiler,
    target: dict[str, object],
    minor: str,
    reference: str,
) -> dict[str, object]:
    project = root / "project"
    project.mkdir(parents=True)
    env = _consumer_environment(root)
    venv = root / "venv"
    commands = [
        _run(
            ["uv", "venv", "--no-config", "--python", reference, str(venv)],
            cwd=root,
            env=env,
            timeout=600,
            role="environment",
        )
    ]
    python = _venv_python(venv)
    execution = _capture_python_execution(
        python,
        env=env,
        cwd=root,
        target=target,
        reference_python=reference,
    )
    _absent_probe(python, env=env, cwd=root)
    env["PYTHON"] = str(python)
    launcher = _launcher(bundle_root)
    commands.append(
        _run(
            [*launcher, "setup", "--install-cli-dependencies"],
            cwd=project,
            env=env,
            timeout=600,
            role="cli_setup",
        )
    )
    commands.append(
        _run(
            [*launcher, "--help"],
            cwd=project,
            env=env,
            timeout=600,
            role="cli_help",
        )
    )
    commands.append(
        _run(
            [str(worker), "--help"],
            cwd=root,
            env=env,
            timeout=60,
            role="worker_help",
        )
    )
    source = project / "release_consumer.py"
    major, minor_number = (int(part) for part in minor.split("."))
    source.write_text(
        f"import sys\nassert sys.version_info[:2] == ({major}, {minor_number})\n"
        f"print({EXPECTED_OUTPUT!r})\n",
        encoding="utf-8",
    )
    profiles = []
    for profile in ("dev", "release"):
        executable = project / (
            f"release_consumer_{profile}" + (".exe" if os.name == "nt" else "")
        )
        diagnostics = project / f"diagnostics-{profile}.json"
        commands.append(
            _run(
                [
                    *launcher,
                    "build",
                    "--target",
                    "native",
                    "--profile",
                    profile,
                    "--python-version",
                    minor,
                    "--diagnostics-file",
                    str(diagnostics),
                    "--output",
                    str(executable),
                    str(source),
                ],
                cwd=root,
                env=env,
                timeout=2700,
                role=f"build_{profile}",
            )
        )
        if not executable.is_file():
            raise RuntimeError(
                f"Molt did not produce the requested binary: {executable}"
            )
        observed = loads_exact(diagnostics.read_text(encoding="utf-8"))
        selected = observed.get("compiler", {})
        if (
            selected.get("sha256") != compiler.record["sha256"]
            or selected.get("cargo_profile") != "release"
            or Path(str(selected.get("path", ""))).resolve()
            != compiler.binary.resolve()
            or observed.get("program", {}).get("profile") != profile
        ):
            raise ValueError(
                f"{minor}/{profile}: guest coordinate changed production compiler"
            )
        compiler.verify_binary(("native-backend",), "release")
        runtime_env = env.copy()
        runtime_env.pop("PYTHON", None)
        commands.append(
            _run(
                [str(executable)],
                cwd=project,
                env=runtime_env,
                timeout=60,
                expected_output=EXPECTED_OUTPUT,
                role=f"run_{profile}",
            )
        )
        profiles.append(
            {
                "profile": profile,
                "compiler_sha256": selected["sha256"],
                "compiler_fingerprint": selected["fingerprint"],
            }
        )
    if (
        _capture_python_execution(
            python,
            env=env,
            cwd=root,
            target=target,
            reference_python=reference,
        )
        != execution
    ):
        raise ValueError(
            "release consumer interpreter identity changed during verification"
        )
    return {
        "python": minor,
        "reference_python": reference,
        "execution": execution,
        "commands": commands,
        "profile_proofs": profiles,
    }


def verify(candidate_dir: Path, receipt: Path) -> dict[str, object]:
    candidate_path = candidate_dir / "candidate.json"
    candidate = _load_candidate(candidate_path)
    if current_host_coordinate() != (
        candidate["target"]["platform"],
        candidate["target"]["arch"],
    ):
        raise ValueError("release consumer must run on its candidate host")
    coordinates, policy_sha256 = consumer_python_policy()
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

    with tempfile.TemporaryDirectory(prefix="molt release consumer ") as temporary:
        root = Path(temporary).resolve()
        extracted = root / "bundle"
        _extract(bundle, extracted)
        bundle_roots = [path for path in extracted.iterdir() if path.is_dir()]
        if len(bundle_roots) != 1:
            raise ValueError(
                "release bundle must contain exactly one top-level directory"
            )
        bundle_root = bundle_roots[0]
        worker_records = [
            record for record in artifacts if record.get("kind") == "molt-worker"
        ]
        if len(worker_records) != 1:
            raise ValueError(
                "candidate must contain exactly one standalone worker bundle"
            )
        worker_extract = root / "worker"
        _extract(candidate_dir / str(worker_records[0]["filename"]), worker_extract)
        worker_name = "molt-worker.exe" if os.name == "nt" else "molt-worker"
        worker_root = worker_extract / f"molt-worker-{candidate['version']}"
        if list(worker_extract.iterdir()) != [worker_root]:
            raise ValueError("standalone worker bundle must have one exact root")
        worker = worker_root / "bin" / worker_name
        if not worker.is_file() or worker.stat().st_size == 0:
            raise ValueError(f"standalone worker bundle is missing {worker_name}")
        if (bundle_root / "bin" / worker_name).exists():
            raise ValueError(
                "compiler bundle must not duplicate standalone worker ownership"
            )

        compiler = installed_compiler(bundle_root / "source")
        if compiler is None or compiler.source_sha != candidate["source_sha"]:
            raise ValueError("Bundle compiler source differs from candidate")
        if compiler.record != candidate["compiler"]:
            raise ValueError("Bundle compiler identity differs from candidate")
        if compiler.launcher != candidate["launcher"]:
            raise ValueError("Bundle launcher identity differs from candidate")
        compiler.verify_launcher()
        compiler.verify_sources()
        compiler.verify_binary(("native-backend", "wasm-backend"), "release")
        if consumer_python_policy(
            bundle_root / "source/config/verified_subset.toml"
        ) != (
            coordinates,
            policy_sha256,
        ):
            raise ValueError("Bundle Python policy differs from release authority")
        proofs = [
            _verify_python_coordinate(
                root=root / f"python-{minor}",
                bundle_root=bundle_root,
                worker=worker,
                compiler=compiler,
                target=candidate["target"],
                minor=minor,
                reference=reference,
            )
            for minor, reference in coordinates
        ]
        compiler.verify_sources()
        # Remove both the portable bundle and its private installed environments;
        # checking only the untouched bootstrap interpreter would miss residue.
        durable_remove_path(bundle_root, retirement_scope="consumer-uninstall")
        durable_remove_path(worker_root, retirement_scope="consumer-uninstall")
        for minor, _ in coordinates:
            coordinate_root = root / f"python-{minor}"
            home = coordinate_root / "molt-home"
            durable_remove_path(home, retirement_scope="consumer-uninstall")
            if home.exists():
                raise RuntimeError(
                    "Molt's private environment remained after uninstall"
                )
            _absent_probe(
                _venv_python(coordinate_root / "venv"),
                env=_consumer_environment(coordinate_root),
                cwd=coordinate_root,
            )
        if bundle_root.exists() or worker_root.exists():
            raise RuntimeError(
                "Molt or worker remained installed after portable bundle removal"
            )

        count = len(coordinates) * 2
        payload: dict[str, object] = {
            "schema": CONSUMER_SCHEMA,
            "candidate": candidate_path.name,
            "candidate_sha256": canonical_json_sha256(candidate),
            "target": candidate["target"],
            "source_sha": candidate["source_sha"],
            "selected": count,
            "executed": count,
            "passed": count,
            "failed": 0,
            "errors": 0,
            "compiler": compiler.record,
            "launcher": compiler.launcher,
            "guest_profiles": ["dev", "release"],
            "python_policy_sha256": policy_sha256,
            "python_proofs": proofs,
            "standalone_native_output": EXPECTED_OUTPUT,
            "uninstall_verified": True,
        }
        validate_consumer_proof(payload, candidate)
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
