"""One sealed post-uninstall replay and retained-evidence contract for logical release lanes."""

from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path, PurePosixPath
import secrets
import tempfile
import zipfile
from typing import Any

from molt.browser_asset_closure import (
    NODE_RUNNER_ENTRY_ASSETS,
    wasm_loader_asset_payloads,
)
from molt.exact_json import (
    canonical_json_sha256,
    capture_exact,
    loads_exact,
    read_exact,
    write_exact,
)
from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
from molt.release_lanes import ReleaseLane, capture_release_lanes
from molt.toolchain_identity import (
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)
from tools.cross_run import (
    SealedRootTransport,
    SEALED_REPLAY_EXECUTION,
    SealedExecutionProjection,
    validate_sealed_systemd_import,
    validate_sealed_systemd_stop,
    validate_sealed_systemd_provider,
    validate_sealed_systemd_configuration,
    validate_sealed_systemd_terminal,
)
from tools.proof_queue_pkg import supervisor_custody, supervisor_generation
from . import execution_root
from .archive import (
    ArchivePolicy,
    extract_zip_strict,
    write_reproducible_zip,
    same_regular_file_bytes,
)
from .release_model import ROOT, validate_artifact_record, write_json

SCHEMA = "molt.release-consumer-replay.v4"
# One admitted source closure already exceeds the generic 4096-member envelope.
# Shared CAS paths can contain two nodes per unique input plus bucket parents;
# the existing root bound is 65536 regular files. ZIP32 is the smaller hard
# transport limit. Byte, ratio and per-file limits remain the generic limits.
CONSUMER_ARCHIVE_POLICY = ArchivePolicy(max_members=zipfile.ZIP_FILECOUNT_LIMIT)


def _evidence_inventory(evidence: Path) -> list[dict[str, Any]]:
    """Completed records retain resources and raw evidence, not materializations."""
    return execution_root.root_inventory(
        evidence, excluded_names=frozenset({"rootfs", "rootfs.tar"})
    )


ENVIRONMENT = {"PATH": "/absent", "HOME": "/absent", "LANG": "C", "LC_ALL": "C"}


def provision_verifier() -> tuple[Path, dict[str, object]]:
    """Development-side tool selection uses the existing Cargo freshness owner."""
    binary, telemetry = supervisor_generation.provision(cwd=ROOT, env=os.environ)
    generation = supervisor_generation.read_generation(telemetry)
    return binary, generation


def expected_runs(
    proofs: list[dict[str, Any]],
    pip_proof: dict[str, Any],
    *,
    selected_lanes: tuple[ReleaseLane, ...] | None = None,
) -> list[dict[str, Any]]:
    lanes = {
        lane.id: lane
        for lane in capture_release_lanes(ROOT).require_selection(selected_lanes)
    }
    rows = []
    for owner, proof in [*(("bundle", proof) for proof in proofs), ("pip", pip_proof)]:
        if proof["python"] not in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS:
            raise ValueError("consumer replay Python coordinate is invalid")
        cells = proof.get("cells")
        if not isinstance(cells, list) or len(cells) != len(lanes):
            raise ValueError("consumer replay omits a required logical product")
        for expected_lane, cell in zip(lanes.values(), cells, strict=True):
            record = cell.get("lane") if isinstance(cell, dict) else None
            if (
                not isinstance(record, dict)
                or set(record)
                != {
                    "backend",
                    "target",
                    "guest_profile",
                    "runtime_profile",
                    "compiler_profile",
                }
                or any(not isinstance(value, str) for value in record.values())
            ):
                raise ValueError("consumer replay lane is invalid")
            lane = ReleaseLane(**record)
            if lanes.get(lane.id) != lane or lane != expected_lane:
                raise ValueError("consumer replay lane is outside release inventory")
            rows.append(
                {
                    "id": f"{owner}-{proof['python']}-{lane.id}",
                    "owner": owner,
                    "python": proof["python"],
                    "lane": lane.as_record(),
                    "artifact": cell["artifact"],
                    "manifest": cell["manifest"],
                }
            )
    if len({row["id"] for row in rows}) != len(rows):
        raise ValueError("consumer replay repeats a logical product")
    return rows


def guest_command(row: dict[str, Any], argv: tuple[str, ...]) -> list[str]:
    prefix = f"/app/{row['id']}"
    if row["lane"]["target"] == "native":
        return [prefix + "/program", *argv]
    return ["/bin/node", "/app/wasm/run_wasm.js", prefix + "/manifest.json", *argv]


def _policy(command: list[str], image_sha256: str, nonce: str) -> dict[str, Any]:
    return {
        "schema": supervisor_custody.SUPERVISOR_POLICY_SCHEMA,
        "nonce": nonce,
        "mode": "leaf",
        "cwd": "/app",
        "command": command,
        "environment": dict(ENVIRONMENT),
        "root_role": "guest",
        "fixed_images": [
            {
                "role": "guest",
                "path": command[0],
                "sha256": image_sha256,
                "root_exit_disposition": "require-exit",
            }
        ],
        "derived_roots": [],
    }


def _equal_artifact(actual: dict[str, Any], expected: dict[str, Any]) -> None:
    if (actual["sha256"], actual["size"]) != (expected["sha256"], expected["size"]):
        raise ValueError("emitted artifact changed before its standalone replay")


def _artifact_json(
    path: Path, expected: dict[str, Any], *, max_bytes: int, label: str
) -> Any:
    identity, payload = capture_exact(path, max_bytes=max_bytes, label=label)
    _equal_artifact({"sha256": identity.sha256, "size": identity.size}, expected)
    return payload


def _artifact_bytes(
    path: Path, expected: dict[str, Any], *, max_bytes: int, label: str
) -> bytes:
    identity, data = capture_stable_regular_file(path, max_bytes=max_bytes, label=label)
    _equal_artifact({"sha256": identity.sha256, "size": identity.size}, expected)
    return data


def _linked_wasm_descriptor(payload: object) -> tuple[str, dict[str, Any]]:
    if (
        not isinstance(payload, dict)
        or payload.get("mode") != "linked"
        or not isinstance(payload.get("modules"), dict)
        or set(payload.get("modules", {})) != {"linked"}
    ):
        raise ValueError(
            "standalone consumer requires the installed command's linked WASM manifest"
        )
    descriptor = payload["modules"]["linked"]
    if not isinstance(descriptor, dict):
        raise ValueError("standalone WASM module descriptor is invalid")
    name = execution_root.relative_path(descriptor.get("path")).as_posix()
    if name == "manifest.json":
        raise ValueError("standalone WASM module collides with its manifest")
    return name, descriptor


def _stage_wasm(
    rootfs: Path,
    prefix: str,
    artifact: dict[str, Any],
    expected_manifest: dict[str, Any],
) -> list[dict[str, Any]]:
    module = Path(artifact["path"])
    manifest = module.parent / "manifest.json"
    actual = execution_root.stage_file(
        rootfs,
        prefix + "manifest.json",
        manifest,
        executable=False,
        max_bytes=min(expected_manifest["size"], 4 * 1024 * 1024),
    )
    _equal_artifact(actual, expected_manifest)
    staged = [actual]
    payload = _artifact_json(
        rootfs / prefix / "manifest.json",
        expected_manifest,
        max_bytes=4 * 1024 * 1024,
        label="standalone WASM manifest",
    )
    name, descriptor = _linked_wasm_descriptor(payload)
    selected = manifest.parent.joinpath(*PurePosixPath(name).parts)
    if selected.resolve(strict=True) != module.resolve(strict=True):
        raise ValueError("standalone WASM manifest selects another artifact")
    _equal_artifact(descriptor, artifact)
    actual = execution_root.stage_file(
        rootfs,
        prefix + name,
        selected,
        executable=False,
        max_bytes=artifact["size"],
    )
    _equal_artifact(actual, artifact)
    return [*staged, actual]


def prepare(
    *,
    evidence: Path,
    candidate: dict[str, Any],
    proofs: list[dict[str, Any]],
    pip_proof: dict[str, Any],
    bundle_source: Path,
    archive_cache: Path,
    supervisor: Path,
    generation: dict[str, Any],
    argv: tuple[str, ...],
    selected_lanes: tuple[ReleaseLane, ...] | None = None,
) -> dict[str, Any]:
    platform, arch = candidate["target"]["platform"], candidate["target"]["arch"]
    if platform != "linux":
        raise ValueError(
            f"sealed standalone replay has no qualified {platform}/{arch} filesystem adapter; host execution is forbidden"
        )
    lanes = capture_release_lanes(ROOT).require_selection(selected_lanes)
    # Fail missing provisionable inputs before uninstall or any guest launch.
    providers = execution_root.archive_inputs(arch=arch)
    evidence.mkdir(parents=True, exist_ok=False)
    rootfs = evidence / "rootfs"
    rootfs.mkdir()
    cache = evidence / "archives"
    cache.mkdir()
    for provider in providers:
        captured = execution_root.stage_file(
            cache,
            provider["filename"],
            archive_cache / provider["filename"],
            executable=False,
            max_bytes=provider["size"],
        )
        _equal_artifact(captured, provider)
    payloads, retained_providers = execution_root.support_payloads(cache, arch=arch)
    if retained_providers != providers:
        raise ValueError("execution-root provider authority changed while staging")
    admitted_files = [
        execution_root.write_payload(rootfs, name, data, executable=True)
        for name, data in payloads.items()
    ]
    supervisor_file = execution_root.stage_file(
        rootfs,
        "bin/molt-proof-supervisor",
        supervisor,
        executable=True,
        max_bytes=generation["binary"]["size_bytes"],
    )
    _equal_artifact(
        supervisor_file,
        {
            "sha256": generation["binary"]["sha256"],
            "size": generation["binary"]["size_bytes"],
        },
    )
    admitted_files.append(supervisor_file)
    write_json(evidence / "supervisor-generation.json", generation)
    assets = wasm_loader_asset_payloads(
        bundle_source / "wasm", NODE_RUNNER_ENTRY_ASSETS
    )
    for name, data in assets.items():
        admitted_files.append(
            execution_root.write_payload(
                rootfs,
                "app/wasm/" + name,
                data,
                executable=False,
            )
        )
    rows = expected_runs(proofs, pip_proof, selected_lanes=lanes)
    executable_paths = ["bin/node", "bin/molt-proof-supervisor"]
    for row in rows:
        prefix = f"app/{row['id']}/"
        if row["lane"]["target"] == "native":
            actual = execution_root.stage_file(
                rootfs,
                prefix + "program",
                Path(row["artifact"]["path"]),
                executable=True,
                max_bytes=row["artifact"]["size"],
            )
            _equal_artifact(actual, row["artifact"])
            admitted_files.append(actual)
            executable_paths.append(prefix + "program")
        else:
            admitted_files.extend(
                _stage_wasm(rootfs, prefix, row["artifact"], row["manifest"])
            )
        command = guest_command(row, argv)
        image = execution_root.linux_root_relative_path(command[0]).as_posix()
        nonce = secrets.token_hex(32)
        policy_path = f"policies/{row['id']}.json"
        image_identity = next(item for item in admitted_files if item["path"] == image)
        policy = _policy(command, image_identity["sha256"], nonce)
        admitted_files.append(
            execution_root.write_payload(
                rootfs,
                policy_path,
                supervisor_custody.encode_supervisor_policy(policy),
                executable=False,
            )
        )
        row.update(command=command, policy=policy_path, nonce=nonce)
    dependencies = execution_root.audit_native_closure(
        rootfs,
        arch=arch,
        executable_paths=executable_paths,
        expected_files=admitted_files,
    )
    seal = execution_root.seal_root(
        rootfs, evidence / "rootfs.tar", expected_files=admitted_files
    )
    return {
        "schema": SCHEMA,
        "candidate_sha256": canonical_json_sha256(candidate),
        "source_sha": candidate["source_sha"],
        "target": candidate["target"],
        "providers": providers,
        "root": seal,
        "dependencies": dependencies,
        "runs": rows,
    }


def execute(
    *, evidence: Path, replay: dict[str, Any], supervisor: Path, expected_stdout: str
) -> dict[str, object]:
    """Called only after the installed Molt owners have been removed."""
    if execution_root.root_inventory(evidence / "rootfs") != replay["root"]["files"]:
        raise ValueError("sealed standalone input bytes changed before execution")
    execution_root.validate_sealed_tar(
        evidence / "rootfs.tar",
        evidence / "rootfs",
        expected_archive=replay["root"]["archive"],
    )
    transport = SealedRootTransport()
    try:
        replay["transport"] = transport.import_sealed_root(
            evidence / "rootfs.tar",
            sha256=replay["root"]["archive"]["sha256"],
            arch=replay["target"]["arch"],
            evidence=evidence / "transport",
        )
        for row in replay["runs"]:
            command = [
                "/bin/molt-proof-supervisor",
                "run-export",
                "--policy",
                "/" + row["policy"],
                "--receipt",
                "/evidence/receipt.json",
            ]
            result = transport.run_sealed(command, timeout=60)
            stdout_path, stderr_path = (
                Path(result.pop("stdout_path")),
                Path(result.pop("stderr_path")),
            )
            if (
                capture_stable_regular_file(
                    stdout_path,
                    label="standalone stdout",
                    max_bytes=max(1, len(expected_stdout.encode())),
                )[1]
                != expected_stdout.encode()
            ):
                raise ValueError(
                    "sealed consumer stdout differs from independent reference"
                )
            receipt = evidence / "runs" / row["id"] / "receipt.json"
            stderr = supervisor_custody.decode_supervisor_export(
                stderr_path, receipt_path=receipt
            )
            verified = supervisor_custody._validated_supervisor_receipt(
                binary=supervisor,
                policy_path=evidence / "rootfs" / row["policy"],
                receipt_path=receipt,
                rootfs=evidence / "rootfs",
                cwd=ROOT,
                env=os.environ,
            )
            require_success(verified)
            result.update(
                stdout=stdout_path.relative_to(evidence).as_posix(),
                stderr=stderr_path.relative_to(evidence).as_posix(),
                guest_stderr_sha256=hashlib.sha256(stderr).hexdigest(),
                receipt=receipt.relative_to(evidence).as_posix(),
            )
            row["execution"] = result
    finally:
        primary = sys.exception()
        try:
            transport.remove_sealed_root()
        except BaseException as cleanup:
            if primary is not None:
                raise BaseExceptionGroup(
                    "sealed replay and cleanup failed", [primary, cleanup]
                ) from None
            raise
    if execution_root.root_inventory(evidence / "rootfs") != replay["root"]["files"]:
        raise ValueError("sealed standalone input bytes changed during execution")
    execution_root.retain_root(
        evidence / "rootfs",
        evidence / "rootfs.tar",
        sealed=replay["root"],
        resources=evidence.parent / "execution-inputs",
    )
    replay["files"] = _evidence_inventory(evidence)
    write_exact(evidence / "replay.json", replay, exclusive=True)
    execution_root.retire_root(
        evidence / "rootfs", evidence / "rootfs.tar", sealed=replay["root"]
    )
    return {
        "path": evidence.name + "/replay.json",
        **execution_root.file_identity(evidence / "replay.json"),
    }


def require_success(receipt: dict[str, Any]) -> None:
    if (
        not isinstance(receipt, dict)
        or any(
            type(receipt.get(key)) is not int
            for key in ("root_exit_code", "error_count", "violation_count")
        )
        or not supervisor_custody.supervisor_receipt_is_complete(receipt)
        or receipt.get("root_exit_code") != 0
        or receipt.get("error_count") != 0
        or receipt.get("violation_count") != 0
        or receipt.get("errors") != []
        or receipt.get("violations") != []
        or not isinstance(receipt.get("accounting"), dict)
        or type(receipt["accounting"].get("active_processes")) is not int
        or receipt["accounting"]["active_processes"] != 0
    ):
        raise ValueError("standalone native custody did not close successfully")


def _validate_generation(generation: dict[str, Any], rootfs: Path) -> None:
    if (
        generation.get("kind") != supervisor_generation.GENERATION_SCHEMA
        or generation.get("freshness_authority") != "cargo-build-locked"
    ):
        raise ValueError("standalone supervisor has no canonical Cargo generation")
    inputs = generation.get("inputs")
    if not isinstance(inputs, dict) or generation.get(
        "input_sha256"
    ) != canonical_json_sha256(inputs):
        raise ValueError("standalone supervisor generation input identity is invalid")
    original_root = PurePosixPath(inputs["source_root"])
    original = {row["path"]: row for row in inputs["files"]}
    expected = supervisor_custody.source_authority_paths(ROOT)
    if set(inputs["source_paths"]) != {
        str(original_root / path.relative_to(ROOT).as_posix()) for path in expected
    }:
        raise ValueError("standalone supervisor source closure differs")
    for path in expected:
        row = original[str(original_root / path.relative_to(ROOT).as_posix())]
        actual = execution_root.file_identity(path)
        if (row["sha256"], row["size_bytes"]) != (actual["sha256"], actual["size"]):
            raise ValueError("standalone supervisor source bytes differ")
    actual = execution_root.file_identity(rootfs / "bin/molt-proof-supervisor")
    if (actual["sha256"], actual["size"]) != (
        generation["binary"]["sha256"],
        generation["binary"]["size_bytes"],
    ):
        raise ValueError("standalone supervisor binary differs from generation")


def validate(
    reference: object,
    *,
    evidence_root: Path,
    candidate: dict[str, Any],
    proofs: list[dict[str, Any]],
    pip_proof: dict[str, Any],
    supervisor: Path,
    argv: tuple[str, ...],
    expected_stdout: str,
    selected_lanes: tuple[ReleaseLane, ...] | None = None,
) -> None:
    lanes = capture_release_lanes(ROOT).require_selection(selected_lanes)
    if (
        not isinstance(reference, dict)
        or set(reference) != {"path", "filename", "sha256", "size"}
        or reference["path"] != "consumer-evidence/replay.json"
        or reference["filename"] != "replay.json"
    ):
        raise ValueError("standalone consumer evidence reference is invalid")
    path = evidence_root / reference["path"]
    evidence = path.parent
    replay = _artifact_json(
        path, reference, max_bytes=4 * 1024 * 1024, label="standalone replay"
    )
    expected_keys = {
        "schema",
        "candidate_sha256",
        "source_sha",
        "target",
        "providers",
        "root",
        "dependencies",
        "runs",
        "transport",
        "files",
    }
    if (
        not isinstance(replay, dict)
        or set(replay) != expected_keys
        or replay["schema"] != SCHEMA
        or replay["candidate_sha256"] != canonical_json_sha256(candidate)
        or replay["source_sha"] != candidate["source_sha"]
        or replay["target"] != candidate["target"]
    ):
        raise ValueError("standalone consumer candidate binding differs")
    if candidate["target"]["platform"] != "linux":
        raise ValueError(
            "standalone consumer filesystem adapter is unavailable for target"
        )
    actual_files = [
        row for row in _evidence_inventory(evidence) if row["path"] != "replay.json"
    ]
    if actual_files != replay["files"]:
        raise ValueError("standalone evidence file closure changed")
    retained_files = {row["path"]: row for row in replay["files"]}
    with execution_root.materialized_root(
        evidence / "rootfs",
        evidence / "rootfs.tar",
        sealed=replay["root"],
        resources=evidence_root / "execution-inputs",
    ):
        root_files = {row["path"]: row for row in replay["root"]["files"]}
        rootfs = evidence / "rootfs"
        if (
            execution_root.root_inventory(rootfs) != replay["root"]["files"]
            or canonical_json_sha256(replay["root"]["files"])
            != replay["root"]["files_sha256"]
        ):
            raise ValueError("standalone sealed root changed")
        execution_root.validate_sealed_tar(
            evidence / "rootfs.tar", rootfs, expected_archive=replay["root"]["archive"]
        )
        arch = candidate["target"]["arch"]
        transport = replay["transport"]
        _validate_transport(
            transport,
            sealed_root=replay["root"],
            arch=arch,
            projection=SEALED_REPLAY_EXECUTION,
        )
        support, providers = execution_root.support_payloads(
            evidence / "archives", arch=arch
        )
        if providers != replay["providers"]:
            raise ValueError("standalone OS/Node providers differ from source pins")
        admitted = set(support)
        for name, data in support.items():
            _equal_artifact(
                execution_root.file_identity(rootfs / name),
                {
                    "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                },
            )
        generation = _artifact_json(
            evidence / "supervisor-generation.json",
            retained_files["supervisor-generation.json"],
            max_bytes=16 * 1024 * 1024,
            label="retained supervisor generation",
        )
        _validate_generation(generation, rootfs)
        admitted.add("bin/molt-proof-supervisor")
        for name, data in wasm_loader_asset_payloads(
            ROOT / "wasm", NODE_RUNNER_ENTRY_ASSETS
        ).items():
            selected = "app/wasm/" + name
            if (
                execution_root.file_identity(rootfs / selected)["sha256"]
                != hashlib.sha256(data).hexdigest()
            ):
                raise ValueError("standalone Node runner source closure changed")
            admitted.add(selected)
        expected = expected_runs(proofs, pip_proof, selected_lanes=lanes)
        if not isinstance(replay["runs"], list) or len(replay["runs"]) != len(expected):
            raise ValueError("standalone replay coordinate closure is incomplete")
        nonces: set[str] = set()
        executables = ["bin/node", "bin/molt-proof-supervisor"]
        for wanted, row in zip(expected, replay["runs"], strict=True):
            if (
                not isinstance(row, dict)
                or set(row) != set(wanted) | {"command", "policy", "nonce", "execution"}
                or any(row.get(key) != value for key, value in wanted.items())
            ):
                raise ValueError(
                    "standalone replay coordinate/artifact binding differs"
                )
            command = guest_command(wanted, argv)
            policy = f"policies/{row['id']}.json"
            if (
                row["command"] != command
                or row["policy"] != policy
                or row["nonce"] in nonces
            ):
                raise ValueError("standalone replay command/policy binding differs")
            nonces.add(row["nonce"])
            if wanted["lane"]["target"] == "native":
                name = f"app/{wanted['id']}/program"
                _equal_artifact(
                    execution_root.file_identity(rootfs / name), wanted["artifact"]
                )
                executables.append(name)
                admitted.add(name)
            else:
                prefix = f"app/{wanted['id']}/"
                manifest = _artifact_json(
                    rootfs / prefix / "manifest.json",
                    wanted["manifest"],
                    max_bytes=4 * 1024 * 1024,
                    label="retained WASM manifest",
                )
                relative, descriptor = _linked_wasm_descriptor(manifest)
                name = prefix + relative
                _equal_artifact(execution_root.file_identity(rootfs / name), descriptor)
                _equal_artifact(descriptor, wanted["artifact"])
                admitted.update({prefix + "manifest.json", name})
            image = rootfs / execution_root.linux_root_relative_path(command[0])
            actual_policy = _artifact_json(
                rootfs / policy,
                root_files[policy],
                max_bytes=65536,
                label="standalone policy",
            )
            if actual_policy != _policy(
                command, execution_root.file_identity(image)["sha256"], row["nonce"]
            ):
                raise ValueError(
                    "standalone policy grants another executable or environment"
                )
            admitted.add(policy)
            execution = row["execution"]
            if not isinstance(execution, dict) or set(execution) != {
                "nonce",
                "observation",
                "teardown",
                "stdout",
                "stderr",
                "receipt",
                "guest_stderr_sha256",
                "execution",
            }:
                raise ValueError("standalone execution record fields are invalid")
            if (
                SealedExecutionProjection.from_dict(execution["execution"])
                != SEALED_REPLAY_EXECUTION
            ):
                raise ValueError(
                    "standalone replay cannot use a compilation root projection"
                )
            supervisor_command = [
                "/bin/molt-proof-supervisor",
                "run-export",
                "--policy",
                "/" + policy,
                "--receipt",
                "/evidence/receipt.json",
            ]
            nonce = execution["nonce"]
            observed = validate_sealed_systemd_configuration(
                execution["observation"],
                nonce=nonce,
                root=transport["root"],
                command=supervisor_command,
            )
            validate_sealed_systemd_terminal(
                execution["observation"], invocation=observed["InvocationID"]
            )
            validate_sealed_systemd_stop(
                execution["teardown"],
                unit=observed["Id"],
                invocation=observed["InvocationID"],
            )
            if (
                execution["stdout"] != f"transport/{nonce}.stdout"
                or execution["stderr"] != f"transport/{nonce}.stderr"
            ):
                raise ValueError(
                    "standalone streams belong to another systemd invocation"
                )
            for key in ("stdout", "stderr", "receipt"):
                execution_root.relative_path(execution[key])
            stdout = evidence / execution["stdout"]
            if (
                _artifact_bytes(
                    stdout,
                    retained_files[execution["stdout"]],
                    label="retained standalone stdout",
                    max_bytes=max(1, len(expected_stdout.encode())),
                )
                != expected_stdout.encode()
            ):
                raise ValueError("retained standalone stdout differs from reference")
            receipt = evidence / execution["receipt"]
            if execution["receipt"] != f"runs/{row['id']}/receipt.json":
                raise ValueError("standalone receipt belongs to another coordinate")
            prefix, receipt_bytes, events = supervisor_custody.read_supervisor_export(
                evidence / execution["stderr"],
                expected=retained_files[execution["stderr"]],
            )
            retained_receipt = _artifact_bytes(
                receipt,
                retained_files[execution["receipt"]],
                max_bytes=supervisor_custody.SUPERVISOR_BUDGETS["receipt_bytes"],
                label="exported receipt",
            )
            descriptor = loads_exact(retained_receipt)["event_log"]
            event_path = receipt.with_name(descriptor["file"])
            retained_events = _artifact_bytes(
                event_path,
                retained_files[event_path.relative_to(evidence).as_posix()],
                max_bytes=max(1, len(events)),
                label="exported events",
            )
            if (
                retained_receipt != receipt_bytes
                or retained_events != events
                or hashlib.sha256(prefix).hexdigest()
                != execution["guest_stderr_sha256"]
            ):
                raise ValueError(
                    "standalone captured export differs from retained native evidence"
                )
            verified = supervisor_custody._validated_supervisor_receipt(
                binary=supervisor,
                policy_path=rootfs / policy,
                receipt_path=receipt,
                rootfs=rootfs,
                cwd=ROOT,
                env=os.environ,
            )
            require_success(verified)
        if admitted != {row["path"] for row in replay["root"]["files"]}:
            raise ValueError("standalone root contains unadmitted files or lost inputs")
        if (
            execution_root.audit_native_closure(
                rootfs,
                arch=arch,
                executable_paths=executables,
                expected_files=replay["root"]["files"],
            )
            != replay["dependencies"]
        ):
            raise ValueError("standalone ELF dependency closure differs")


def archive_filename(candidate: dict[str, Any]) -> str:
    target = candidate["target"]
    return f"molt-consumer-evidence-{candidate['version']}-{target['platform']}-{target['arch']}.zip"


def validate_resources(*, evidence_root: Path, consumer: dict[str, Any]) -> None:
    """Require exactly the CAS buffers selected by admitted root descriptions."""
    resources = evidence_root / "execution-inputs"
    paths = [consumer["standalone"]["path"]]
    for proof in [*consumer["python_proofs"], consumer["pip_proof"]]:
        path = execution_root.relative_path(proof["compilation"]["path"])
        paths.extend(
            (
                path.as_posix(),
                (
                    path.parent.with_name(path.parent.name + "-cold") / path.name
                ).as_posix(),
            )
        )
    expected = {}
    nodes: set[str] = set()
    for name in paths:
        owner = evidence_root / execution_root.relative_path(name)
        if (owner.parent / "rootfs").exists() or (owner.parent / "rootfs.tar").exists():
            raise ValueError(
                "consumer evidence still contains an unretired root materialization"
            )
        record = read_exact(
            owner,
            max_bytes=64 * 1024**2,
            label="retained resource owner",
        )
        for resource in execution_root.validate_root_record(record["root"]):
            row = resource.as_record()
            reference = resource.reference(resources)
            relative = Path(reference.path).relative_to(resources).as_posix()
            relative_path = PurePosixPath(relative)
            nodes.update(
                parent.as_posix()
                for parent in (relative_path, *relative_path.parents)
                if parent.as_posix() != "."
            )
            if len(nodes) > CONSUMER_ARCHIVE_POLICY.max_members:
                raise ValueError("consumer resources exceed ZIP32 member-count policy")
            expected[relative] = {
                "path": relative,
                "filename": "payload",
                "sha256": row["sha256"],
                "size": row["size"],
                "mode": 0o444,
            }
    if execution_root.root_inventory(resources) != [
        expected[name] for name in sorted(expected)
    ]:
        raise ValueError(
            "consumer retained resource closure differs from admitted roots"
        )


def publish_archive(
    *, candidate_dir: Path, receipt: Path, candidate: dict[str, Any]
) -> Path:
    """Retain proof inputs with the existing bounded reproducible ZIP owner."""
    if receipt != candidate_dir / "consumer-verification.json":
        raise ValueError("consumer proof must publish beside its candidate")
    validate_resources(
        evidence_root=candidate_dir,
        consumer=read_exact(
            receipt, max_bytes=1024 * 1024, label="consumer publication receipt"
        ),
    )
    output = candidate_dir / archive_filename(candidate)
    write_reproducible_zip(
        candidate_dir,
        output,
        source_date_epoch=candidate["source_date_epoch"],
        policy=CONSUMER_ARCHIVE_POLICY,
        mode_resolver=lambda name: (
            0o444
            if name.is_relative_to("execution-inputs")
            else 0o755
            if (candidate_dir / name).stat().st_mode & 0o111
            else 0o644
        ),
        selected=(
            "candidate.json",
            *(row["filename"] for row in candidate["runtime_builds"]),
            "consumer-verification.json",
            "compilation-evidence",
            "consumer-evidence",
            "execution-inputs",
        ),
    )
    return output


def admit_archive(
    *, candidate_dir: Path, candidate: dict[str, Any], supervisor: Path
) -> dict[str, Any]:
    """Replay the bytes actually shipped, without producer paths or containers."""
    from .release_authority import _load_candidate, validate_consumer_proof

    archive = candidate_dir / archive_filename(candidate)
    with tempfile.TemporaryDirectory(prefix="molt-consumer-admission-") as raw:
        extracted = Path(raw).resolve() / "consumer"
        identity = extract_zip_strict(
            archive, extracted, policy=CONSUMER_ARCHIVE_POLICY
        )
        archived_candidate = _load_candidate(extracted / "candidate.json")
        if archived_candidate != candidate:
            raise ValueError("consumer archive candidate differs from admitted source")
        for name in (
            "candidate.json",
            "consumer-verification.json",
            *(row["filename"] for row in candidate["runtime_builds"]),
        ):
            if not same_regular_file_bytes(extracted / name, candidate_dir / name):
                raise ValueError(
                    "consumer archive differs from admitted candidate/proof"
                )
        if {path.name for path in extracted.iterdir()} != {
            "candidate.json",
            *(row["filename"] for row in candidate["runtime_builds"]),
            "consumer-verification.json",
            "consumer-evidence",
            "compilation-evidence",
            "execution-inputs",
        }:
            raise ValueError("consumer archive root closure differs")
        consumer = read_exact(
            extracted / "consumer-verification.json",
            max_bytes=1024 * 1024,
            label="archived consumer proof",
        )
        validate_consumer_proof(
            consumer, candidate, evidence_root=extracted, supervisor=supervisor
        )
    verify_stable_regular_file_identity(
        identity, label="consumer evidence archive", hash_content=True
    )
    record = {
        "filename": archive.name,
        "kind": "molt-consumer-evidence",
        "sha256": identity.sha256,
        "size": identity.size,
    }
    record.update(
        name="molt-consumer-evidence",
        version=candidate["version"],
        platform=candidate["target"]["platform"],
        arch=candidate["target"]["arch"],
        libc=None,
    )
    return validate_artifact_record(record, version=candidate["version"])


# Bounded, invocation-local projections of the existing retained evidence files.
# No verified flags or process identities are persisted by a compiler/cache.
_MAX_EMISSION_SCOPES = 1024


def _compilation_scope_sources(
    evidence: Path, compilation: dict[str, Any]
) -> dict[str, tuple[Path, Path]]:
    from molt.backend_protocol import validate_emission_scope

    rootfs = evidence / "rootfs"
    sources: dict[str, tuple[Path, Path]] = {}
    cells = {cell["id"]: cell for cell in compilation["cells"]}
    for cell in cells.values():
        policy_path = rootfs / cell["policy"]
        policy = read_exact(
            policy_path, max_bytes=16 * 1024**2, label="compilation scope policy"
        )
        scope = validate_emission_scope(policy["nonce"])
        if scope is None or scope in sources:
            raise ValueError("compilation emission scope is absent or repeated")
        sources[scope] = (policy_path, evidence / "runs" / cell["id"] / "receipt.json")
    origins = compilation["origins"]
    if (
        not isinstance(origins, list)
        or len(origins) + len(sources) > _MAX_EMISSION_SCOPES
    ):
        raise ValueError("retained emission scope inventory exceeds its bound")
    identities = set()
    for row in origins:
        if not isinstance(row, dict) or set(row) != {
            "scope",
            "cell",
            "identity_sha256",
        }:
            raise ValueError("retained emission scope reference is not exact")
        scope = validate_emission_scope(row["scope"])
        identity = validate_emission_scope(row["identity_sha256"])
        if (
            scope is None
            or identity is None
            or scope in sources
            or identity in identities
            or row["cell"] not in cells
        ):
            raise ValueError("retained emission scope is absent, repeated or foreign")
        identities.add(identity)
        directory = evidence / "origins" / identity
        policy_path, receipt_path = (
            directory / "policy.json",
            directory / "receipt.json",
        )
        policy = read_exact(
            policy_path, max_bytes=16 * 1024**2, label="original emission policy"
        )
        cell = cells[row["cell"]]
        expected = read_exact(
            rootfs / cell["policy"],
            max_bytes=16 * 1024**2,
            label="current compilation policy template",
        )
        expected = {**expected, "nonce": scope}
        if policy != expected:
            raise ValueError(
                "original emission policy differs from admitted process/image coordinate"
            )
        config = read_exact(
            directory / "config.json",
            max_bytes=1024**2,
            label="original emission controller configuration",
        )
        expected_config = read_exact(
            rootfs / ("app/compilation-" + cell["id"] + ".json"),
            max_bytes=1024**2,
            label="compilation controller template",
        )
        expected_config = {
            **expected_config,
            "environment": {
                **expected_config["environment"],
                "MOLT_EMISSION_SCOPE": scope,
            },
        }
        if config != expected_config:
            raise ValueError("original emission controller configuration differs")
        receipt = read_exact(
            receipt_path,
            max_bytes=supervisor_custody.SUPERVISOR_BUDGETS["receipt_bytes"],
            label="original emission receipt",
        )
        if receipt.get("identity_sha256") != identity:
            raise ValueError(
                "original emission receipt identity differs from retained reference"
            )
        sources[scope] = (policy_path, receipt_path)
    return sources


def _verified_emission_scope(
    scope: str | None,
    *,
    sources: dict[str, tuple[Path, Path]],
    verified_scopes: dict[str, tuple[dict[str, Any], Any]],
    supervisor: Path,
    rootfs: Path,
) -> tuple[dict[str, Any], Any]:
    if scope is None or scope not in sources:
        raise ValueError(
            "original emission scope has no retained closed supervisor evidence"
        )
    if scope in verified_scopes:
        return verified_scopes[scope]
    policy_path, receipt_path = sources[scope]
    policy = read_exact(
        policy_path, max_bytes=16 * 1024**2, label="original emission scope"
    )
    if policy.get("nonce") != scope:
        raise ValueError("emission scope names another supervisor policy")
    receipt = supervisor_custody._validated_supervisor_receipt(
        binary=supervisor,
        policy_path=policy_path,
        receipt_path=receipt_path,
        rootfs=rootfs,
        cwd=ROOT,
        env=os.environ,
        require_closed=True,
    )
    require_success(receipt)
    if receipt["nonce_sha256"] != hashlib.sha256(scope.encode("ascii")).hexdigest():
        raise ValueError("emission scope differs from its closed receipt")
    events = supervisor_custody._verified_supervisor_event_artifact(
        receipt_path=receipt_path,
        descriptor=receipt["event_log"],
        collect_images=True,
        collect_processes=True,
    )
    verified_scopes[scope] = (receipt, events)
    return receipt, events


def _compilation_emission_hints(captured: dict[str, Any]) -> set[tuple[str, int]]:
    from molt.backend_protocol import BackendEmissionOrigin, BackendNativeWorkerOrigin

    origin = BackendEmissionOrigin.from_payload(captured["compiler"])
    hints = {(origin.producer.scope, origin.producer.process_id)}
    if origin.codegen.process_id is not None:
        hints.add((origin.producer.scope, origin.codegen.process_id))
    hints.update((worker.scope, worker.process_id) for worker in origin.codegen.workers)
    if captured["stdlib"] is not None:
        hints.update(
            (worker.scope, worker.process_id)
            for worker in (
                BackendNativeWorkerOrigin.from_payload(row)
                for row in captured["stdlib"]["workers"]
            )
        )
    if any(scope is None for scope, _ in hints):
        raise ValueError("compiler emission lacks its original supervisor scope")
    return {(scope, pid) for scope, pid in hints if scope is not None}


def compilation_process_images(
    *,
    supervisor: Path,
    rootfs: Path,
    policy_path: Path,
    receipt_path: Path,
    observation: object,
    compiler_image: tuple[str, str, int],
    linker_image: tuple[str, str, int],
    additional_images: tuple[tuple[str, str, int], ...],
    _scope_sources: dict[str, tuple[Path, Path]] | None = None,
    _verified_scopes: dict[str, tuple[dict[str, Any], Any]] | None = None,
) -> dict[str, Any]:
    """Join retained emission to actual images through native receipt replay.

    This is the process component of compilation evidence. It grants neither
    immutable-input namespace authority nor authenticated performance admission;
    those consumers must verify their own retained root and measurement joins.
    File selection, an embedded success flag, and a caller-authored image list
    cannot replace native verification of the policy, receipt and event journal.
    """
    from molt.cli.link_input_observation import validate_compilation_input_observation

    captured = validate_compilation_input_observation(observation)
    from molt.cli.link_input_observation import LinkInputObservation

    link = captured["link"]
    records = (
        link["links"].values()
        if link["schema"] == "molt.wasm-link-inputs.v2"
        else (link,)
    )
    for path, _digest, _size in (compiler_image, linker_image, *additional_images):
        execution_root.linux_root_relative_path(path)
    if any(
        LinkInputObservation.from_record(row).linker_path != linker_image[0]
        for row in records
    ):
        raise ValueError("linker process join selected another recorded producer")
    policy = read_exact(
        policy_path, max_bytes=16 * 1024**2, label="compilation policy scope"
    )
    from molt.backend_protocol import validate_emission_scope

    current_scope = validate_emission_scope(policy["nonce"])
    if current_scope is None:
        raise ValueError("compilation policy has no emission scope")
    sources = (
        _scope_sources
        if _scope_sources is not None
        else {current_scope: (policy_path, receipt_path)}
    )
    verified_scopes = _verified_scopes if _verified_scopes is not None else {}
    verified, events = _verified_emission_scope(
        current_scope,
        sources=sources,
        verified_scopes=verified_scopes,
        supervisor=supervisor,
        rootfs=rootfs,
    )
    hints = _compilation_emission_hints(captured)
    actual = []
    references = {}
    for scope, pid in sorted(hints):
        original, original_events = _verified_emission_scope(
            scope,
            sources=sources,
            verified_scopes=verified_scopes,
            supervisor=supervisor,
            rootfs=rootfs,
        )
        stable, image = original_events.producing_process(pid)
        if image != compiler_image:
            raise ValueError(
                "compiler emission belongs to another actual process image"
            )
        references[scope] = {
            "scope": scope,
            "policy_sha256": original["policy_sha256"],
            "receipt_identity_sha256": original["identity_sha256"],
            "event_log": original["event_log"],
        }
        actual.append(
            {
                "scope": scope,
                "process_id": pid,
                "stable_process_id": stable,
                "path": image[0],
                "sha256": image[1],
                "size": image[2],
            }
        )
    if linker_image not in events.images or any(
        row not in events.images for row in additional_images
    ):
        raise ValueError(
            "compilation journal omitted a selected actual linker/driver image"
        )
    return {
        "compiler_processes": actual,
        "emission_scopes": list(references.values()),
        "linker_image": {
            "path": linker_image[0],
            "sha256": linker_image[1],
            "size": linker_image[2],
        },
        "additional_images": [
            {"path": path, "sha256": digest, "size": size}
            for path, digest, size in additional_images
        ],
        "event_log": verified["event_log"],
        "receipt_identity_sha256": verified["identity_sha256"],
    }


# The controller runs only in the compilation projection. Its source is retained
# in the sealed input root and its command/environment are native-policy inputs.
# stdout is a bounded output archive; stderr remains available to run-export.
_COMPILATION_RUNTIME_STORE = "/work/state/installed-runtime"

_COMPILATION_CONTROLLER = r"""import hashlib,json,os,pathlib,shutil,stat,subprocess,sys,time,zipfile
config=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if set(config)!={"command","environment","timeout","output_max_bytes","cache_seed","cache_contract","compiler_source_root"}:
    raise ValueError("compilation controller configuration is invalid")
output=pathlib.Path("/work/output")
output.mkdir()
for name in ("tmp","state","cache","build-state","cargo","uv-cache"):
    (pathlib.Path("/work")/name).mkdir(exist_ok=True)
def copy_cache(source,destination,restore=False,paths=None):
    if not source.exists(): return
    if restore:
        sys.path.insert(0,str(pathlib.Path(config["compiler_source_root"])/"src"))
        os.environ.clear(); os.environ.update(config["environment"])
        from molt.default_paths import is_backend_cache_artifact_name
        from molt.cli.backend_artifact_contract import BackendArtifactContract,BackendArtifactKind
        from molt.cli.backend_cache import restore_backend_cache_artifact
        contract=BackendArtifactContract(BackendArtifactKind(config["cache_contract"]["kind"]),config["cache_contract"]["target_triple"])
        cache_artifact=lambda name:is_backend_cache_artifact_name(name) and name.endswith(contract.suffix)
    total=0; count=0
    for path in sorted(source.rglob("*") if paths is None else paths):
        info=path.lstat()
        if stat.S_ISDIR(info.st_mode): continue
        if not stat.S_ISREG(info.st_mode) or path.is_symlink(): raise ValueError("cache snapshot contains a link or special file")
        total+=info.st_size; count+=1
        if count>4096 or total>config["output_max_bytes"]: raise ValueError("cache snapshot exceeds compilation export budget")
        target=destination/path.relative_to(source); target.parent.mkdir(parents=True,exist_ok=True)
        if restore and path.name.endswith(".sync.json") and cache_artifact(path.name[:-len(".sync.json")]):
            if not path.with_name(path.name[:-len(".sync.json")]).is_file(): raise ValueError("cache snapshot receipt has no artifact")
            continue
        if restore and cache_artifact(path.name):
            restore_backend_cache_artifact(path,target,artifact_contract=contract)
            continue
        with path.open("rb") as original,target.open("xb") as copied:
            before=os.fstat(original.fileno())
            shutil.copyfileobj(original,copied,64*1024)
            after=os.fstat(original.fileno())
        identity=lambda row:(row.st_dev,row.st_ino,row.st_size,row.st_mtime_ns,row.st_ctime_ns)
        if identity(info)!=identity(before) or identity(before)!=identity(after) or identity(after)!=identity(path.lstat()): raise ValueError("cache generation changed during export")
copy_cache(pathlib.Path(config["cache_seed"]),pathlib.Path("/work/cache"),restore=True)
started=time.monotonic()
with (output/"command.stdout").open("xb") as stdout, (output/"command.stderr").open("xb") as stderr:
    completed=subprocess.run(config["command"],cwd="/work",env=config["environment"],stdout=stdout,stderr=stderr,timeout=config["timeout"],check=False)
result={"argv":config["command"],"returncode":completed.returncode,"duration_sec":time.monotonic()-started}
(output/"command.json").write_text(json.dumps(result,sort_keys=True,separators=(",",":"))+"\n",encoding="utf-8")
if completed.returncode==0:
    sys.path.insert(0,str(pathlib.Path(config["compiler_source_root"])/"src"))
    os.environ.clear(); os.environ.update(config["environment"])
    from molt.backend_protocol import BackendEmissionOrigin,BackendStdlibOrigin
    from molt.cli.backend_artifact_contract import BackendArtifactContract,BackendArtifactKind
    from molt.cli.backend_cache import compilation_cache_export_paths
    from molt.cli.link_fingerprints import _read_link_fingerprint
    from molt.cli.link_input_observation import validate_compilation_input_observation
    from molt.exact_json import read_exact
    matches=[]
    for path in output.rglob(".molt-artifact-receipt-*.json"):
        receipt=_read_link_fingerprint(path)
        if receipt is not None and "input_observation" in receipt["outputs"]: matches.append(receipt)
    if len(matches)!=1: raise ValueError("cache export requires one captured final-link publication")
    observation_path=pathlib.Path(matches[0]["outputs"]["input_observation"]["path"])
    observation_path.relative_to(output)
    observed=validate_compilation_input_observation(read_exact(observation_path,max_bytes=16*1024**2,label="cache export compilation observation"))
    compiler_origin=BackendEmissionOrigin.from_payload(observed["compiler"])
    stdlib_origin=None if observed["stdlib"] is None else BackendStdlibOrigin.from_payload(observed["stdlib"])
    contract=BackendArtifactContract(BackendArtifactKind(config["cache_contract"]["kind"]),config["cache_contract"]["target_triple"])
    with compilation_cache_export_paths(pathlib.Path("/work/cache"),cache=observed["cache"],compiler_origin=compiler_origin,stdlib_origin=stdlib_origin,artifact_contract=contract) as paths:
        copy_cache(pathlib.Path("/work/cache"),output/"cache-export",paths=paths)
files=[]; total=0
for path in sorted(output.rglob("*")):
    mode=path.lstat().st_mode
    if stat.S_ISDIR(mode): continue
    if not stat.S_ISREG(mode) or path.is_symlink(): raise ValueError("compilation output has a link or special file")
    total+=path.stat().st_size
    if len(files)>=4096 or total>config["output_max_bytes"]: raise ValueError("compilation output exceeds its declared bound")
    files.append(path)
with zipfile.ZipFile(sys.stdout.buffer,mode="w",compression=zipfile.ZIP_STORED,allowZip64=False) as archive:
    for path in files:
        info=zipfile.ZipInfo(path.relative_to(output).as_posix(),date_time=(1980,1,1,0,0,0))
        info.create_system=3; info.file_size=path.stat().st_size
        info.external_attr=(stat.S_IFREG | (0o555 if path.stat().st_mode & 0o111 else 0o444)) << 16
        with path.open("rb") as source, archive.open(info,"w") as sink:
            shutil.copyfileobj(source,sink,64*1024)
raise SystemExit(completed.returncode)
"""


def _compilation_supervisor_command(policy: str) -> list[str]:
    return [
        "/bin/molt-proof-supervisor",
        "run-export",
        "--policy",
        "/" + policy,
        "--receipt",
        "/evidence/receipt.json",
    ]


def _validate_compilation_inputs(
    *,
    evidence: Path,
    compilation: dict[str, Any],
    compiler: Any,
    lanes: tuple[ReleaseLane, ...],
    python_minor: str,
    launcher: list[str],
    controller_python: Path,
) -> Any:
    from molt.compiler_distribution import installed_compiler, verify_runtime_tree

    projection = SealedExecutionProjection.from_dict(compilation["execution"])
    if projection.mode != "compilation":
        raise ValueError(
            "installed compilation requires its typed namespace projection"
        )
    rootfs = evidence / "rootfs"
    if execution_root.root_inventory(rootfs) != compilation["root"]["files"]:
        raise ValueError("compilation inputs changed before execution")
    execution_root.validate_sealed_tar(
        evidence / "rootfs.tar",
        rootfs,
        expected_archive=compilation["root"]["archive"],
        projection=projection,
    )
    _validate_compilation_plan(
        rootfs=rootfs,
        compilation=compilation,
        lanes=lanes,
        python_minor=python_minor,
        launcher=launcher,
        controller_python=controller_python,
    )
    _validate_generation(compilation["supervisor_generation"], rootfs)
    source_root = execution_root.linux_root_relative_path(
        compilation["compiler_source_root"]
    )
    installed = installed_compiler(rootfs / source_root)
    from dataclasses import replace

    if (
        installed is None
        or replace(installed, source_root=compiler.source_root) != compiler
        or installed.record != compilation["compiler"]
        or installed.source_sha != compilation["source_sha"]
        or compilation["target"]
        != {"platform": "linux", "arch": compiler.record["arch"]}
    ):
        raise ValueError("compilation input root belongs to another installed compiler")
    installed.verify_sources()
    runtime = execution_root.compilation_runtime_inventory(
        installed, tuple(cell["runtime_cell"] for cell in compilation["cells"])
    )
    verify_runtime_tree(installed.runtime_root, runtime)
    return installed


def execute_compilation(
    *,
    evidence: Path,
    compilation: dict[str, Any],
    supervisor: Path,
    compiler: Any,
    lanes: tuple[ReleaseLane, ...],
    python_minor: str,
    launcher: list[str],
    controller_python: Path,
) -> dict[str, object]:
    """Import one immutable input root, run its declared cells, close custody.

    Every invocation receives a fresh ephemeral workspace. Compiler/Python/SDK
    bytes are copied, archived and imported once for this installed frontend
    coordinate; each cell retains its own native journal and output archive.
    This compilation evidence still requires measurement and release admission.
    """
    installed = _validate_compilation_inputs(
        evidence=evidence,
        compilation=compilation,
        compiler=compiler,
        lanes=lanes,
        python_minor=python_minor,
        launcher=launcher,
        controller_python=controller_python,
    )
    projection = SealedExecutionProjection.from_dict(compilation["execution"])
    rootfs = evidence / "rootfs"
    transport = SealedRootTransport()
    scope_sources = _compilation_scope_sources(evidence, compilation)
    verified_scopes: dict[str, tuple[dict[str, Any], Any]] = {}
    try:
        compilation["transport"] = transport.import_sealed_root(
            evidence / "rootfs.tar",
            sha256=compilation["root"]["archive"]["sha256"],
            arch=compilation["target"]["arch"],
            evidence=evidence / "transport",
            projection=projection,
        )
        for cell in compilation["cells"]:
            result = transport.run_sealed(
                _compilation_supervisor_command(cell["policy"]),
                timeout=projection.runtime_max_seconds,
                projection=projection,
            )
            stdout_path, stderr_path = (
                Path(result.pop("stdout_path")),
                Path(result.pop("stderr_path")),
            )
            receipt = evidence / "runs" / cell["id"] / "receipt.json"
            stderr = supervisor_custody.decode_supervisor_export(
                stderr_path, receipt_path=receipt
            )
            result.update(
                stdout=stdout_path.relative_to(evidence).as_posix(),
                stderr=stderr_path.relative_to(evidence).as_posix(),
                receipt=receipt.relative_to(evidence).as_posix(),
                guest_stderr_sha256=hashlib.sha256(stderr).hexdigest(),
            )
            cell["execution"] = result
            scope = read_exact(
                rootfs / cell["policy"],
                max_bytes=16 * 1024**2,
                label="compilation policy",
            )["nonce"]
            _verified_emission_scope(
                scope,
                sources=scope_sources,
                verified_scopes=verified_scopes,
                supervisor=supervisor,
                rootfs=rootfs,
            )
    finally:
        primary = sys.exception()
        try:
            transport.remove_sealed_root()
        except BaseException as cleanup:
            if primary is not None:
                raise BaseExceptionGroup(
                    "sealed compilation and cleanup failed", [primary, cleanup]
                )
            raise
    if execution_root.root_inventory(rootfs) != compilation["root"]["files"]:
        raise ValueError("compilation input root changed during execution")
    _validate_transport(
        compilation["transport"],
        sealed_root=compilation["root"],
        arch=compilation["target"]["arch"],
        projection=projection,
    )
    for scope in scope_sources:
        _verified_emission_scope(
            scope,
            sources=scope_sources,
            verified_scopes=verified_scopes,
            supervisor=supervisor,
            rootfs=rootfs,
        )
    for cell in compilation["cells"]:
        _receive_compilation_cell(
            evidence=evidence,
            rootfs=rootfs,
            compilation=compilation,
            cell=cell,
            installed=installed,
            supervisor=supervisor,
            scope_sources=scope_sources,
            verified_scopes=verified_scopes,
        )
        if compilation["cache_seed"]:
            current_scope = read_exact(
                rootfs / cell["policy"],
                max_bytes=16 * 1024**2,
                label="warm compilation policy",
            )["nonce"]
            if not any(
                row["scope"] != current_scope
                for row in cell["processes"]["compiler_processes"]
            ):
                raise ValueError(
                    "seeded compilation did not retain any original cached emission"
                )
    execution_root.retain_root(
        rootfs,
        evidence / "rootfs.tar",
        sealed=compilation["root"],
        resources=evidence.parent.parent / "execution-inputs",
        projection=projection,
    )
    compilation["files"] = _evidence_inventory(evidence)
    from molt.exact_json import encode_exact
    from molt.file_publication import atomic_write_bytes

    # A new final generation owns the verified references. Never mutate a
    # sealed cache artifact or overwrite an earlier completed compilation.
    atomic_write_bytes(
        evidence / "compilation.json", encode_exact(compilation), exclusive=True
    )
    execution_root.retire_root(
        rootfs,
        evidence / "rootfs.tar",
        sealed=compilation["root"],
        projection=projection,
    )
    return compilation


def _receive_compilation_cell(
    *,
    evidence: Path,
    rootfs: Path,
    compilation: dict[str, Any],
    cell: dict[str, Any],
    installed: Any,
    supervisor: Path,
    output_root: Path | None = None,
    scope_sources: dict[str, tuple[Path, Path]],
    verified_scopes: dict[str, tuple[dict[str, Any], Any]],
) -> None:
    projection = SealedExecutionProjection.from_dict(compilation["execution"])
    run = cell["execution"]
    if SealedExecutionProjection.from_dict(run["execution"]) != projection:
        raise ValueError("compilation launch changed its execution projection")
    actual_unit = validate_sealed_systemd_configuration(
        run["observation"],
        nonce=run["nonce"],
        root=compilation["transport"]["root"],
        command=_compilation_supervisor_command(cell["policy"]),
        projection=projection,
    )
    validate_sealed_systemd_terminal(
        run["observation"], invocation=actual_unit["InvocationID"]
    )
    validate_sealed_systemd_stop(
        run["teardown"], unit=actual_unit["Id"], invocation=actual_unit["InvocationID"]
    )
    if (
        run["stdout"] != f"transport/{run['nonce']}.stdout"
        or run["stderr"] != f"transport/{run['nonce']}.stderr"
        or run["receipt"] != f"runs/{cell['id']}/receipt.json"
    ):
        raise ValueError("compilation exported streams belong to another invocation")
    cell["outputs"] = receive_compilation_outputs(
        archive=evidence / cell["execution"]["stdout"],
        output=(output_root or evidence / "outputs") / cell["id"],
        projection=projection,
        command=cell["command"],
        runtime_cell=cell["runtime_cell"],
        target=cell["lane"]["target"],
    )
    from molt.cli.link_input_observation import LinkInputObservation

    lane = ReleaseLane(**cell["lane"])
    installed.verify_binary(lane.compiler_features, lane.compiler_profile)
    if cell["runtime_cell"] not in installed.runtime["cells"]:
        raise ValueError(
            "compilation runtime is not a member of the installed candidate"
        )
    compiler_identity = execution_root.file_identity(installed.binary)
    compiler_image = (
        "/" + installed.binary.relative_to(rootfs).as_posix(),
        compiler_identity["sha256"],
        compiler_identity["size"],
    )
    observation = cell["outputs"]["observation"]
    link = observation["link"]
    records = (
        link["links"].values()
        if link["schema"] == "molt.wasm-link-inputs.v2"
        else (link,)
    )
    selected_linkers = {
        LinkInputObservation.from_record(row).linker_path for row in records
    }
    if len(selected_linkers) != 1:
        raise ValueError("compilation family selected different linker producers")
    linker_path = selected_linkers.pop()
    linker_identity = execution_root.file_identity(
        rootfs / execution_root.linux_root_relative_path(linker_path)
    )
    additional_images = ()
    if lane.target == "native":
        driver = cell["outputs"]["preparation"]["driver"]
        observed_driver = execution_root.file_identity(
            rootfs / execution_root.linux_root_relative_path(driver["path"])
        )
        _equal_artifact(observed_driver, driver)
        additional_images = ((driver["path"], driver["sha256"], driver["size"]),)
    cell["processes"] = compilation_process_images(
        supervisor=supervisor,
        rootfs=rootfs,
        policy_path=rootfs / cell["policy"],
        receipt_path=evidence / run["receipt"],
        observation=observation,
        compiler_image=compiler_image,
        linker_image=(linker_path, linker_identity["sha256"], linker_identity["size"]),
        additional_images=additional_images,
        _scope_sources=scope_sources,
        _verified_scopes=verified_scopes,
    )


def validate_compilation(
    *,
    evidence: Path,
    compilation: dict[str, Any],
    compiler: Any,
    lanes: tuple[ReleaseLane, ...],
    python_minor: str,
    supervisor: Path,
    launcher: list[str],
    controller_python: Path,
) -> None:
    """Replay retained compile exports and native receipts at the consumer.

    The caller must supply the independently admitted installed compiler. This
    verifier neither executes a namespace nor trusts the saved output/process
    summaries. It reconstructs them from the sealed inputs, raw linker buffers
    and native-verified process journal, then compares the exact saved record.
    """
    expected_keys = {
        "schema",
        "execution",
        "target",
        "source_sha",
        "cells",
        "python",
        "source_sha256",
        "launcher",
        "controller_python",
        "compiler",
        "compiler_source_root",
        "environment_contract",
        "providers",
        "root",
        "supervisor_generation",
        "origins",
        "cache_seed",
        "transport",
        "files",
    }
    if not isinstance(compilation, dict) or set(compilation) != expected_keys:
        raise ValueError("retained compilation record is not exact")
    if [
        row
        for row in _evidence_inventory(evidence)
        if row["path"] != "compilation.json"
    ] != compilation["files"]:
        raise ValueError("retained compilation evidence file closure changed")
    if (
        read_exact(
            evidence / "compilation.json",
            max_bytes=64 * 1024**2,
            label="retained compilation record",
        )
        != compilation
    ):
        raise ValueError(
            "retained compilation record differs from its supplied projection"
        )
    installed = _validate_compilation_inputs(
        evidence=evidence,
        compilation=compilation,
        compiler=compiler,
        lanes=lanes,
        python_minor=python_minor,
        launcher=launcher,
        controller_python=controller_python,
    )
    projection = SealedExecutionProjection.from_dict(compilation["execution"])
    _validate_transport(
        compilation["transport"],
        sealed_root=compilation["root"],
        arch=compilation["target"]["arch"],
        projection=projection,
    )
    cell_keys = {
        "id",
        "lane",
        "command",
        "runtime_cell",
        "policy",
        "execution",
        "outputs",
        "processes",
    }
    scope_sources = _compilation_scope_sources(evidence, compilation)
    verified_scopes: dict[str, tuple[dict[str, Any], Any]] = {}
    for scope in scope_sources:
        _verified_emission_scope(
            scope,
            sources=scope_sources,
            verified_scopes=verified_scopes,
            supervisor=supervisor,
            rootfs=evidence / "rootfs",
        )
    with tempfile.TemporaryDirectory(prefix="molt-compilation-readback-") as raw:
        for cell in compilation["cells"]:
            if not isinstance(cell, dict) or set(cell) != cell_keys:
                raise ValueError("retained compilation cell is not exact")
            replayed = dict(cell)
            _receive_compilation_cell(
                evidence=evidence,
                rootfs=evidence / "rootfs",
                compilation=compilation,
                cell=replayed,
                installed=installed,
                supervisor=supervisor,
                output_root=Path(raw),
                scope_sources=scope_sources,
                verified_scopes=verified_scopes,
            )
            if compilation["cache_seed"]:
                current_scope = read_exact(
                    evidence / "rootfs" / cell["policy"],
                    max_bytes=16 * 1024**2,
                    label="warm compilation policy",
                )["nonce"]
                if not any(
                    row["scope"] != current_scope
                    for row in replayed["processes"]["compiler_processes"]
                ):
                    raise ValueError(
                        "retained seeded compilation contains no reused emission"
                    )
            if replayed != cell:
                raise ValueError(
                    "retained compilation summary differs from owning evidence"
                )
            if (
                execution_root.root_inventory(evidence / "outputs" / cell["id"])
                != replayed["outputs"]["files"]
            ):
                raise ValueError(
                    "retained compilation output differs from its raw export"
                )
    if [
        row
        for row in _evidence_inventory(evidence)
        if row["path"] != "compilation.json"
    ] != compilation["files"]:
        raise ValueError("retained compilation evidence changed during replay")


def _validate_compilation_command(
    command: dict[str, Any], output: dict[str, Any], retained: Path
) -> None:
    expected = output["command"]
    if (
        command["argv"] != expected["argv"]
        or command["returncode"] != expected["returncode"]
        or command["duration_seconds"] != expected["duration_sec"]
        or command["stdout_sha256"]
        != execution_root.file_identity(retained / "command.stdout")["sha256"]
        or command["stderr_sha256"]
        != execution_root.file_identity(retained / "command.stderr")["sha256"]
    ):
        raise ValueError(
            "installed command summary differs from its raw compilation export"
        )


def validate_installed_compilation(
    reference: object,
    *,
    evidence_root: Path,
    candidate: dict[str, Any],
    proof: dict[str, Any],
    launcher: list[str],
    controller_python: Path,
    owner: str,
    supervisor: Path,
    selected_lanes: tuple[ReleaseLane, ...] | None = None,
    selected_stdlib_profile: str | None = None,
) -> None:
    """Bind the retained compilation to the release candidate and replay cells."""
    from molt.cli.config_resolution import RUNTIME_STDLIB_PROFILE_TIERS
    from molt.compiler_distribution import installed_compiler

    if owner not in {"bundle", "pip"}:
        raise ValueError("installed compilation owner is invalid")
    if selected_stdlib_profile is not None and (
        not isinstance(selected_stdlib_profile, str)
        or selected_stdlib_profile not in RUNTIME_STDLIB_PROFILE_TIERS
    ):
        raise ValueError("installed compilation runtime stdlib profile is invalid")
    lanes = capture_release_lanes(ROOT).require_selection(selected_lanes)
    expected_path = f"compilation-evidence/{owner}-{proof['python']}/compilation.json"
    if (
        not isinstance(reference, dict)
        or set(reference) != {"path", "filename", "size", "sha256"}
        or reference["path"] != expected_path
        or reference["filename"] != "compilation.json"
    ):
        raise ValueError("installed compilation reference is invalid")
    record_path = evidence_root / expected_path
    compilation = _artifact_json(
        record_path,
        reference,
        max_bytes=64 * 1024**2,
        label="installed compilation evidence",
    )
    if not isinstance(compilation, dict):
        raise ValueError("installed compilation evidence is not an object")
    with execution_root.materialized_root(
        record_path.parent / "rootfs",
        record_path.parent / "rootfs.tar",
        sealed=compilation["root"],
        resources=evidence_root / "execution-inputs",
        projection=SealedExecutionProjection.from_dict(compilation["execution"]),
    ):
        source_relative = execution_root.linux_root_relative_path(
            compilation["compiler_source_root"]
        )
        source_root = PurePosixPath("/") / source_relative
        source = record_path.parent / "rootfs" / source_relative
        installed = installed_compiler(source)
        if (
            installed is None
            or installed.source_sha != candidate["source_sha"]
            or installed.record != candidate["compiler"]
            or installed.launcher != candidate["launcher"]
            or installed.wasm_facts != candidate["wasm_facts"]
            or installed.runtime != candidate["runtime"]
            or canonical_json_sha256(list(installed.files))
            != candidate["native_build"]["source"]["files_sha256"]
        ):
            raise ValueError(
                "retained compilation installed inputs differ from the release candidate"
            )
        interpreter = proof["execution"]["python"]
        if interpreter["executable"] != controller_python.as_posix():
            raise ValueError(
                "installed compilation selected another controller interpreter"
            )
        interpreter_file = (
            record_path.parent
            / "rootfs"
            / execution_root.linux_root_relative_path(str(controller_python))
        )
        _equal_artifact(execution_root.file_identity(interpreter_file), interpreter)
        installed.verify_launcher()
        installed.verify_wasm_facts()
        if (
            capture_release_lanes(installed.source_root).require_selection(
                selected_lanes
            )
            != lanes
        ):
            raise ValueError("installed compilation lane policy differs from release")
        validate_compilation(
            evidence=record_path.parent,
            compilation=compilation,
            compiler=installed,
            lanes=lanes,
            python_minor=proof["python"],
            supervisor=supervisor,
            launcher=launcher,
            controller_python=controller_python,
        )
        cold_evidence = record_path.parent.with_name(record_path.parent.name + "-cold")
        cold = read_exact(
            cold_evidence / "compilation.json",
            max_bytes=64 * 1024**2,
            label="cold installed compilation",
        )
        with execution_root.materialized_root(
            cold_evidence / "rootfs",
            cold_evidence / "rootfs.tar",
            sealed=cold["root"],
            resources=evidence_root / "execution-inputs",
            projection=SealedExecutionProjection.from_dict(cold["execution"]),
        ):
            validate_compilation(
                evidence=cold_evidence,
                compilation=cold,
                compiler=installed,
                lanes=lanes,
                python_minor=proof["python"],
                supervisor=supervisor,
                launcher=launcher,
                controller_python=controller_python,
            )
            if selected_stdlib_profile is not None and any(
                cell["runtime_cell"]["key"]["stdlib_profile"] != selected_stdlib_profile
                for phase in (compilation, cold)
                for cell in phase["cells"]
            ):
                raise ValueError(
                    "installed compilation used another runtime stdlib profile"
                )
            if cold["cache_seed"] or cold["origins"] or not compilation["cache_seed"]:
                raise ValueError(
                    "installed cold/warm compilation phases have incorrect reuse inputs"
                )
            expected_origins = []
            for scope, cell_id, _, receipt_path in _compilation_reuse_scopes(
                cold_evidence, cold
            ):
                receipt = read_exact(
                    receipt_path,
                    max_bytes=supervisor_custody.SUPERVISOR_BUDGETS["receipt_bytes"],
                    label="cold emission receipt",
                )
                expected_origins.append(
                    {
                        "scope": scope,
                        "cell": cell_id,
                        "identity_sha256": receipt["identity_sha256"],
                    }
                )
            expected_seed = []
            for cell in cold["cells"]:
                for row in cell["outputs"]["files"]:
                    relative = PurePosixPath(row["path"])
                    if relative.is_relative_to("cache-export"):
                        expected_seed.append(
                            {
                                "path": "app/cache/"
                                + cell["id"]
                                + "/"
                                + relative.relative_to("cache-export").as_posix(),
                                "sha256": row["sha256"],
                                "size": row["size"],
                                "mode": 0o444,
                            }
                        )
            if compilation["origins"] != expected_origins or compilation[
                "cache_seed"
            ] != sorted(expected_seed, key=lambda row: row["path"]):
                raise ValueError(
                    "warm compilation did not import the selected cold cache generation"
                )
            cells = proof["cells"]
            if len(cells) != len(compilation["cells"]):
                raise ValueError("release proof omits an installed compilation product")
            by_role = {row["role"]: row for row in proof["commands"]}
            prefix = "pip_" if owner == "pip" else ""
            for lane, produced in zip(lanes, cold["cells"], strict=True):
                command = by_role[
                    f"{prefix}cold_{'build' if lane.target == 'native' else 'run'}_{lane.id}"
                ]
                _validate_compilation_command(
                    command, produced["outputs"], cold_evidence / "outputs" / lane.id
                )
            for lane, cell, produced in zip(
                lanes, cells, compilation["cells"], strict=True
            ):
                retained = record_path.parent / "outputs" / lane.id
                output = produced["outputs"]
                artifact = output["artifact"]
                if (
                    cell["lane"] != produced["lane"]
                    or any(
                        cell["artifact"][key] != artifact[key]
                        for key in ("size", "sha256")
                    )
                    or PurePosixPath(cell["artifact"]["path"]).name
                    != PurePosixPath(artifact["path"]).name
                ):
                    raise ValueError(
                        "standalone product differs from its installed compilation"
                    )
                command = by_role[
                    f"{prefix}{'build' if lane.target == 'native' else 'run'}_{lane.id}"
                ]
                _validate_compilation_command(command, output, retained)
                diagnostics = read_exact(
                    retained / "diagnostics.json",
                    max_bytes=16 * 1024**2,
                    label="installed compilation diagnostics",
                )
                compiler_row = (
                    diagnostics.get("compiler", {})
                    if isinstance(diagnostics, dict)
                    else {}
                )
                compiler_path = source_root.parent / installed.record["path"]
                if (
                    diagnostics.get("program") != lane.as_record()
                    or compiler_row.get("path") != compiler_path.as_posix()
                    or compiler_row.get("sha256") != cell["compiler_sha256"]
                    or compiler_row.get("fingerprint") != cell["compiler_fingerprint"]
                    or compiler_row.get("cargo_profile") != lane.compiler_profile
                ):
                    raise ValueError(
                        "installed product diagnostics differ from admitted compiler or lane"
                    )
                if lane.target == "wasm":
                    _equal_artifact(
                        execution_root.file_identity(retained / "manifest.json"),
                        cell["manifest"],
                    )


def compilation_runtime_selection(
    diagnostics: Path,
    *,
    compiler: Any,
    lane: ReleaseLane,
) -> tuple[str, dict[str, Any]]:
    """Read the preflight selection hint, without granting consumed-byte proof.

    Frontend reachability chooses the concrete tier. The fresh sealed build
    must subsequently consume the same installed member, independently checked
    from its linker reproducer. This avoids a second tier-selection algorithm.
    """
    from molt.compiler_distribution import NATIVE_RUNTIME_CELL, WASM_RUNTIME_CELL

    value = read_exact(
        diagnostics,
        max_bytes=16 * 1024 * 1024,
        label="installed compilation preparation diagnostics",
    )
    row = value.get("installed_runtime_selection") if isinstance(value, dict) else None
    kind = NATIVE_RUNTIME_CELL if lane.target == "native" else WASM_RUNTIME_CELL
    if (
        not isinstance(row, dict)
        or row not in compiler.runtime["cells"]
        or row.get("kind") != kind
        or row.get("key", {}).get("cargo_profile") != lane.runtime_profile
        or value.get("program") != lane.as_record()
    ):
        raise ValueError(
            "installed compilation has no exact preflight runtime selection"
        )
    return kind, dict(row["key"])


def _compilation_reuse_scopes(
    previous: Path, prior: dict[str, Any]
) -> list[tuple[str, str, Path, Path]]:
    sources = _compilation_scope_sources(previous, prior)
    scope_cells = {row["scope"]: row["cell"] for row in prior["origins"]}
    for cell in prior["cells"]:
        policy = read_exact(
            previous / "rootfs" / cell["policy"],
            max_bytes=16 * 1024**2,
            label="original policy",
        )
        scope_cells[policy["nonce"]] = cell["id"]
    needed_scopes = {
        scope
        for cell in prior["cells"]
        for scope, _ in _compilation_emission_hints(cell["outputs"]["observation"])
    }
    if len(needed_scopes) + len(prior["cells"]) > _MAX_EMISSION_SCOPES:
        raise ValueError("selected cache emission scopes exceed the retained bound")
    return [
        (scope, scope_cells[scope], *sources[scope]) for scope in sorted(needed_scopes)
    ]


def _retain_compilation_reuse(
    *,
    evidence: Path,
    previous: Path,
    compiler: Any,
    lanes: tuple[ReleaseLane, ...],
    python_minor: str,
    supervisor: Path,
    launcher: list[str],
    controller_python: Path,
) -> tuple[list[dict[str, str]], list[dict[str, Any]]]:
    """Snapshot one validated export and its distinct raw original sessions.

    Copy no recursive evidence roots. The resulting generation references only
    its own immutable files; prior evidence remains unchanged and independently
    replayable. This retains process evidence, not performance admission.
    """
    identity, prior = capture_exact(
        previous / "compilation.json",
        max_bytes=64 * 1024**2,
        label="prior completed compilation",
    )
    with execution_root.materialized_root(
        previous / "rootfs",
        previous / "rootfs.tar",
        sealed=prior["root"],
        resources=previous.parent.parent / "execution-inputs",
        projection=SealedExecutionProjection.from_dict(prior["execution"]),
    ):
        validate_compilation(
            evidence=previous,
            compilation=prior,
            compiler=compiler,
            lanes=lanes,
            python_minor=python_minor,
            supervisor=supervisor,
            launcher=launcher,
            controller_python=controller_python,
        )
        expected = {row["path"]: row for row in prior["files"]}
        expected.update(
            {"rootfs/" + row["path"]: row for row in prior["root"]["files"]}
        )

        def snapshot(source: Path, destination: str, *, root: Path) -> dict[str, Any]:
            relative = source.relative_to(previous).as_posix()
            row = expected.get(relative)
            if row is None:
                raise ValueError(
                    "original emission evidence is outside completed inventory"
                )
            actual = execution_root.stage_file(
                root, destination, source, executable=False, max_bytes=row["size"]
            )
            _equal_artifact(actual, row)
            return {
                "path": destination,
                "sha256": actual["sha256"],
                "size": actual["size"],
                "mode": 0o444,
            }

        origins = []
        for scope, cell_id, policy_path, receipt_path in _compilation_reuse_scopes(
            previous, prior
        ):
            receipt = read_exact(
                receipt_path,
                max_bytes=supervisor_custody.SUPERVISOR_BUDGETS["receipt_bytes"],
                label="original closed receipt",
            )
            receipt_identity = receipt["identity_sha256"]
            from molt.backend_protocol import validate_emission_scope

            if validate_emission_scope(receipt_identity) is None:
                raise ValueError("original emission receipt has no exact identity")
            owner = "origins/" + receipt_identity
            config_path = (
                previous / "rootfs" / ("app/compilation-" + cell_id + ".json")
                if policy_path.is_relative_to(previous / "rootfs")
                else policy_path.with_name("config.json")
            )
            snapshot(policy_path, owner + "/policy.json", root=evidence)
            snapshot(config_path, owner + "/config.json", root=evidence)
            snapshot(receipt_path, owner + "/receipt.json", root=evidence)
            event_name = receipt["event_log"]["file"]
            execution_root.relative_path(event_name)
            if Path(event_name).name != event_name:
                raise ValueError(
                    "original emission event reference escapes its session"
                )
            snapshot(
                receipt_path.with_name(event_name),
                owner + "/" + event_name,
                root=evidence,
            )
            origins.append(
                {
                    "scope": scope,
                    "cell": cell_id,
                    "identity_sha256": receipt_identity,
                }
            )
        seeds = []
        for cell in prior["cells"]:
            for row in cell["outputs"]["files"]:
                relative = PurePosixPath(row["path"])
                if not relative.is_relative_to("cache-export"):
                    continue
                leaf = relative.relative_to("cache-export").as_posix()
                source = previous / "outputs" / cell["id"] / relative
                seeds.append(
                    snapshot(
                        source,
                        "app/cache/" + cell["id"] + "/" + leaf,
                        root=evidence / "rootfs",
                    )
                )
        if not seeds:
            raise ValueError("prior compilation retained no cache export to reuse")
        verify_stable_regular_file_identity(
            identity, label="prior completed compilation", hash_content=True
        )
        return origins, sorted(seeds, key=lambda row: row["path"])


def _compilation_cache_contract(
    lane: ReleaseLane, runtime_cell: dict[str, Any]
) -> dict[str, str]:
    from molt.compilation_target import admit_target
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract

    # consumer_guest_command selects native build's default bin emission or
    # WASM run's wasm emission; exact command replay rejects other requests.
    contract = resolve_backend_artifact_contract(
        target=admit_target(runtime_cell["key"]["target_triple"]),
        emit_mode="bin" if lane.target == "native" else "wasm",
    )
    assert contract.target_triple is not None
    return {"kind": contract.kind.value, "target_triple": contract.target_triple}


def prepare_compilation(
    *,
    evidence: Path,
    compiler: Any,
    launcher: list[str],
    python_executable: Path,
    python_captures: list[dict[str, Any]],
    toolchain_capture: dict[str, Any],
    cas_root: Path,
    native_link_inputs: dict[str, tuple[Path, object, object]],
    environment: dict[str, str],
    cells: list[tuple[ReleaseLane, str, dict[str, Any]]],
    python_minor: str,
    projection: SealedExecutionProjection,
    archive_cache: Path,
    supervisor: Path,
    generation: dict[str, Any],
    reuse_compilation: Path | None = None,
) -> dict[str, Any]:
    """Prepare one admission-only installed build in a closed input namespace.

    The caller supplies existing canonical captures after installed CLI setup.
    This path performs no downloads or builds and introduces no provider. All
    copies, full input hashing and linker reproducer writes are admission-only.
    """
    from molt.compiler_distribution import InstalledCompiler
    from molt.cli.installed_runtime import select_installed_runtime_cell
    from molt.exact_json import encode_exact
    from tools.proof_queue_pkg.execution_environment import (
        _deterministic_execution_environment,
        _environment_name_class,
    )
    from molt.process_environment import NameRule
    from .release_authority import consumer_guest_command, consumer_guest_source

    if (
        not isinstance(compiler, InstalledCompiler)
        or compiler.record["platform"] != "linux"
        or projection.mode != "compilation"
        or not python_captures
        or not cells
        or len({lane.id for lane, _, _ in cells}) != len(cells)
        or any(
            lane not in capture_release_lanes(compiler.source_root).lanes
            for lane, _, _ in cells
        )
    ):
        raise ValueError(
            "installed compilation inputs have no applicable closed-root authority"
        )
    if set(native_link_inputs) != {
        lane.id for lane, _, _ in cells if lane.target == "native"
    }:
        raise ValueError(
            "compilation preparation omits or substitutes native linker resources"
        )
    if evidence.exists():
        raise ValueError("compilation evidence destination already exists")
    rootfs = evidence / "rootfs"
    rootfs.mkdir(parents=True)
    # The namespace has its own native process authority. Host queue tokens,
    # suite leases and scratch paths are not valid capabilities in that root.
    detached_environment = {
        name: value
        for name, value in environment.items()
        if _environment_name_class(name, rule=NameRule.EXACT) != "queue-owned-custody"
    }
    selected_env, environment_contract = _deterministic_execution_environment(
        detached_environment,
        override_names=(),
    )
    environment_contract["omitted_names"] = sorted(
        set(environment_contract["omitted_names"])
        | (environment.keys() - detached_environment.keys())
    )
    compile_env = dict(selected_env)
    compile_env.update(
        {
            "HOME": "/absent",
            "TMPDIR": "/work/tmp",
            "MOLT_EXT_ROOT": "/work/state",
            "MOLT_CACHE": "/work/cache",
            "MOLT_BUILD_STATE_DIR": "/work/build-state",
            "MOLT_INSTALLED_RUNTIME_STORE": _COMPILATION_RUNTIME_STORE,
            "CARGO_TARGET_DIR": "/work/cargo",
            "MOLT_BACKEND_DAEMON_SOCKET_DIR": "/work/sockets",
            "UV_CACHE_DIR": "/work/uv-cache",
            "UV_OFFLINE": "1",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONHASHSEED": "0",
        }
    )
    environment_contract["passed_names"] = sorted(
        {*compile_env, "MOLT_EMISSION_SCOPE"}, key=str.casefold
    )
    if not compile_env.get("MOLT_HOME"):
        raise ValueError("installed compilation must retain its prepared MOLT_HOME")
    selected_cells = [
        (
            lane,
            select_installed_runtime_cell(
                compiler,
                kind=runtime_kind,
                key=runtime_key,
                retention_store=Path(_COMPILATION_RUNTIME_STORE),
            ),
        )
        for lane, runtime_kind, runtime_key in cells
    ]
    execution_root.stage_compilation_inputs(
        rootfs,
        compiler=compiler,
        runtime_cells=tuple(cell.record for _, cell in selected_cells),
        python_captures=python_captures,
        toolchain_capture=toolchain_capture,
        cas_root=cas_root,
    )
    for reproducer, observation, preparation in native_link_inputs.values():
        execution_root.stage_compilation_link_inputs(
            rootfs, reproducer=reproducer, observation=observation
        )
        execution_root.stage_compilation_driver_inputs(rootfs, preparation=preparation)
    payloads, providers = execution_root.support_payloads(
        archive_cache, arch=compiler.record["arch"]
    )
    existing = {row["path"]: row for row in execution_root.root_inventory(rootfs)}
    for name, data in payloads.items():
        if name in existing:
            _equal_artifact(
                existing[name],
                {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()},
            )
        else:
            execution_root.write_payload(rootfs, name, data, executable=True)
    supervisor_row = execution_root.stage_file(
        rootfs,
        "bin/molt-proof-supervisor",
        supervisor,
        executable=True,
        max_bytes=generation["binary"]["size_bytes"],
    )
    _equal_artifact(
        supervisor_row,
        {
            "size": generation["binary"]["size_bytes"],
            "sha256": generation["binary"]["sha256"],
        },
    )
    source = consumer_guest_source(python_minor).encode()
    execution_root.write_payload(rootfs, "app/source.py", source, executable=False)
    execution_root.write_payload(
        rootfs,
        "app/compilation-controller.py",
        _COMPILATION_CONTROLLER.encode(),
        executable=False,
    )
    origins, cache_seed = (
        ([], [])
        if reuse_compilation is None
        else _retain_compilation_reuse(
            evidence=evidence,
            previous=reuse_compilation,
            compiler=compiler,
            lanes=tuple(lane for lane, _, _ in cells),
            python_minor=python_minor,
            supervisor=supervisor,
            launcher=launcher,
            controller_python=python_executable,
        )
    )
    files = execution_root.root_inventory(rootfs)
    executable = python_executable.as_posix()
    controller_image = next(
        (row for row in files if "/" + row["path"] == executable), None
    )
    if controller_image is None or controller_image["mode"] != 0o555:
        raise ValueError(
            "compilation controller interpreter has no captured lexical image"
        )
    fixed_images = []
    for row in files:
        if row["mode"] != 0o555 or row["path"] == "bin/molt-proof-supervisor":
            continue
        path = "/" + row["path"]
        role = (
            "compile-controller"
            if path == executable
            else "compiler"
            if path == compiler.binary.as_posix()
            else "compile-tool"
        )
        fixed_images.append(
            {
                "role": role,
                "path": path,
                "sha256": row["sha256"],
                "root_exit_disposition": "terminate"
                if role == "compiler"
                else "require-exit",
            }
        )
    prepared_cells = []
    for lane, selected_cell in selected_cells:
        runtime_cell = dict(selected_cell.record)
        output = (
            "/work/output/program.wasm"
            if lane.target == "wasm"
            else "/work/output/program"
        )
        command = consumer_guest_command(
            launcher,
            lane=lane,
            python_minor=python_minor,
            diagnostics="/work/output/diagnostics.json",
            output=output,
            source="/app/source.py",
            capture_link_inputs=True,
        )
        config_path = "app/compilation-" + lane.id + ".json"
        policy_path = "policies/compilation-" + lane.id + ".json"
        scope = secrets.token_hex(32)
        config = {
            "command": command,
            "environment": {
                **compile_env,
                **lane.environment(),
                "MOLT_EMISSION_SCOPE": scope,
            },
            "cache_seed": "/app/cache/" + lane.id,
            "cache_contract": _compilation_cache_contract(lane, runtime_cell),
            "compiler_source_root": compiler.source_root.as_posix(),
            "timeout": projection.runtime_max_seconds - 10,
            "output_max_bytes": projection.stdout_max_bytes,
        }
        if config["timeout"] <= 0:
            raise ValueError(
                "compilation execution budget cannot contain its controller"
            )
        files.append(
            execution_root.write_payload(
                rootfs, config_path, encode_exact(config), executable=False
            )
        )
        policy = {
            "schema": supervisor_custody.SUPERVISOR_POLICY_SCHEMA,
            "nonce": scope,
            "mode": "declared-tree",
            "cwd": "/work",
            "command": [
                executable,
                "-I",
                "-B",
                "/app/compilation-controller.py",
                "/" + config_path,
            ],
            "environment": dict(ENVIRONMENT),
            "root_role": "compile-controller",
            "derived_roots": [],
            "fixed_images": fixed_images,
        }
        files.append(
            execution_root.write_payload(
                rootfs,
                policy_path,
                supervisor_custody.encode_supervisor_policy(policy),
                executable=False,
            )
        )
        prepared_cells.append(
            {
                "id": lane.id,
                "lane": lane.as_record(),
                "command": command,
                "runtime_cell": runtime_cell,
                "policy": policy_path,
            }
        )
    seal = execution_root.seal_root(
        rootfs, evidence / "rootfs.tar", expected_files=files, projection=projection
    )
    return {
        "schema": "molt.installed-compilation.v4",
        "origins": origins,
        "cache_seed": cache_seed,
        "execution": projection.to_dict(),
        "target": {"platform": "linux", "arch": compiler.record["arch"]},
        "source_sha": compiler.source_sha,
        "cells": prepared_cells,
        "python": python_minor,
        "source_sha256": hashlib.sha256(source).hexdigest(),
        "launcher": list(launcher),
        "controller_python": python_executable.as_posix(),
        "compiler": dict(compiler.record),
        "compiler_source_root": compiler.source_root.as_posix(),
        "environment_contract": environment_contract,
        "providers": providers,
        "root": seal,
        "supervisor_generation": generation,
    }


def receive_compilation_outputs(
    *,
    archive: Path,
    output: Path,
    projection: SealedExecutionProjection,
    command: list[str],
    runtime_cell: dict[str, Any],
    target: str,
) -> dict[str, Any]:
    """Recheck the exported final-link family against its raw input buffers.

    This component consumes output bytes, not an embedded success claim. The
    caller must separately establish the closed namespace and actual process
    images. It neither admits a performance cell nor authenticates a release.
    """
    from .archive import ArchivePolicy
    from molt.cli.link_fingerprints import _read_link_fingerprint
    from molt.cli.link_input_observation import (
        LinkInputObservation,
        capture_link_inputs,
        validate_compilation_input_observation,
    )
    from molt.cli.static_archive_identity import artifact_content_identity
    from molt.artifact_publication import publication_receipt_path

    if projection.mode != "compilation" or target not in {"native", "wasm"}:
        raise ValueError("compile export has no applicable execution projection")
    # The existing release transport bounds metadata before allocation, rejects
    # links/collisions/traversal and publishes the extracted tree atomically.
    bound = projection.stdout_max_bytes
    identity = extract_zip_strict(
        archive,
        output,
        policy=ArchivePolicy(
            max_archive_bytes=bound,
            max_file_bytes=bound,
            max_total_bytes=bound,
        ),
    )
    result = read_exact(
        output / "command.json",
        max_bytes=1024 * 1024,
        label="installed compilation command result",
    )
    import math

    if (
        not isinstance(result, dict)
        or set(result) != {"argv", "returncode", "duration_sec"}
        or result["argv"] != command
        or type(result["returncode"]) is not int
        or result["returncode"] != 0
        or type(result["duration_sec"]) not in {int, float}
        or not math.isfinite(result["duration_sec"])
        or result["duration_sec"] < 0
    ):
        raise ValueError("exported compilation command differs or failed")
    for stream in ("command.stdout", "command.stderr"):
        execution_root.file_identity(output / stream)
    # Discover through the one canonical final-link receipt. A caller-authored
    # observation path or self-contained summary cannot select another family.
    matches = []
    for path in output.rglob(".molt-artifact-receipt-*.json"):
        if path.is_relative_to(output / "cache-export"):
            continue
        receipt = _read_link_fingerprint(path)
        if receipt is not None and "input_observation" in receipt["outputs"]:
            matches.append((path, receipt))
    if len(matches) != 1:
        raise ValueError(
            "compile export omitted or repeated its observed final-link family"
        )
    receipt_path, receipt = matches[0]
    family = receipt["outputs"]
    primary_role = "binary" if target == "native" else "linked"
    if (
        primary_role not in family
        or ("linked" if target == "native" else "binary") in family
    ):
        raise ValueError("compile export final-link target differs")
    projected: dict[str, Path] = {}
    for role, row in family.items():
        original = PurePosixPath("/") / execution_root.linux_root_relative_path(
            row["path"]
        )
        try:
            relative = original.relative_to("/work/output")
        except ValueError as exc:
            raise ValueError(
                "final-link output escaped its retained compile export"
            ) from exc
        execution_root.relative_path(relative.as_posix())
        local = output.joinpath(*relative.parts)
        if (
            local in projected.values()
            or artifact_content_identity(local) != row["identity"]
        ):
            raise ValueError("exported final-link bytes differ from their publication")
        projected[role] = local
    if publication_receipt_path(projected[primary_role]) != receipt_path:
        raise ValueError("final-link receipt names another publication anchor")
    observation = validate_compilation_input_observation(
        read_exact(
            projected["input_observation"],
            max_bytes=16 * 1024 * 1024,
            label="exported compiler/linker observation",
        )
    )
    link = observation["link"]
    if target == "native":
        reports = [("input_reproducer", link)]
        runtime_role = "runtime_archive"
    else:
        if link["schema"] != "molt.wasm-link-inputs.v2":
            raise ValueError("WASM compilation has no source-buffer observation")
        reports = [
            (name + "_input_reproducer", item) for name, item in link["links"].items()
        ]
        runtime_role = "wasm_reloc_member"
    from contextlib import contextmanager
    from collections.abc import Iterator
    from typing import BinaryIO
    from molt.backend_protocol import BackendEmissionOrigin, BackendStdlibOrigin

    # The retained LLD buffers, not the producer's archive paths or whole-file
    # summaries, must own the constituent check. Retain only worker archives
    # during the existing traversal; the canonical verifier owns their member
    # coverage/order/digests and documented padding. In-process and WASM bytes
    # already have the whole-buffer comparison below and need no extra copy.
    worker_archives: dict[str, list[BackendEmissionOrigin | BackendStdlibOrigin]] = {}
    if target == "native":
        native_roles = dict(LinkInputObservation.from_record(link).roles)
        compiler_origin = BackendEmissionOrigin.from_payload(observation["compiler"])
        if compiler_origin.codegen.workers:
            worker_archives.setdefault(native_roles["compiler_output"], []).append(
                compiler_origin
            )
        if observation["stdlib"] is not None:
            worker_archives.setdefault(native_roles["stdlib"], []).append(
                BackendStdlibOrigin.from_payload(observation["stdlib"])
            )

    @contextmanager
    def verify_worker_buffer(path: str, _size: int) -> Iterator[BinaryIO | None]:
        origins = worker_archives.get(path)
        if origins is None:
            yield None
            return
        with tempfile.TemporaryDirectory(prefix="molt-received-worker-archive-") as raw:
            retained = Path(raw) / "archive.a"
            with retained.open("xb") as destination:
                yield destination
            for origin in origins:
                origin.verify_output(retained)

    for role, record in reports:
        if role not in projected:
            raise ValueError("compile export omitted the raw linker reproducer")
        declared = LinkInputObservation.from_record(record)
        contents = {
            path: {"size": size, "sha256": digest}
            for path, size, digest in declared.inputs
        }
        actual = capture_link_inputs(
            projected[role],
            dialect=declared.dialect,
            linker_path=declared.linker_path,
            expected={name: (path, contents[path]) for name, path in declared.roles},
            input_sink=verify_worker_buffer if worker_archives else None,
        )
        if actual != declared:
            raise ValueError("exported summary differs from raw linker input buffers")
    preparation = None
    if target == "native":
        from molt.cli.link_input_observation import validate_native_link_preparation

        if not {"driver_preparation", "driver_dependencies"} <= projected.keys():
            raise ValueError(
                "native compile export omitted its driver preparation family"
            )
        preparation = validate_native_link_preparation(
            read_exact(
                projected["driver_preparation"],
                max_bytes=16 * 1024 * 1024,
                label="native driver preparation",
            )
        )
    runtime = [row for row in runtime_cell["files"] if row["role"] == runtime_role]
    if len(runtime) != 1:
        raise ValueError("selected installed runtime has no unique consumed member")
    expected_runtime = {key: runtime[0][key] for key in ("size", "sha256")}
    if target == "native":
        declared = LinkInputObservation.from_record(link)
        contents = {
            path: {"size": size, "sha256": digest}
            for path, size, digest in declared.inputs
        }
        consumed_runtime = contents[dict(declared.roles)["runtime"]]
    else:
        consumed_runtime = link["source_inputs"]["runtime"]
        if "app" in link["links"]:
            shared = [
                row
                for row in runtime_cell["files"]
                if row["role"] == "wasm_shared_member"
            ]
            if len(shared) != 1 or link["source_inputs"]["deployment_runtime"] != {
                key: shared[0][key] for key in ("size", "sha256")
            }:
                raise ValueError(
                    "WASM deployment consumed another installed runtime member"
                )
    if consumed_runtime != expected_runtime:
        raise ValueError("linker consumed another installed runtime generation")
    verify_stable_regular_file_identity(
        identity, label="compilation output archive", hash_content=True
    )
    return {
        "archive": {"size": identity.size, "sha256": identity.sha256},
        "command": result,
        "observation": observation,
        "preparation": preparation,
        "artifact": {
            "path": projected[primary_role].relative_to(output).as_posix(),
            **execution_root.file_identity(projected[primary_role]),
        },
        "publication": receipt,
        "files": execution_root.root_inventory(output),
    }


def _validate_transport(
    transport: object,
    *,
    sealed_root: dict[str, Any],
    arch: str,
    projection: SealedExecutionProjection,
) -> None:
    """One retained systemd import/teardown authority for both projections."""
    if (
        not isinstance(transport, dict)
        or set(transport)
        != {
            "archive_sha256",
            "execution",
            "root",
            "files",
            "nonce",
            "observation",
            "provider",
            "teardown",
        }
        or transport["archive_sha256"] != sealed_root["archive"]["sha256"]
        or transport["files"] != sealed_root["files"]
        or transport["root"] != f"/run/molt-release-{transport['nonce']}/rootfs"
    ):
        raise ValueError("standalone transport archive binding differs")
    if SealedExecutionProjection.from_dict(transport["execution"]) != projection:
        raise ValueError("standalone import execution projection differs")
    validate_sealed_systemd_provider(transport["provider"], arch=arch)
    imported = validate_sealed_systemd_import(
        transport["observation"],
        nonce=transport["nonce"],
        sha256=transport["archive_sha256"],
        root=transport["root"],
    )
    validate_sealed_systemd_stop(
        transport["teardown"], unit=imported["Id"], invocation=imported["InvocationID"]
    )


def _validate_compilation_plan(
    *,
    rootfs: Path,
    compilation: dict[str, Any],
    lanes: tuple[ReleaseLane, ...],
    python_minor: str,
    launcher: list[str],
    controller_python: Path,
) -> None:
    """Reconstruct controller commands/policies before granting any execution.

    A sealed arbitrary Python controller can forge summaries while separately
    executing the admitted compiler. Sealing is insufficient: these exact source
    bytes and every cell's command/configuration must be the owning producer.
    """
    from .release_authority import consumer_guest_command, consumer_guest_source
    from tools.proof_queue_pkg.execution_environment import (
        _deterministic_execution_environment,
        _environment_name_class,
    )
    from molt.process_environment import NameRule

    if (
        compilation.get("schema") != "molt.installed-compilation.v4"
        or compilation.get("python") != python_minor
        or compilation.get("launcher") != launcher
        or compilation.get("controller_python") != controller_python.as_posix()
        or [row.get("lane") for row in compilation["cells"]]
        != [lane.as_record() for lane in lanes]
        or len({lane.id for lane in lanes}) != len(lanes)
        or not lanes
    ):
        raise ValueError("compilation plan omits or substitutes a requested cell")
    projection = SealedExecutionProjection.from_dict(compilation["execution"])
    source = consumer_guest_source(python_minor).encode()
    for name, expected in (
        ("app/source.py", source),
        ("app/compilation-controller.py", _COMPILATION_CONTROLLER.encode()),
    ):
        observed = execution_root.file_identity(rootfs / name)
        _equal_artifact(
            observed,
            {"size": len(expected), "sha256": hashlib.sha256(expected).hexdigest()},
        )
    if compilation["source_sha256"] != hashlib.sha256(source).hexdigest():
        raise ValueError("compilation source belongs to another requested program")
    compiler_path = (
        PurePosixPath("/")
        / execution_root.linux_root_relative_path(compilation["compiler_source_root"])
    ).parent / compilation["compiler"]["path"]
    executable = compilation["controller_python"]
    images = []
    for row in execution_root.root_inventory(rootfs):
        if row["mode"] != 0o555 or row["path"] == "bin/molt-proof-supervisor":
            continue
        path = "/" + row["path"]
        role = (
            "compile-controller"
            if path == executable
            else "compiler"
            if path == compiler_path.as_posix()
            else "compile-tool"
        )
        images.append(
            {
                "role": role,
                "path": path,
                "sha256": row["sha256"],
                "root_exit_disposition": "terminate"
                if role == "compiler"
                else "require-exit",
            }
        )
    seed_files = [
        row
        for row in execution_root.root_inventory(rootfs)
        if row["path"].startswith("app/cache/")
    ]
    if seed_files != compilation["cache_seed"] or bool(seed_files) != bool(
        compilation["origins"]
    ):
        raise ValueError(
            "compilation cache seed differs from retained immutable inputs"
        )
    if any(
        len(PurePosixPath(row["path"]).parts) < 4
        or PurePosixPath(row["path"]).parts[2] not in {lane.id for lane in lanes}
        for row in seed_files
    ):
        raise ValueError("compilation cache seed names another lane")
    nonces = set()
    for lane, cell in zip(lanes, compilation["cells"], strict=True):
        policy_path = "policies/compilation-" + lane.id + ".json"
        config_path = "app/compilation-" + lane.id + ".json"
        if cell["id"] != lane.id or cell["policy"] != policy_path:
            raise ValueError("compilation cell policy coordinate differs")
        output = (
            "/work/output/program.wasm"
            if lane.target == "wasm"
            else "/work/output/program"
        )
        command = consumer_guest_command(
            compilation["launcher"],
            lane=lane,
            python_minor=python_minor,
            diagnostics="/work/output/diagnostics.json",
            output=output,
            source="/app/source.py",
            capture_link_inputs=True,
        )
        if cell["command"] != command:
            raise ValueError("compilation cell uses a different public command")
        config = read_exact(
            rootfs / config_path,
            max_bytes=1024 * 1024,
            label="compilation controller configuration",
        )
        if not isinstance(config, dict) or set(config) != {
            "command",
            "environment",
            "timeout",
            "output_max_bytes",
            "cache_seed",
            "cache_contract",
            "compiler_source_root",
        }:
            raise ValueError("compilation controller configuration fields differ")
        env = config["environment"]
        if (
            not isinstance(env, dict)
            or any(
                not isinstance(k, str) or not isinstance(v, str) for k, v in env.items()
            )
            or any(
                _environment_name_class(name, rule=NameRule.EXACT)
                == "queue-owned-custody"
                for name in env
            )
        ):
            raise ValueError("compilation environment carries foreign custody")
        classified, _ = _deterministic_execution_environment(env, override_names=())
        if (
            dict(classified) != env
            or env.get("MOLT_INSTALLED_RUNTIME_STORE") != _COMPILATION_RUNTIME_STORE
            or any(env.get(name) != value for name, value in lane.environment().items())
        ):
            raise ValueError("compilation environment differs from its declared policy")
        if config != {
            "command": command,
            "environment": env,
            "timeout": projection.runtime_max_seconds - 10,
            "output_max_bytes": projection.stdout_max_bytes,
            "cache_seed": "/app/cache/" + lane.id,
            "cache_contract": _compilation_cache_contract(lane, cell["runtime_cell"]),
            "compiler_source_root": compilation["compiler_source_root"],
        }:
            raise ValueError(
                "compilation controller command or resource policy differs"
            )
        policy = read_exact(
            rootfs / policy_path,
            max_bytes=16 * 1024 * 1024,
            label="compilation native policy",
        )
        nonce = policy.get("nonce")
        from molt.backend_protocol import validate_emission_scope

        if (
            validate_emission_scope(nonce) is None
            or nonce in nonces
            or env.get("MOLT_EMISSION_SCOPE") != nonce
        ):
            raise ValueError(
                "compilation policy nonce is absent, repeated or differs from emission scope"
            )
        nonces.add(nonce)
        expected = {
            "schema": supervisor_custody.SUPERVISOR_POLICY_SCHEMA,
            "nonce": nonce,
            "mode": "declared-tree",
            "cwd": "/work",
            "command": [
                executable,
                "-I",
                "-B",
                "/app/compilation-controller.py",
                "/" + config_path,
            ],
            "environment": dict(ENVIRONMENT),
            "root_role": "compile-controller",
            "derived_roots": [],
            "fixed_images": images,
        }
        if policy != expected:
            raise ValueError(
                "compilation policy permits another producer or process family"
            )
