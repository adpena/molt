#!/usr/bin/env python3
"""Verify one immutable release candidate as a clean external consumer."""

from __future__ import annotations

import argparse
from collections.abc import Mapping
import sys
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import time
from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from molt.cli.link_input_observation import CompilationInputRecord

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.temporary_artifacts import OwnedTemporaryDirectory  # noqa: E402
from molt.process_environment import (  # noqa: E402
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)

from tools.cross_run import SealedExecutionProjection  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

from tools.release import consumer_replay  # noqa: E402
from tools.release.archive import extract_zip_strict  # noqa: E402
from tools.release.build_bundle import RELEASE_BUNDLE_ARCHIVE_POLICY  # noqa: E402
from tools.release.release_authority import (  # noqa: E402
    CONSUMER_EXPECTED_STDOUT,
    CONSUMER_GUEST_ARGV,
    consumer_lanes,
    CONSUMER_SCHEMA,
    COMPONENT_CONSUMER_SCHEMA,
    ConsumerComponent,
    validate_component_consumer_proof,
    consumer_guest_command,
    consumer_guest_source,
    consumer_python_policy,
    _load_candidate,
    validate_consumer_proof,
)
from molt.compiler_distribution import InstalledCompiler, installed_compiler  # noqa: E402
from molt.release_lanes import ReleaseLane, capture_release_lanes  # noqa: E402
from molt.source_root import PACKAGED_DISTRIBUTION_PATH  # noqa: E402
from molt.exact_json import canonical_json_sha256, loads_exact  # noqa: E402
from molt.file_publication import durable_remove_path  # noqa: E402
from molt.python_interpreter import probe_python_command  # noqa: E402
from molt.python_environment_identity import (  # noqa: E402
    python_identity_verifier_arguments,
    PythonIdentityVerifier,
)
from molt.rust_toolchain import rust_channel  # noqa: E402
from molt.toolchain_identity import (  # noqa: E402
    executable_candidates,
    executable_content_identity,
    stable_regular_file_content_identity,
)
from molt.verified_subset import current_host_coordinate, host_coordinate  # noqa: E402
from molt.wasm_artifact import (  # noqa: E402
    wasm_runtime_manifest_entry_path,
    wasm_runtime_manifest_path,
)
from tools.release.release_model import sha256_file, write_json  # noqa: E402
from tools.release.binary_compatibility import (  # noqa: E402
    WheelCompatibilityError,
    audit_linked_executable,
    audit_wheel,
)
from tools.release.runtime_cells import declared_cell_keys, inventory_cell_keys  # noqa: E402

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
    env: ProcessEnvironment,
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
    env: ProcessEnvironment,
    cwd: Path,
    target: dict[str, object],
    reference_python: str,
) -> dict[str, object]:
    with PythonIdentityVerifier() as verifier:
        command = [str(python), *python_identity_verifier_arguments(verifier.prefix)]
        interpreter = probe_python_command(
            (str(python),), verifier=verifier, env=env, cwd=cwd
        )
        result = verifier.run(
            [*command, "-c", _HOST_PROBE],
            memory_guard_prefix=_COMMANDS.prefix,
            guard_source_root=_COMMANDS.repo_root,
            env=env,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
            encoding="utf-8",
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


def _absent_probe(python: Path, *, env: ProcessEnvironment, cwd: Path) -> None:
    with PythonIdentityVerifier() as verifier:
        verifier.run(
            [
                str(python),
                *python_identity_verifier_arguments(verifier.prefix),
                "-c",
                "import importlib.util; assert importlib.util.find_spec('molt') is None",
            ],
            cwd=cwd,
            memory_guard_prefix=_COMMANDS.prefix,
            guard_source_root=_COMMANDS.repo_root,
            env=env,
            capture_output=True,
            timeout=30,
            check=True,
        )


def _consumer_environment(root: Path) -> ProcessEnvironment:
    env = ProcessEnvironmentBuilder(os.environ)
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
    lane: ReleaseLane,
) -> tuple[str, str]:
    """Read this product's complete selected lane and production compiler."""
    if not diagnostics.is_file():
        raise RuntimeError(f"{lane.id}: the cell build wrote no diagnostics")
    observed = loads_exact(diagnostics.read_text(encoding="utf-8"))
    selected = observed.get("compiler") if isinstance(observed, dict) else None
    program = observed.get("program") if isinstance(observed, dict) else None
    if (
        not isinstance(selected, dict)
        or not isinstance(program, dict)
        or selected.get("sha256") != compiler.record["sha256"]
        or selected.get("cargo_profile") != lane.compiler_profile
        or Path(str(selected.get("path", ""))).resolve() != compiler.binary.resolve()
        or program != lane.as_record()
    ):
        raise ValueError(
            f"{lane.id}: guest cell changed production compiler or selected lane"
        )
    compiler.verify_binary(
        lane.compiler_features,
        lane.compiler_profile,
    )
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
    env: Mapping[str, str], *, root: Path
) -> ProcessEnvironment:
    """Consumer environment in which no Rust toolchain can be resolved.

    Installed Molt must compile guests with Cargo, rustc and rustup absent: Rust
    selectors are removed, Cargo/rustup homes point at empty private roots, and
    every PATH directory providing a Rust command is excluded through the shared
    executable-search authority. A Rust tool that shares a directory with the
    consumer's own tools fails closed instead of weakening the proof.
    """
    result = ProcessEnvironmentBuilder(env)
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
    result["PATH"] = os.pathsep.join(
        entry
        for entry in result.get("PATH", "").split(os.pathsep)
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
    return ProcessEnvironment(result)


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


def _compilation_captures(
    *,
    interpreters: tuple[Path, ...],
    tools: tuple[Path, ...],
    compiler: InstalledCompiler,
    cwd: Path,
    env: ProcessEnvironment,
    supervisor: Path,
    cas_root: Path,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    """Use canonical Python/toolchain producers; these select immutable inputs.

    Actual consumption is established only by the subsequent sealed run. Full
    captures are admission-only and shared across this coordinate's lane set.
    """
    from molt.python_capture import validate_python_capture
    from tools.proof_queue_pkg import (
        command_identity,
        command_admission,
        execution_environment,
        toolchain_capture,
    )

    python_captures = []
    for interpreter in dict.fromkeys(interpreters):
        with PythonIdentityVerifier() as verifier:
            python_captures.append(
                validate_python_capture(
                    command_identity._parse_json_output(
                        command_identity._run_python_captured(
                            verifier,
                            command_identity._python_probe_command(
                                interpreter,
                                verifier_bytecode_prefix=verifier.prefix,
                                source_root=compiler.source_root,
                                admit_published_environment=False,
                            ),
                            cwd=cwd,
                            env=env,
                        ),
                        purpose="installed compilation frontend capture",
                    )
                )
            )
    captured: dict[str, Any] = {}
    for tool in dict.fromkeys(tools):
        exact = [str(tool), "--version"]
        envelope = command_admission.envelope_for_command(exact)
        _, located, _, updates = execution_environment._locate_toolchain_watch_roots(
            envelope, exact, cwd=cwd, env=env, supervisor_binary=supervisor
        )
        if updates:
            raise ValueError(
                "installed compilation tool capture changed the selected environment"
            )
        _, identities = execution_environment._capture_toolchains(
            envelope,
            exact,
            cwd=cwd,
            env=env,
            source_root=compiler.source_root,
            hash_workers=1,
            located_toolchains=located,
        )
        for name, identity in identities.items():
            if name in captured and captured[name] != identity:
                raise ValueError(
                    "installed compilation lanes selected different toolchain generations"
                )
            captured[name] = identity
    _, reference, _ = toolchain_capture.publish_capture(cas_root, captured)
    return python_captures, reference


def _installed_frontend_python(
    *,
    compiler: InstalledCompiler,
    python: Path,
    env: ProcessEnvironment,
    cwd: Path,
    portable_bundle: bool,
) -> Path:
    if not portable_bundle:
        return python
    # Ask the existing bootstrap identity owner; never guess its environment
    # directory or run setup again. Its native launcher selects this interpreter.
    code = (
        "import json,runpy,sys; from pathlib import Path; "
        "root=Path(sys.argv[1]); "
        "m=runpy.run_path(str(root/'source'/'packaging'/'bootstrap.py')); "
        "home,identity=m['_release_inputs'](root); "
        "print(json.dumps(str(home/'environments'/identity/'bin'/'python')))"
    )
    with PythonIdentityVerifier() as verifier:
        result = verifier.run(
            [
                str(python),
                *python_identity_verifier_arguments(verifier.prefix),
                "-c",
                code,
                str(compiler.source_root.parent),
            ],
            cwd=cwd,
            memory_guard_prefix=_COMMANDS.prefix,
            guard_source_root=_COMMANDS.repo_root,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
            check=True,
        )
    value = loads_exact(result.stdout)
    if (
        not isinstance(value, str)
        or not Path(value).is_absolute()
        or not Path(value).is_file()
    ):
        raise ValueError("installed bootstrap did not identify its prepared frontend")
    return Path(value)


def _preparation_link_inputs(
    output: Path, *, target: str
) -> tuple[CompilationInputRecord, dict[str, Path]]:
    from molt.artifact_publication import publication_receipt_path
    from molt.cli.link_fingerprints import _read_link_fingerprint, _link_outputs_match
    from molt.cli.link_input_observation import validate_compilation_input_observation
    from molt.exact_json import read_exact

    primary = (
        output
        if target == "native"
        else wasm_runtime_manifest_entry_path(wasm_runtime_manifest_path(output))
    )
    path = publication_receipt_path(primary)
    receipt = _read_link_fingerprint(path)
    if receipt is None:
        raise ValueError("installed preparation omitted its final-link publication")
    outputs = {role: Path(row["path"]) for role, row in receipt["outputs"].items()}
    if not _link_outputs_match(
        outputs=outputs, fingerprint=receipt["fingerprint"], receipt_path=path
    ):
        raise ValueError("installed preparation final-link family changed")
    if "input_observation" not in outputs:
        raise ValueError("installed preparation omitted compiler/linker observations")
    observation = validate_compilation_input_observation(
        read_exact(
            outputs["input_observation"],
            max_bytes=16 * 1024 * 1024,
            label="installed preparation compiler/linker observation",
        )
    )
    return observation, outputs


def _verify_guest_cells(
    *,
    root: Path,
    project: Path,
    source: Path,
    launcher: list[str],
    env: ProcessEnvironment,
    compiler: InstalledCompiler,
    minor: str,
    target: dict[str, object],
    wheel_tag: str,
    evidence_dir: Path,
    commands: list[dict[str, Any]],
    role_prefix: str,
    python: Path,
    portable_bundle: bool,
    compilation_evidence: Path,
    archive_cache: Path,
    supervisor: Path,
    generation: dict[str, Any],
    projection: SealedExecutionProjection,
    selected_lanes: tuple[ReleaseLane, ...] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Prepare inputs, then admit actual installed products before uninstall.

    Preparation is not a used-byte claim. Only the immutable-root compilation's
    exported products and closed native journal can enter standalone replay.
    """
    from molt.cli.link_input_observation import (
        LinkInputObservation,
        validate_native_link_preparation,
    )
    from molt.exact_json import read_exact
    from molt.toolchain_identity import find_executable
    from molt.wasm_optimizer_identity import find_wasm_opt

    lanes = consumer_lanes(selected_lanes)
    selected = []
    native_inputs = {}
    tools: list[Path] = []
    for lane in lanes:
        cell_root = project / lane.id
        cell_root.mkdir()
        diagnostics = cell_root / "diagnostics.json"
        output = cell_root / ("program.wasm" if lane.target == "wasm" else "program")
        command = consumer_guest_command(
            launcher,
            lane=lane,
            python_minor=minor,
            diagnostics=str(diagnostics),
            output=str(output),
            source=str(source),
            capture_link_inputs=True,
        )
        # The preparatory command discovers the frontend-selected runtime tier,
        # linker buffers and C dependencies. Its timings/products are not proof.
        _run(
            command,
            cwd=root,
            env=env.updated(lane.environment()),
            timeout=2700,
            role=f"prepare_{lane.id}",
            expected_stdout=CONSUMER_EXPECTED_STDOUT if lane.target == "wasm" else None,
        )
        _diagnosed_compiler(diagnostics, compiler=compiler, lane=lane)
        kind, key = consumer_replay.compilation_runtime_selection(
            diagnostics, compiler=compiler, lane=lane
        )
        selected.append((lane, kind, key))
        observation, outputs = _preparation_link_inputs(output, target=lane.target)
        link = observation["link"]
        records = link["links"].values() if lane.target == "wasm" else (link,)
        tools.extend(
            Path(LinkInputObservation.from_record(row).linker_path) for row in records
        )
        if lane.target == "native":
            if (
                not {"driver_preparation", "driver_dependencies", "input_reproducer"}
                <= outputs.keys()
            ):
                raise ValueError(
                    "installed native preparation omitted C/linker input custody"
                )
            preparation = validate_native_link_preparation(
                read_exact(
                    outputs["driver_preparation"],
                    max_bytes=16 * 1024 * 1024,
                    label="installed native driver preparation",
                )
            )
            native_inputs[lane.id] = (outputs["input_reproducer"], link, preparation)
            tools.append(Path(preparation["driver"]["path"]))
    if portable_bundle:
        uv = find_executable("uv", environment=env)
        if uv is None:
            raise ValueError("installed bundle has no selected uv launcher")
        tools.append(uv)
    if any(lane.target == "wasm" for lane in lanes):
        node = find_executable("node", environment=env)
        optimizer = find_wasm_opt(environment=env, source_root=compiler.source_root)
        if node is None or optimizer is None:
            raise ValueError(
                "installed WASM compilation has no selected Node/optimizer"
            )
        tools.extend((node, Path(optimizer)))
    frontend = _installed_frontend_python(
        compiler=compiler,
        python=python,
        env=env,
        cwd=root,
        portable_bundle=portable_bundle,
    )
    captures, tool_capture = _compilation_captures(
        interpreters=(python, frontend),
        tools=tuple(tools),
        compiler=compiler,
        cwd=root,
        env=env,
        supervisor=supervisor,
        cas_root=root / "capture-cas",
    )
    # The cold export is an actual closed compilation generation. A distinct
    # warm root imports its retained cache and raw original supervisor sessions;
    # Command durations exclude cache import/export and evidence verification;
    # neither these observations nor preparation timings grant performance admission.
    preparation = dict(
        compiler=compiler,
        launcher=launcher,
        python_executable=python,
        python_captures=captures,
        toolchain_capture=tool_capture,
        cas_root=root / "capture-cas",
        native_link_inputs=native_inputs,
        environment=dict(env),
        cells=selected,
        python_minor=minor,
        projection=projection,
        archive_cache=archive_cache,
        supervisor=supervisor,
        generation=generation,
    )
    cold_evidence = compilation_evidence.with_name(compilation_evidence.name + "-cold")
    compilation = None
    for destination, reuse in (
        (cold_evidence, None),
        (compilation_evidence, cold_evidence),
    ):
        plan = consumer_replay.prepare_compilation(
            evidence=destination,
            reuse_compilation=reuse,
            **preparation,
        )
        compilation = consumer_replay.execute_compilation(
            evidence=destination,
            compilation=plan,
            supervisor=supervisor,
            compiler=compiler,
            lanes=lanes,
            python_minor=minor,
            launcher=launcher,
            controller_python=python,
        )
        if reuse is None:
            for lane, row in zip(lanes, compilation["cells"], strict=True):
                cold_output = cold_evidence / "outputs" / lane.id
                result = row["outputs"]["command"]
                commands.append(
                    {
                        "role": f"{role_prefix}cold_{'build' if lane.target == 'native' else 'run'}_{lane.id}",
                        "argv": result["argv"],
                        "returncode": result["returncode"],
                        "duration_seconds": result["duration_sec"],
                        "stdout_sha256": sha256_file(cold_output / "command.stdout"),
                        "stderr_sha256": sha256_file(cold_output / "command.stderr"),
                    }
                )
    assert compilation is not None
    cells = []
    for lane, row in zip(lanes, compilation["cells"], strict=True):
        cell_root = compilation_evidence / "outputs" / lane.id
        output = cell_root / ("program.wasm" if lane.target == "wasm" else "program")
        diagnostics = cell_root / "diagnostics.json"
        result = row["outputs"]["command"]
        commands.append(
            {
                "role": f"{role_prefix}{'build' if lane.target == 'native' else 'run'}_{lane.id}",
                "argv": result["argv"],
                "returncode": result["returncode"],
                "duration_seconds": result["duration_sec"],
                "stdout_sha256": sha256_file(cell_root / "command.stdout"),
                "stderr_sha256": sha256_file(cell_root / "command.stderr"),
            }
        )
        if lane.target == "native":
            _audit_guest(
                output, target=target, wheel_tag=wheel_tag, evidence_dir=evidence_dir
            )
            commands.append(
                _run(
                    [str(output), *CONSUMER_GUEST_ARGV],
                    cwd=project,
                    env=env.without(("PYTHON",)),
                    timeout=60,
                    role=f"{role_prefix}run_{lane.id}",
                    expected_stdout=CONSUMER_EXPECTED_STDOUT,
                )
            )
        elif (
            cell_root / "command.stdout"
        ).read_bytes() != CONSUMER_EXPECTED_STDOUT.encode():
            raise ValueError("sealed WASM compilation returned another guest result")
        # Diagnostics contain the namespace compiler path. The journal/receiver
        # already joined that exact image to this installed compiler generation.
        compiler_sha256, fingerprint = _diagnosed_compiler(
            diagnostics, compiler=compiler, lane=lane
        )
        artifact = {
            key: row["outputs"]["artifact"][key] for key in ("path", "sha256", "size")
        }
        artifact["path"] = str(cell_root / artifact["path"])
        cells.append(
            {
                "lane": lane.as_record(),
                "diagnostics": str(diagnostics),
                "output": str(output),
                "compiler_sha256": compiler_sha256,
                "compiler_fingerprint": fingerprint,
                "artifact": artifact,
                "manifest": (
                    {
                        "path": str(cell_root / "manifest.json"),
                        **stable_regular_file_content_identity(
                            cell_root / "manifest.json", label="compiled WASM manifest"
                        ),
                    }
                    if lane.target == "wasm"
                    else None
                ),
            }
        )
    pointer = {
        "path": "compilation-evidence/"
        + compilation_evidence.name
        + "/compilation.json",
        **consumer_replay.execution_root.file_identity(
            compilation_evidence / "compilation.json"
        ),
    }
    return cells, pointer


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
    compilation_evidence: Path,
    archive_cache: Path,
    supervisor: Path,
    generation: dict[str, Any],
    projection: SealedExecutionProjection,
    component: ConsumerComponent | None = None,
) -> dict[str, Any]:
    """Install one platform wheel and produce the complete logical lane set."""
    root.mkdir(parents=True)
    env = _consumer_environment(root)
    if component is not None:
        env = env.updated({"MOLT_STDLIB_PROFILE": component.stdlib_profile})
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
    pip_compiler = installed_compiler(
        venv.joinpath(*PACKAGED_DISTRIBUTION_PATH) / "source"
    )
    if (
        pip_compiler is None
        or pip_compiler.record != compiler.record
        or pip_compiler.wasm_facts != compiler.wasm_facts
        or pip_compiler.runtime != compiler.runtime
    ):
        raise ValueError("pip consumer did not install the candidate compiler/runtime")
    pip_compiler.verify_sources()
    pip_compiler.verify_wasm_facts()
    pip_compiler.verify_runtime()
    script = venv / ("Scripts/molt.exe" if os.name == "nt" else "bin/molt")
    execution = _capture_python_execution(
        python, env=env, cwd=root, target=target, reference_python=reference
    )
    cells, compilation = _verify_guest_cells(
        root=root,
        project=project,
        source=source,
        launcher=[str(script)],
        env=env,
        compiler=pip_compiler,
        minor=minor,
        target=target,
        wheel_tag=wheel_tag,
        evidence_dir=evidence_dir,
        commands=commands,
        role_prefix="pip_",
        python=python,
        portable_bundle=False,
        compilation_evidence=compilation_evidence,
        archive_cache=archive_cache,
        supervisor=supervisor,
        generation=generation,
        projection=projection,
        selected_lanes=None if component is None else component.lanes,
    )
    if (
        _capture_python_execution(
            python, env=env, cwd=root, target=target, reference_python=reference
        )
        != execution
    ):
        raise ValueError("release consumer pip interpreter changed during compilation")
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
        "execution": execution,
        "commands": commands,
        "source": str(source),
        "source_sha256": sha256_file(source),
        "cells": cells,
        "compilation": compilation,
        "compiler_sha256": compiler.record["sha256"],
        "wasm_facts_sha256": compiler.wasm_facts["sha256"],
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
    compilation_evidence: Path,
    archive_cache: Path,
    supervisor: Path,
    generation: dict[str, Any],
    projection: SealedExecutionProjection,
    component: ConsumerComponent | None = None,
) -> dict[str, Any]:
    project = root / "project"
    project.mkdir(parents=True)
    env = _consumer_environment(root)
    if component is not None:
        env = env.updated({"MOLT_STDLIB_PROFILE": component.stdlib_profile})
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
    env = env.updated({"PYTHON": str(python)})
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
    cells, compilation = _verify_guest_cells(
        root=root,
        project=project,
        source=source,
        launcher=launcher,
        env=env,
        compiler=compiler,
        minor=minor,
        target=target,
        wheel_tag=wheel_tag,
        evidence_dir=evidence_dir,
        commands=commands,
        role_prefix="",
        python=python,
        portable_bundle=True,
        compilation_evidence=compilation_evidence,
        archive_cache=archive_cache,
        supervisor=supervisor,
        generation=generation,
        projection=projection,
        selected_lanes=None if component is None else component.lanes,
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
        "compilation": compilation,
    }


def _uninstall_and_replay(
    *,
    root: Path,
    bundle_root: Path,
    worker_root: Path,
    coordinates: tuple[tuple[str, str], ...],
    proofs: list[dict[str, Any]],
    evidence: Path,
    replay: dict[str, Any],
    supervisor: Path,
) -> dict[str, object]:
    """Remove installed owners, then execute exact products in the sealed root."""
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
    for proof in proofs:
        for cell in proof["cells"]:
            identity = stable_regular_file_content_identity(
                Path(cell["artifact"]["path"]), label="release standalone artifact"
            )
            if (identity["sha256"], identity["size"]) != (
                cell["artifact"]["sha256"],
                cell["artifact"]["size"],
            ):
                raise RuntimeError("guest artifact changed before its standalone run")
    return consumer_replay.execute(
        evidence=evidence,
        replay=replay,
        supervisor=supervisor,
        expected_stdout=CONSUMER_EXPECTED_STDOUT,
    )


def verify(
    candidate_dir: Path,
    receipt: Path,
    *,
    compilation_projection: SealedExecutionProjection,
    archive_cache: Path | None = None,
    component: ConsumerComponent | None = None,
) -> dict[str, object]:
    if component is not None and not isinstance(component, ConsumerComponent):
        raise ValueError("component consumer requires an explicit declared selection")
    receipt_name = (
        "consumer-verification.json"
        if component is None
        else "consumer-component-verification.json"
    )
    if receipt != candidate_dir / receipt_name:
        raise ValueError("consumer receipt must be beside its candidate")
    candidate_path = candidate_dir / "candidate.json"
    candidate = _load_candidate(candidate_path)
    if (
        receipt.exists()
        or (receipt.parent / "consumer-evidence").exists()
        or (receipt.parent / "compilation-evidence").exists()
        or (candidate_dir / consumer_replay.archive_filename(candidate)).exists()
    ):
        raise ValueError(
            "consumer evidence destination already exists; use a fresh candidate output"
        )
    if current_host_coordinate() != (
        candidate["target"]["platform"],
        candidate["target"]["arch"],
    ):
        raise ValueError("release consumer must run on its candidate host")
    all_coordinates, policy_sha256 = consumer_python_policy()
    coordinates = all_coordinates
    if component is not None:
        coordinates = tuple(
            row for row in coordinates if row[0] == component.python_minor
        )
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

    with OwnedTemporaryDirectory(prefix="molt release consumer ") as temporary:
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
        if compiler.wasm_facts != candidate["wasm_facts"]:
            raise ValueError("Bundle WASM facts identity differs from candidate")
        compiler.verify_launcher()
        compiler.verify_sources()
        compiler.verify_wasm_facts()
        if (
            rust_channel((compiler.source_root / "rust-toolchain.toml").read_bytes())
            != candidate["native_build"]["policy"]["rust_channel"]
        ):
            raise ValueError("Bundle Rust channel differs from native build receipt")
        from tools.release.native_build import llvm_policy_sha256

        if (
            llvm_policy_sha256(compiler.files)
            != candidate["native_build"]["llvm"]["policy_sha256"]
        ):
            raise ValueError("Bundle LLVM policy differs from native build receipt")
        lanes = consumer_lanes(None if component is None else component.lanes)
        for profile in sorted({lane.compiler_profile for lane in lanes}):
            features = {
                feature
                for lane in lanes
                if lane.compiler_profile == profile
                for feature in lane.compiler_features
            }
            compiler.verify_binary(tuple(sorted(features)), profile)
        compiler.verify_runtime()
        if compiler.runtime != candidate["runtime"]:
            raise ValueError("Bundle runtime cells differ from candidate")
        actual_keys = inventory_cell_keys(compiler.runtime)
        declared_keys = declared_cell_keys(compiler.source_root)
        if (component is None and actual_keys != declared_keys) or (
            component is not None and not set(actual_keys).issubset(declared_keys)
        ):
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
            all_coordinates,
            policy_sha256,
        ):
            raise ValueError("Bundle Python policy differs from release authority")
        if capture_release_lanes(compiler.source_root).lanes != consumer_lanes():
            raise ValueError("Bundle release lanes differ from release authority")
        if archive_cache is None:
            raise ValueError(
                "standalone replay requires --execution-archive-cache with the pinned OS/Node archives; no automatic fetch"
            )
        if candidate["target"]["platform"] != "linux":
            raise ValueError(
                "standalone replay has no filesystem adapter for this target; host replay is forbidden"
            )
        # Source/provenance admission precedes consumer work and any guest launch.
        consumer_replay.execution_root.support_payloads(
            archive_cache, arch=candidate["target"]["arch"]
        )
        supervisor, generation = consumer_replay.provision_verifier()
        if compilation_projection.mode != "compilation":
            raise ValueError(
                "installed consumer requires an explicit compilation resource projection"
            )
        projection = compilation_projection
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
                compilation_evidence=receipt.parent
                / "compilation-evidence"
                / f"bundle-{minor}",
                archive_cache=archive_cache,
                supervisor=supervisor,
                generation=generation,
                projection=projection,
                component=component,
            )
            for minor, reference in coordinates
        ]
        compiler.verify_sources()
        compiler.verify_wasm_facts()
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
            compilation_evidence=receipt.parent
            / "compilation-evidence"
            / f"pip-{minor}",
            archive_cache=archive_cache,
            supervisor=supervisor,
            generation=generation,
            projection=projection,
            component=component,
        )
        standalone_evidence = receipt.parent / "consumer-evidence"
        replay = consumer_replay.prepare(
            evidence=standalone_evidence,
            candidate=candidate,
            proofs=proofs,
            pip_proof=pip_proof,
            bundle_source=bundle_root / "source",
            archive_cache=archive_cache,
            supervisor=supervisor,
            generation=generation,
            argv=CONSUMER_GUEST_ARGV,
            selected_lanes=None if component is None else component.lanes,
        )
        standalone = _uninstall_and_replay(
            root=root,
            bundle_root=bundle_root,
            worker_root=worker_root,
            coordinates=coordinates,
            proofs=proofs,
            evidence=standalone_evidence,
            replay=replay,
            supervisor=supervisor,
        )

        lanes = consumer_lanes(None if component is None else component.lanes)
        count = (len(coordinates) + 1) * len(lanes)
        payload: dict[str, object] = {
            "schema": CONSUMER_SCHEMA
            if component is None
            else COMPONENT_CONSUMER_SCHEMA,
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
            "wasm_facts": compiler.wasm_facts,
            "runtime": compiler.runtime,
            "pip_proof": pip_proof,
            "guest_cells": [lane.as_record() for lane in lanes],
            "expected_stdout": CONSUMER_EXPECTED_STDOUT,
            "python_policy_sha256": policy_sha256,
            "python_proofs": proofs,
            "uninstall_verified": True,
            "standalone": standalone,
        }
        if component is None:
            validate_consumer_proof(
                payload, candidate, evidence_root=receipt.parent, supervisor=supervisor
            )
        else:
            payload["scope"] = component.as_record()
            validate_component_consumer_proof(
                payload,
                candidate,
                component=component,
                evidence_root=receipt.parent,
                supervisor=supervisor,
            )
        write_json(receipt, payload)
        if component is None:
            consumer_replay.publish_archive(
                candidate_dir=candidate_dir, receipt=receipt, candidate=candidate
            )
        return payload


def main() -> None:
    from molt.cli.config_resolution import RUNTIME_STDLIB_PROFILE_TIERS

    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--execution-archive-cache", type=Path, required=True)
    parser.add_argument("--compile-memory-max-bytes", type=int, required=True)
    parser.add_argument("--compile-workspace-max-bytes", type=int, required=True)
    parser.add_argument("--compile-runtime-max-seconds", type=int, required=True)
    parser.add_argument(
        "--component-python",
        choices=[minor for minor, _ in consumer_python_policy()[0]],
    )
    parser.add_argument(
        "--component-lane",
        action="append",
        choices=[lane.id for lane in consumer_lanes()],
    )
    parser.add_argument(
        "--component-stdlib-profile", choices=tuple(RUNTIME_STDLIB_PROFILE_TIERS)
    )
    args = parser.parse_args()
    requested = (
        args.component_python,
        args.component_lane,
        args.component_stdlib_profile,
    )
    component = None
    if any(value is not None for value in requested):
        if not all(value is not None for value in requested):
            parser.error(
                "component evidence requires Python, lane and stdlib selections"
            )
        if len(set(args.component_lane)) != len(args.component_lane):
            parser.error("component evidence cannot repeat a lane")
        component = ConsumerComponent(
            args.component_python,
            tuple(lane for lane in consumer_lanes() if lane.id in args.component_lane),
            args.component_stdlib_profile,
        )
    payload = verify(
        args.candidate,
        args.receipt,
        archive_cache=args.execution_archive_cache,
        compilation_projection=SealedExecutionProjection.compilation(
            memory_max_bytes=args.compile_memory_max_bytes,
            workspace_max_bytes=args.compile_workspace_max_bytes,
            runtime_max_seconds=args.compile_runtime_max_seconds,
        ),
        component=component,
    )
    print(json.dumps(payload, sort_keys=True))


if __name__ == "__main__":
    main()
