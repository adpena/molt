"""One sealed post-uninstall replay and retained-evidence contract for v7."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path, PurePosixPath
import secrets
import tempfile
from typing import Any

from molt.browser_asset_closure import (
    NODE_RUNNER_ENTRY_ASSETS,
    wasm_loader_asset_payloads,
)
from molt.exact_json import (
    canonical_json_sha256,
    capture_exact,
    encode_exact,
    loads_exact,
    read_exact,
)
from molt.target_python import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
from molt.toolchain_identity import (
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)
from tools.cross_run import (
    DockerTransport,
    Host,
    validate_sealed_docker_configuration,
    validate_sealed_docker_image,
    validate_sealed_docker_provider,
)
from tools.proof_queue_pkg import supervisor_custody, supervisor_generation
from . import execution_root
from .archive import extract_zip_strict, write_reproducible_zip, same_regular_file_bytes
from .release_model import ROOT, validate_artifact_record, write_json

SCHEMA = "molt.release-consumer-replay.v1"
ENVIRONMENT = {"PATH": "/absent", "HOME": "/absent", "LANG": "C", "LC_ALL": "C"}


def provision_verifier() -> tuple[Path, dict[str, object]]:
    """Development-side tool selection uses the existing Cargo freshness owner."""
    binary, telemetry = supervisor_generation.provision(cwd=ROOT, env=os.environ)
    generation = supervisor_generation.read_generation(telemetry)
    return binary, generation


def _cell_id(owner: str, minor: str, target: str, profile: str) -> str:
    if (
        owner not in {"bundle", "pip"}
        or minor not in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
        or target not in {"native", "wasm"}
        or profile not in {"dev", "release"}
    ):
        raise ValueError("consumer replay coordinate is invalid")
    return f"{owner}-{minor}-{target}-{profile}"


def expected_runs(
    proofs: list[dict[str, Any]], pip_proof: dict[str, Any]
) -> list[dict[str, Any]]:
    rows = [
        {
            "id": _cell_id("bundle", proof["python"], cell["target"], cell["profile"]),
            "owner": "bundle",
            "python": proof["python"],
            "target": cell["target"],
            "profile": cell["profile"],
            "artifact": cell["artifact"],
            "manifest": cell["manifest"],
        }
        for proof in proofs
        for cell in proof["cells"]
    ]
    rows.append(
        {
            "id": _cell_id("pip", pip_proof["python"], "native", "release"),
            "owner": "pip",
            "python": pip_proof["python"],
            "target": "native",
            "profile": "release",
            "artifact": pip_proof["artifact"],
            "manifest": None,
        }
    )
    return rows


def guest_command(row: dict[str, Any], argv: tuple[str, ...]) -> list[str]:
    prefix = f"/app/{row['id']}"
    if row["target"] == "native":
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
) -> dict[str, Any]:
    platform, arch = candidate["target"]["platform"], candidate["target"]["arch"]
    if platform != "linux":
        raise ValueError(
            f"sealed standalone replay has no qualified {platform}/{arch} filesystem adapter; host execution is forbidden"
        )
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
    rows = expected_runs(proofs, pip_proof)
    executable_paths = ["bin/node", "bin/molt-proof-supervisor"]
    for row in rows:
        prefix = f"app/{row['id']}/"
        if row["target"] == "native":
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
        image = command[0].removeprefix("/")
        nonce = secrets.token_hex(32)
        policy_path = f"policies/{row['id']}.json"
        image_identity = next(item for item in admitted_files if item["path"] == image)
        policy = _policy(command, image_identity["sha256"], nonce)
        admitted_files.append(
            execution_root.write_payload(
                rootfs,
                policy_path,
                encode_exact(policy),
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
    transport = DockerTransport(
        Host(
            name="release-standalone",
            target=replay["target"]["arch"] + "-unknown-linux-gnu",
            transport="docker",
        )
    )
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
        transport.remove_sealed_root()
    if execution_root.root_inventory(evidence / "rootfs") != replay["root"]["files"]:
        raise ValueError("sealed standalone input bytes changed during execution")
    replay["files"] = execution_root.root_inventory(evidence)
    write_json(evidence / "replay.json", replay)
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
        or receipt.get("complete") is not True
        or receipt.get("state") != "COMPLETE"
        or receipt.get("root_exit_code") != 0
        or receipt.get("error_count") != 0
        or receipt.get("violation_count") != 0
        or receipt.get("errors") != []
        or receipt.get("violations") != []
        or receipt.get("accounting", {}).get("active_processes") != 0
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
) -> None:
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
        row
        for row in execution_root.root_inventory(evidence)
        if row["path"] != "replay.json"
    ]
    if actual_files != replay["files"]:
        raise ValueError("standalone evidence file closure changed")
    retained_files = {row["path"]: row for row in replay["files"]}
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
    if (
        not isinstance(transport, dict)
        or set(transport) != {"archive_sha256", "image", "docker"}
        or transport["archive_sha256"] != replay["root"]["archive"]["sha256"]
    ):
        raise ValueError("standalone transport archive binding differs")
    validate_sealed_docker_image(
        transport["image"], sha256=transport["archive_sha256"], arch=arch
    )
    docker = transport["docker"]
    if (
        not isinstance(docker, dict)
        or set(docker) != {"path", "sha256", "endpoint", "provider"}
        or docker["endpoint"] != "unix:///var/run/docker.sock"
    ):
        raise ValueError("standalone transport provider binding differs")
    validate_sealed_docker_provider(docker["provider"], arch=arch)
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
    expected = expected_runs(proofs, pip_proof)
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
            raise ValueError("standalone replay coordinate/artifact binding differs")
        command = guest_command(wanted, argv)
        policy = f"policies/{row['id']}.json"
        if (
            row["command"] != command
            or row["policy"] != policy
            or row["nonce"] in nonces
        ):
            raise ValueError("standalone replay command/policy binding differs")
        nonces.add(row["nonce"])
        if wanted["target"] == "native":
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
        image = rootfs / command[0].removeprefix("/")
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
            "before",
            "after",
            "stdout",
            "stderr",
            "receipt",
            "guest_stderr_sha256",
        }:
            raise ValueError("standalone execution record fields are invalid")
        supervisor_command = [
            "/bin/molt-proof-supervisor",
            "run-export",
            "--policy",
            "/" + policy,
            "--receipt",
            "/evidence/receipt.json",
        ]
        image_id = replay["transport"]["image"]["Id"]
        validate_sealed_docker_configuration(
            execution["before"], image=image_id, command=supervisor_command
        )
        after = execution["after"]
        validate_sealed_docker_configuration(
            after, image=image_id, command=supervisor_command
        )
        if (
            after.get("Id") != execution["before"].get("Id")
            or after.get("State", {}).get("Running") is not False
            or after["State"].get("Pid") != 0
            or after["State"].get("ExitCode") != 0
            or after["State"].get("OOMKilled") is not False
        ):
            raise ValueError("standalone container termination is incomplete")
        nonce = execution["before"]["Config"]["Labels"].get("org.molt.release-custody")
        if (
            not isinstance(nonce, str)
            or len(nonce) != 32
            or any(c not in "0123456789abcdef" for c in nonce)
        ):
            raise ValueError("standalone transport nonce is invalid")
        if (
            execution["stdout"] != f"transport/{nonce}.stdout"
            or execution["stderr"] != f"transport/{nonce}.stderr"
            or after["Config"]["Labels"].get("org.molt.release-custody") != nonce
        ):
            raise ValueError("standalone streams belong to another container")
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
            evidence / execution["stderr"], expected=retained_files[execution["stderr"]]
        )
        retained_receipt = _artifact_bytes(
            receipt,
            retained_files[execution["receipt"]],
            max_bytes=65536,
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
            or hashlib.sha256(prefix).hexdigest() != execution["guest_stderr_sha256"]
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


def publish_archive(
    *, candidate_dir: Path, receipt: Path, candidate: dict[str, Any]
) -> Path:
    """Retain proof inputs with the existing bounded reproducible ZIP owner."""
    if receipt != candidate_dir / "consumer-verification.json":
        raise ValueError("consumer proof must publish beside its candidate")
    output = candidate_dir / archive_filename(candidate)
    with tempfile.TemporaryDirectory(prefix="molt-consumer-retention-") as raw:
        staged = Path(raw).resolve()
        execution_root.stage_file(
            staged, "candidate.json", candidate_dir / "candidate.json", executable=False
        )
        execution_root.stage_file(
            staged, "consumer-verification.json", receipt, executable=False
        )
        for row in execution_root.root_inventory(candidate_dir / "consumer-evidence"):
            captured = execution_root.stage_file(
                staged,
                "consumer-evidence/" + row["path"],
                candidate_dir / "consumer-evidence" / row["path"],
                executable=bool(row["mode"] & 0o111),
                max_bytes=row["size"],
            )
            _equal_artifact(captured, row)
        write_reproducible_zip(
            staged, output, source_date_epoch=candidate["source_date_epoch"]
        )
    return output


def admit_archive(
    *, candidate_dir: Path, candidate: dict[str, Any], supervisor: Path
) -> dict[str, Any]:
    """Replay the bytes actually shipped, without producer paths or containers."""
    from .release_authority import validate_consumer_proof

    archive = candidate_dir / archive_filename(candidate)
    with tempfile.TemporaryDirectory(prefix="molt-consumer-admission-") as raw:
        extracted = Path(raw).resolve() / "consumer"
        identity = extract_zip_strict(archive, extracted)
        archived_candidate = read_exact(
            extracted / "candidate.json",
            max_bytes=4 * 1024 * 1024,
            label="archived candidate",
        )
        if archived_candidate != candidate:
            raise ValueError("consumer archive candidate differs from admitted source")
        for name in ("candidate.json", "consumer-verification.json"):
            if not same_regular_file_bytes(extracted / name, candidate_dir / name):
                raise ValueError(
                    "consumer archive differs from admitted candidate/proof"
                )
        if {path.name for path in extracted.iterdir()} != {
            "candidate.json",
            "consumer-verification.json",
            "consumer-evidence",
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
