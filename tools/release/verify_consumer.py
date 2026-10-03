#!/usr/bin/env python3
"""Verify one immutable release candidate as a clean external consumer."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import time
from typing import Any

from tools.command_execution import CommandExecutor

from .archive import extract_zip_strict
from .build_bundle import RELEASE_BUNDLE_ARCHIVE_POLICY
from .release_authority import (
    CONSUMER_EXPECTED_STDOUT,
    CONSUMER_GUEST_ARGV,
    CONSUMER_GUEST_CELLS,
    CONSUMER_SCHEMA,
    consumer_guest_command,
    consumer_guest_source,
    consumer_python_policy,
    _load_candidate,
    validate_consumer_proof,
)
from molt.compiler_distribution import InstalledCompiler, installed_compiler
from molt.exact_json import canonical_json_sha256, loads_exact
from molt.file_publication import durable_remove_path
from molt.python_interpreter import probe_python_command
from molt.toolchain_identity import (
    executable_candidates,
    executable_content_identity,
    stable_regular_file_content_identity,
)
from molt.verified_subset import current_host_coordinate, host_coordinate
from molt.wasm_artifact import (
    wasm_runtime_manifest_entry_path,
    wasm_runtime_manifest_path,
)
from .release_model import sha256_file, write_json
from .binary_compatibility import (
    WheelCompatibilityError,
    audit_linked_executable,
    audit_wheel,
)
from .runtime_cells import declared_cell_keys, inventory_cell_keys
from .native_build import rust_channel

_COMMANDS = CommandExecutor.for_file(__file__)


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
    expected_stdout: str | None = None,
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
    if expected_stdout is not None and result.stdout != expected_stdout:
        raise RuntimeError(
            f"consumer command returned {result.stdout!r}; expected {expected_stdout!r}"
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
        "PYTHON",
        "MOLT_BUNDLE_ROOT",
        "MOLT_SOURCE_ROOT",
        "MOLT_BACKEND_PROFILE",
        "MOLT_DEV_BACKEND_CARGO_PROFILE",
        "MOLT_RELEASE_BACKEND_CARGO_PROFILE",
        "MOLT_DEV_CARGO_PROFILE",
        "MOLT_RELEASE_CARGO_PROFILE",
        "MOLT_SKIP_RUNTIME_REBUILD",
        "MOLT_WASM_RUNTIME_DIR",
        "MOLT_WASM_CARGO_PROFILE",
        "MOLT_RUNTIME_BUILD_PROFILE",
    ):
        env.pop(name, None)
    env["MOLT_HOME"] = str(root / "molt-home")
    env.pop("MOLT_PROJECT_ROOT", None)
    env["PIP_DISABLE_PIP_VERSION_CHECK"] = "1"
    return rust_toolchain_absent_environment(env, root=root)


def _diagnosed_compiler(
    diagnostics: Path,
    *,
    compiler: InstalledCompiler,
    target: str,
    profile: str,
) -> tuple[str, str]:
    """Read one cell compiler and program identity from its own build diagnostics."""
    if not diagnostics.is_file():
        raise RuntimeError(f"{target}/{profile}: the cell build wrote no diagnostics")
    observed = loads_exact(diagnostics.read_text(encoding="utf-8"))
    selected = observed.get("compiler") if isinstance(observed, dict) else None
    program = observed.get("program") if isinstance(observed, dict) else None
    if (
        not isinstance(selected, dict)
        or not isinstance(program, dict)
        or selected.get("sha256") != compiler.record["sha256"]
        or selected.get("cargo_profile") != "release"
        or Path(str(selected.get("path", ""))).resolve() != compiler.binary.resolve()
        or program.get("profile") != profile
        or program.get("target") != target
    ):
        raise ValueError(f"{target}/{profile}: guest cell changed production compiler")
    compiler.verify_binary((f"{target}-backend",), "release")
    return selected["sha256"], selected["fingerprint"]


def _linked_wasm_artifact(output: Path) -> dict[str, object]:
    """Identify the module named by the execution manifest beside ``output``."""
    module = wasm_runtime_manifest_entry_path(wasm_runtime_manifest_path(output))
    identity = stable_regular_file_content_identity(
        module, label="release consumer linked WASM"
    )
    return {"path": str(module), "sha256": identity["sha256"], "size": identity["size"]}


_RUST_TOOLCHAIN_COMMANDS = ("cargo", "rustc", "rustup")
_RUST_TOOLCHAIN_ENVIRONMENT = (
    "CARGO",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_TARGET_DIR",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "RUSTUP_TOOLCHAIN",
)


def _same_directory(entry: str, directory: Path) -> bool:
    raw = entry.strip().strip('"')
    return bool(raw) and os.path.normcase(os.path.abspath(raw)) == os.path.normcase(
        os.path.abspath(directory)
    )


def rust_toolchain_absent_environment(
    env: dict[str, str], *, root: Path
) -> dict[str, str]:
    """Consumer environment in which no Rust toolchain can be resolved.

    Installed Molt must compile guests with Cargo, rustc and rustup absent: Rust
    selectors are removed, Cargo/rustup homes point at empty private roots, and
    every PATH directory providing a Rust command is excluded through the shared
    executable-search authority. A Rust tool that shares a directory with the
    consumer's own tools fails closed instead of weakening the proof.
    """
    result = dict(env)
    for name in _RUST_TOOLCHAIN_ENVIRONMENT:
        result.pop(name, None)
    empty = root / "rust-toolchain-absent"
    for name, variable in (("cargo", "CARGO_HOME"), ("rustup", "RUSTUP_HOME")):
        (empty / name).mkdir(parents=True, exist_ok=True)
        result[variable] = str(empty / name)
    providers = {
        candidate.parent
        for command in _RUST_TOOLCHAIN_COMMANDS
        for candidate in executable_candidates(command, environment=result)
    }
    keys = [
        key for key in result if (key.upper() if os.name == "nt" else key) == "PATH"
    ] or ["PATH"]
    for key in keys:
        result[key] = os.pathsep.join(
            entry
            for entry in result.get(key, "").split(os.pathsep)
            if entry and not any(_same_directory(entry, path) for path in providers)
        )
    remaining = [
        str(candidate)
        for command in _RUST_TOOLCHAIN_COMMANDS
        for candidate in executable_candidates(command, environment=result)
    ]
    if remaining:
        raise RuntimeError(
            "release consumer cannot make the Rust toolchain unavailable: "
            + ", ".join(remaining)
        )
    if not list(executable_candidates("uv", environment=result)):
        raise RuntimeError(
            "release consumer Rust toolchain shares a PATH directory with uv; "
            "use a host whose Rust toolchain is installed separately"
        )
    return result


def _require_no_cargo_runtime(home: Path) -> None:
    built = (
        sorted(path.name for path in (home / "target").rglob("*molt_runtime*"))
        if (home / "target").exists()
        else []
    )
    if built:
        raise RuntimeError(
            f"installed Molt built runtime artifacts with Cargo: {built[:5]}"
        )


def _record_compatibility_failure(
    evidence_dir: Path,
    error: WheelCompatibilityError,
    artifact: Path,
    *,
    preserve: bool,
) -> None:
    """Keep the audit evidence, and a transient guest binary, for repair."""
    evidence_dir.mkdir(parents=True, exist_ok=True)
    preserved = evidence_dir / artifact.name if preserve else artifact
    if preserve:
        shutil.copy2(artifact, preserved)
    write_json(
        evidence_dir / f"{artifact.name}.compatibility.json",
        {"error": str(error), "artifact": str(preserved), "evidence": error.evidence},
    )


def _audit_platform_wheel(
    wheel: Path, *, target: dict[str, object], evidence_dir: Path
) -> str:
    """The candidate wheel's tag must be the one its own binaries admit."""
    try:
        return audit_wheel(
            wheel, platform=str(target["platform"]), arch=str(target["arch"])
        ).tag
    except WheelCompatibilityError as exc:
        _record_compatibility_failure(evidence_dir, exc, wheel, preserve=False)
        raise


def _audit_guest(
    executable: Path, *, target: dict[str, object], wheel_tag: str, evidence_dir: Path
) -> None:
    """A program linked from shipped runtime cells must fit the wheel's claim."""
    try:
        audit_linked_executable(
            executable,
            platform=str(target["platform"]),
            arch=str(target["arch"]),
            claimed_tag=wheel_tag,
        )
    except WheelCompatibilityError as exc:
        _record_compatibility_failure(evidence_dir, exc, executable, preserve=True)
        raise


def _verify_pip_distribution(
    *,
    root: Path,
    wheel: Path,
    wheel_record: dict[str, object],
    compiler: InstalledCompiler,
    minor: str,
    reference: str,
    target: dict[str, object],
    wheel_tag: str,
    evidence_dir: Path,
) -> dict[str, Any]:
    """Install the platform wheel with plain pip semantics and build natively."""
    root.mkdir(parents=True)
    env = _consumer_environment(root)
    venv = root / "venv"
    python = _venv_python(venv)
    commands = [
        _run(
            ["uv", "venv", "--no-config", "--python", reference, str(venv)],
            cwd=root,
            env=env,
            timeout=600,
            role="pip_environment",
        ),
        _run(
            [
                "uv",
                "pip",
                "install",
                "--no-config",
                "--python",
                str(python),
                str(wheel),
            ],
            cwd=root,
            env=env,
            timeout=1800,
            role="pip_install",
        ),
    ]
    project = root / "project"
    project.mkdir()
    source = project / "release_consumer.py"
    source.write_bytes(consumer_guest_source(minor).encode("utf-8"))
    diagnostics = project / "diagnostics.json"
    output = project / ("release_consumer" + (".exe" if os.name == "nt" else ""))
    script = venv / ("Scripts/molt.exe" if os.name == "nt" else "bin/molt")
    commands.append(
        _run(
            consumer_guest_command(
                [str(script)],
                target="native",
                profile="release",
                python_minor=minor,
                diagnostics=str(diagnostics),
                output=str(output),
                source=str(source),
            ),
            cwd=root,
            env=env,
            timeout=2700,
            role="pip_build_native_release",
        )
    )
    _audit_guest(output, target=target, wheel_tag=wheel_tag, evidence_dir=evidence_dir)
    identity = executable_content_identity(output, label="pip consumer executable")
    commands.append(
        _run(
            [str(output), *CONSUMER_GUEST_ARGV],
            cwd=project,
            env=env,
            timeout=60,
            role="pip_run_native_release",
            expected_stdout=CONSUMER_EXPECTED_STDOUT,
        )
    )
    observed = loads_exact(diagnostics.read_text(encoding="utf-8"))
    selected = observed.get("compiler") if isinstance(observed, dict) else None
    if (
        not isinstance(selected, dict)
        or selected.get("sha256") != compiler.record["sha256"]
        or venv.resolve() not in Path(str(selected.get("path", ""))).resolve().parents
    ):
        raise ValueError("pip consumer did not use the wheel's packaged compiler")
    _require_no_cargo_runtime(root / "molt-home")
    commands.append(
        _run(
            ["uv", "pip", "uninstall", "--python", str(python), "molt"],
            cwd=root,
            env=env,
            timeout=600,
            role="pip_uninstall",
        )
    )
    _absent_probe(python, env=env, cwd=root)
    return {
        "wheel": {key: wheel_record[key] for key in ("filename", "sha256", "size")},
        "python": minor,
        "reference_python": reference,
        "commands": commands,
        "artifact": {
            "path": str(output),
            "sha256": identity["sha256"],
            "size": identity["size"],
        },
        "compiler_sha256": compiler.record["sha256"],
    }


def _verify_python_coordinate(
    *,
    root: Path,
    bundle_root: Path,
    worker: Path,
    compiler: InstalledCompiler,
    target: dict[str, object],
    minor: str,
    reference: str,
    wheel_tag: str,
    evidence_dir: Path,
) -> dict[str, Any]:
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
    # Bytes, not text mode: the receipt digest is the canonical program's.
    source.write_bytes(consumer_guest_source(minor).encode("utf-8"))
    runtime_env = env.copy()
    runtime_env.pop("PYTHON", None)
    cells: list[dict[str, object]] = []
    for guest_target, profile in CONSUMER_GUEST_CELLS:
        # One fresh directory per cell keeps outputs and manifests apart and
        # proves every observed artifact came from this cell's command.
        cell_root = project / f"{guest_target}-{profile}"
        cell_root.mkdir()
        diagnostics = cell_root / "diagnostics.json"
        output = cell_root / (
            "release_consumer.wasm"
            if guest_target == "wasm"
            else "release_consumer" + (".exe" if os.name == "nt" else "")
        )
        command = consumer_guest_command(
            launcher,
            target=guest_target,
            profile=profile,
            python_minor=minor,
            diagnostics=str(diagnostics),
            output=str(output),
            source=str(source),
        )
        if guest_target == "native":
            commands.append(
                _run(
                    command,
                    cwd=root,
                    env=env,
                    timeout=2700,
                    role=f"build_native_{profile}",
                )
            )
            if not output.is_file():
                raise RuntimeError(
                    f"Molt did not produce the requested binary: {output}"
                )
            _audit_guest(
                output, target=target, wheel_tag=wheel_tag, evidence_dir=evidence_dir
            )
            identity = executable_content_identity(
                output, label="release consumer guest executable"
            )
            commands.append(
                _run(
                    [str(output), *CONSUMER_GUEST_ARGV],
                    cwd=project,
                    env=runtime_env,
                    timeout=60,
                    role=f"run_native_{profile}",
                    expected_stdout=CONSUMER_EXPECTED_STDOUT,
                )
            )
            artifact = {
                "path": str(output),
                "sha256": identity["sha256"],
                "size": identity["size"],
            }
        else:
            # Public `molt run` owns the one linked build and its Node host.
            commands.append(
                _run(
                    command,
                    cwd=project,
                    env=env,
                    timeout=2700,
                    role=f"run_wasm_{profile}",
                    expected_stdout=CONSUMER_EXPECTED_STDOUT,
                )
            )
            artifact = _linked_wasm_artifact(output)
        compiler_sha256, fingerprint = _diagnosed_compiler(
            diagnostics, compiler=compiler, target=guest_target, profile=profile
        )
        cells.append(
            {
                "target": guest_target,
                "profile": profile,
                "diagnostics": str(diagnostics),
                "output": str(output),
                "compiler_sha256": compiler_sha256,
                "compiler_fingerprint": fingerprint,
                "artifact": artifact,
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
    _require_no_cargo_runtime(root / "molt-home")
    return {
        "python": minor,
        "reference_python": reference,
        "execution": execution,
        "source": str(source),
        "source_sha256": sha256_file(source),
        "commands": commands,
        "cells": cells,
    }


def _uninstall_and_replay_native(
    *,
    root: Path,
    bundle_root: Path,
    worker_root: Path,
    coordinates: tuple[tuple[str, str], ...],
    proofs: list[dict[str, Any]],
) -> None:
    """Remove every installed owner before replaying the exact native products."""
    durable_remove_path(bundle_root, retirement_scope="consumer-uninstall")
    durable_remove_path(worker_root, retirement_scope="consumer-uninstall")
    for minor, _ in coordinates:
        coordinate_root = root / f"python-{minor}"
        home = coordinate_root / "molt-home"
        durable_remove_path(home, retirement_scope="consumer-uninstall")
        if home.exists():
            raise RuntimeError("Molt's private environment remained after uninstall")
        _absent_probe(
            _venv_python(coordinate_root / "venv"),
            env=_consumer_environment(coordinate_root),
            cwd=coordinate_root,
        )
    if bundle_root.exists() or worker_root.exists():
        raise RuntimeError(
            "Molt or worker remained installed after portable bundle removal"
        )
    for (minor, _), proof in zip(coordinates, proofs, strict=True):
        coordinate_root = root / f"python-{minor}"
        standalone_env = _consumer_environment(coordinate_root)
        for cell in proof["cells"]:
            if cell["target"] != "native":
                continue
            executable = Path(cell["output"])
            identity = executable_content_identity(
                executable, label="release consumer standalone executable"
            )
            if (identity["sha256"], identity["size"]) != (
                cell["artifact"]["sha256"],
                cell["artifact"]["size"],
            ):
                raise RuntimeError(
                    f"{minor}/native/{cell['profile']}: guest executable "
                    "changed before its standalone run"
                )
            proof["commands"].append(
                _run(
                    [str(executable), *CONSUMER_GUEST_ARGV],
                    cwd=coordinate_root / "project",
                    env=standalone_env,
                    timeout=60,
                    role=f"standalone_native_{cell['profile']}",
                    expected_stdout=CONSUMER_EXPECTED_STDOUT,
                )
            )


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
        worker_identity = stable_regular_file_content_identity(
            worker, label="installed release worker"
        )
        if any(
            worker_identity[key]
            != candidate["native_build"]["artifacts"]["worker"][key]
            for key in ("sha256", "size")
        ):
            raise ValueError("Bundle worker identity differs from native build receipt")
        if (bundle_root / "bin" / worker_name).exists():
            raise ValueError(
                "compiler bundle must not duplicate standalone worker ownership"
            )

        compiler = installed_compiler(bundle_root / "source")
        if compiler is None or compiler.source_sha != candidate["source_sha"]:
            raise ValueError("Bundle compiler source differs from candidate")
        if (
            canonical_json_sha256(list(compiler.files))
            != candidate["native_build"]["source"]["files_sha256"]
        ):
            raise ValueError(
                "Bundle source inventory differs from native build receipt"
            )
        if compiler.record != candidate["compiler"]:
            raise ValueError("Bundle compiler identity differs from candidate")
        if compiler.launcher != candidate["launcher"]:
            raise ValueError("Bundle launcher identity differs from candidate")
        compiler.verify_launcher()
        compiler.verify_sources()
        if (
            rust_channel((compiler.source_root / "rust-toolchain.toml").read_bytes())
            != candidate["native_build"]["policy"]["rust_channel"]
        ):
            raise ValueError("Bundle Rust channel differs from native build receipt")
        compiler.verify_binary(("native-backend", "wasm-backend"), "release")
        compiler.verify_runtime()
        if compiler.runtime != candidate["runtime"]:
            raise ValueError("Bundle runtime cells differ from candidate")
        if inventory_cell_keys(compiler.runtime) != declared_cell_keys():
            raise ValueError(
                "Bundle runtime cells differ from the derived release policy"
            )
        wheel_records = [
            record for record in artifacts if record.get("kind") == "wheel"
        ]
        if len(wheel_records) != 1:
            raise ValueError("candidate must contain exactly one platform wheel")
        evidence_dir = receipt.parent / "wheel-compatibility-evidence"
        wheel_tag = _audit_platform_wheel(
            candidate_dir / str(wheel_records[0]["filename"]),
            target=candidate["target"],
            evidence_dir=evidence_dir,
        )
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
                wheel_tag=wheel_tag,
                evidence_dir=evidence_dir,
            )
            for minor, reference in coordinates
        ]
        compiler.verify_sources()
        compiler.verify_runtime()
        minor, reference = coordinates[0]
        pip_proof = _verify_pip_distribution(
            root=root / "pip",
            wheel=candidate_dir / str(wheel_records[0]["filename"]),
            wheel_record=wheel_records[0],
            compiler=compiler,
            minor=minor,
            reference=reference,
            target=candidate["target"],
            wheel_tag=wheel_tag,
            evidence_dir=evidence_dir,
        )
        _uninstall_and_replay_native(
            root=root,
            bundle_root=bundle_root,
            worker_root=worker_root,
            coordinates=coordinates,
            proofs=proofs,
        )

        count = len(coordinates) * len(CONSUMER_GUEST_CELLS)
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
            "runtime": compiler.runtime,
            "pip_proof": pip_proof,
            "guest_cells": [list(cell) for cell in CONSUMER_GUEST_CELLS],
            "expected_stdout": CONSUMER_EXPECTED_STDOUT,
            "python_policy_sha256": policy_sha256,
            "python_proofs": proofs,
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
