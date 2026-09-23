"""Native process-supervisor policy, receipt, and custody authority."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import sys
import tempfile
import time
from typing import Mapping, Sequence

from molt.dx import checkout_custody
from molt.path_custody import host_path_is_within
from molt.file_publication import durable_publish_directory_exclusive
from molt import dx as _dx

from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import execution_custody
from tools.proof_queue_pkg import process_image_capture


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(
        path,
        (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode(),
    )


SUPERVISOR_SOURCE_ROOT = admission._REPO_ROOT / "tools" / "proof_supervisor"
SUPERVISOR_CACHE_DIRNAME = "proof-supervisor"
SUPERVISOR_IDENTITY_SCHEMA = "molt.proof-supervisor-identity.v1"
SUPERVISOR_BINARY_NAME = "molt-proof-supervisor" + (".exe" if os.name == "nt" else "")
_SUPERVISOR_SOURCE_FILES = ("Cargo.toml", "Cargo.lock")


def _supervisor_source_files() -> list[Path]:
    files = [SUPERVISOR_SOURCE_ROOT / name for name in _SUPERVISOR_SOURCE_FILES]
    files.extend(sorted((SUPERVISOR_SOURCE_ROOT / "src").rglob("*.rs")))
    return files


def _rustc_identity(env: Mapping[str, str]) -> dict[str, str]:
    completed = command_identity._run_captured(
        ("rustc", "-vV"), cwd=SUPERVISOR_SOURCE_ROOT, env=env, timeout=60.0
    )
    if completed.returncode != 0:
        raise ValueError(
            "native proof supervisor toolchain probe failed: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    fields = {
        key.strip(): value.strip()
        for line in completed.stdout.splitlines()
        if ":" in line
        for key, value in (line.split(":", 1),)
    }
    return {
        "host": fields.get("host", ""),
        "release": fields.get("release", ""),
        "commit_hash": fields.get("commit-hash", ""),
    }


def supervisor_source_identity(env: Mapping[str, str]) -> dict[str, object]:
    """Content identity of the supervisor build: exact sources plus toolchain."""
    digest = hashlib.sha256()
    sources: list[dict[str, object]] = []
    for path in _supervisor_source_files():
        relative = path.relative_to(SUPERVISOR_SOURCE_ROOT).as_posix()
        data = path.read_bytes()
        file_digest = hashlib.sha256(data).hexdigest()
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(file_digest.encode())
        digest.update(b"\0")
        sources.append(
            {"path": relative, "sha256": file_digest, "size_bytes": len(data)}
        )
    rustc = _rustc_identity(env)
    for key in sorted(rustc):
        digest.update(f"{key}={rustc[key]}".encode())
        digest.update(b"\0")
    digest.update(b"profile=release\0")
    return {
        "schema": SUPERVISOR_IDENTITY_SCHEMA,
        "identity": digest.hexdigest(),
        "sources": sources,
        "rustc": rustc,
        "profile": "release",
        "binary_name": SUPERVISOR_BINARY_NAME,
    }


def supervisor_cache_root(
    env: Mapping[str, str], *, source_root: Path = admission._REPO_ROOT
) -> Path:
    """Shared, custody-external root for content-addressed supervisor binaries.

    A supervisor binary is derived from the queue's own checkout (its Rust
    source plus the Rust toolchain), never from the project a proof runs in,
    so the cache lives under that checkout's durable custody root and is
    shared by every proof root on the host. When that checkout is itself
    scratch (hosted fixtures) the cache moves to the host temp root so it
    never lands under admitted source.
    """
    source_root = Path(source_root).resolve()
    custody_root = checkout_custody(source_root, env, require_exists=False).custody_root
    if custody_root == source_root or host_path_is_within(custody_root, source_root):
        custody_root = Path(tempfile.gettempdir()) / "molt-custody"
    return custody_root / SUPERVISOR_CACHE_DIRNAME


def _cached_supervisor(cache_dir: Path) -> Path | None:
    binary = cache_dir / SUPERVISOR_BINARY_NAME
    manifest = cache_dir / "identity.json"
    if not binary.is_file() or not manifest.is_file():
        return None
    try:
        recorded = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(recorded, Mapping):
        return None
    if recorded.get("schema") != SUPERVISOR_IDENTITY_SCHEMA:
        return None
    if command_identity._file_identity(binary)["sha256"] != recorded.get(
        "binary_sha256"
    ):
        return None
    return binary


def _build_proof_supervisor(*, cwd: Path, env: Mapping[str, str]) -> Path:
    build = SUPERVISOR_SOURCE_ROOT / "build.py"
    completed = command_identity._run_captured(
        (sys.executable, str(build), "--release"),
        cwd=cwd,
        env=env,
        timeout=600.0,
    )
    if completed.returncode != 0:
        raise ValueError(
            "native proof supervisor provisioning failed: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    lines = [line.strip() for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise ValueError("native proof supervisor build returned no binary")
    binary = Path(lines[-1]).resolve(strict=True)
    if not binary.is_file():
        raise ValueError("native proof supervisor binary is unavailable")
    return binary


def _publish_supervisor(
    binary: Path, identity: Mapping[str, object], cache_dir: Path
) -> Path:
    cache_root = cache_dir.parent
    cache_root.mkdir(parents=True, exist_ok=True)
    staged = cache_root / f".staging-{cache_dir.name}-{secrets.token_hex(8)}"
    staged.mkdir()
    try:
        staged_binary = staged / SUPERVISOR_BINARY_NAME
        staged_binary.write_bytes(binary.read_bytes())
        staged_binary.chmod(0o755)
        _atomic_json(
            staged / "identity.json",
            {
                **identity,
                "binary_sha256": command_identity._file_identity(staged_binary)[
                    "sha256"
                ],
            },
        )
        try:
            durable_publish_directory_exclusive(staged, cache_dir)
        except FileExistsError:
            # A concurrent provisioner published the same identity first; its
            # bytes are content-equal by construction, so adopt them.
            pass
    finally:
        if staged.exists():
            for child in staged.iterdir():
                child.unlink()
            staged.rmdir()
    cached = _cached_supervisor(cache_dir)
    if cached is None:
        raise ValueError(f"published proof supervisor failed verification: {cache_dir}")
    return cached


def _provision_proof_supervisor(
    *, cwd: Path, env: Mapping[str, str]
) -> tuple[Path, dict[str, object]]:
    """Return the content-addressed supervisor binary, building only on a miss.

    Building inside every proof's own timed window made any fresh logs root
    (every test, every scratch queue) pay a cold cargo release build and time
    out. The binary is keyed by its exact sources plus rustc identity and
    published once into the shared custody-external cache.
    """
    started = time.perf_counter()
    identity = supervisor_source_identity(env)
    cache_dir = supervisor_cache_root(env) / str(identity["identity"])
    cached = _cached_supervisor(cache_dir)
    cache_state = "hit"
    if cached is None:
        cache_state = "miss"
        built = _build_proof_supervisor(cwd=cwd, env=env)
        cached = _publish_supervisor(built, identity, cache_dir)
    binary_identity = command_identity._file_identity(cached)
    target_dir = env.get("CARGO_TARGET_DIR")
    return cached, {
        "schema": "molt.proof-supervisor-provision-telemetry.v2",
        "build_s": time.perf_counter() - started,
        "cache": cache_state,
        "cache_dir": str(cache_dir),
        "source_identity": identity["identity"],
        "build_target_dir": (
            str(Path(target_dir).resolve(strict=True))
            if target_dir and Path(target_dir).exists()
            else None
        ),
        "build_output_sha256": binary_identity["sha256"],
        "build_output_size_bytes": binary_identity["size_bytes"],
    }


def prewarm_proof_supervisor(
    *, cwd: Path, env: Mapping[str, str] | None = None
) -> dict[str, object]:
    """Populate the shared cache ahead of proof execution (CI and local prepare)."""
    resolved_env = dict(os.environ if env is None else env)
    with tempfile.TemporaryDirectory(
        prefix="molt-proof-supervisor-prewarm-"
    ) as scratch:
        resolved_env.setdefault("CARGO_TARGET_DIR", scratch)
        _, telemetry = _provision_proof_supervisor(cwd=cwd, env=resolved_env)
    return telemetry


def _supervisor_fixed_images(
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    execution_command: Sequence[str],
    platform_process_images: Sequence[Mapping[str, object]] = (),
) -> tuple[str, list[dict[str, str]]]:
    root = os.path.normcase(os.path.abspath(execution_command[0]))
    identities: dict[str, tuple[str, str]] = {}
    images: dict[tuple[str, str], dict[str, str]] = {}

    def add(
        role: str,
        raw_path: object,
        raw_digest: object,
        raw_root_exit_disposition: object = None,
    ) -> None:
        if not isinstance(raw_path, str) or not isinstance(raw_digest, str):
            return
        if re.fullmatch(r"[0-9a-f]{64}", raw_digest) is None:
            return
        path = Path(raw_path)
        if not path.is_absolute() or not path.is_file():
            return
        key = os.path.normcase(os.path.abspath(path))
        disposition = (
            str(raw_root_exit_disposition)
            if raw_root_exit_disposition is not None
            else "require-exit"
        )
        if disposition not in {"require-exit", "terminate"}:
            raise ValueError(
                f"supervisor image has invalid root-exit disposition: {path}"
            )
        row = {"role": role, "path": str(path), "sha256": raw_digest}
        if disposition != "require-exit":
            row["root_exit_disposition"] = disposition
        identity = (raw_digest, disposition)
        prior_identity = identities.get(key)
        if prior_identity is not None and prior_identity[0] != raw_digest:
            raise ValueError(f"supervisor image has conflicting identities: {path}")
        if prior_identity is not None and prior_identity[1] != disposition:
            raise ValueError(
                f"supervisor image has conflicting root-exit dispositions: {path}"
            )
        identities[key] = identity
        images[(key, role)] = row

    root_path = Path(os.path.abspath(execution_command[0]))
    if not root_path.is_file():
        raise ValueError("supervisor root executable is unavailable")
    add("root-command", str(root_path), command_identity._hash_file(root_path))
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            continue
        for image in process_image_capture.toolchain_images(str(name), raw):
            add(
                str(image["role"]),
                image["path"],
                image["sha256"],
                image.get("root_exit_disposition"),
            )
    for name, raw in environment_executables.items():
        if not isinstance(raw, Mapping):
            continue
        executable = raw.get("executable")
        if isinstance(executable, Mapping):
            add(f"env:{name}", executable.get("path"), executable.get("sha256"))
    for image in platform_process_images:
        add(
            str(image.get("role") or "platform-process"),
            image.get("path"),
            image.get("sha256"),
            image.get("root_exit_disposition"),
        )
    if root not in identities:
        raise ValueError("supervisor policy has no captured root executable image")
    return "root-command", [images[key] for key in sorted(images)]


BUILD_OUTPUT_ROLE = "build-output"
SCRATCH_OUTPUT_ROLE = "scratch-output"
ATTESTED_ENVIRONMENT_ROLE = "attested-environment"
PROOF_SCRATCH_ROOT_ENV = _dx.PROOF_SCRATCH_ROOT_ENV
# Run-owned derived roots the queue creates fresh for every declared-tree
# proof, named by the environment variable the proof reads them from.
RUN_OWNED_DERIVED_ROOTS = (
    (BUILD_OUTPUT_ROLE, "CARGO_TARGET_DIR"),
    (SCRATCH_OUTPUT_ROLE, PROOF_SCRATCH_ROOT_ENV),
)


def _supervisor_derived_roots(
    *,
    descendants: object,
    env: Mapping[str, str],
    derived_environments: Sequence[Mapping[str, object]] = (),
) -> list[dict[str, str]]:
    if descendants == "forbidden":
        if derived_environments:
            raise ValueError("a leaf closure cannot launch from derived environments")
        return []
    roots: list[dict[str, str]] = []
    for row in derived_environments:
        path = Path(str(row.get("root") or ""))
        if not path.is_absolute() or not path.is_dir():
            raise ValueError(
                "declared derived environment root must be an existing absolute "
                f"directory: {path}"
            )
        roots.append(
            {"role": ATTESTED_ENVIRONMENT_ROLE, "path": str(path.resolve(strict=True))}
        )
    for role, name in RUN_OWNED_DERIVED_ROOTS:
        raw = env.get(name)
        if not raw:
            continue
        path = Path(raw)
        if not path.is_absolute() or not path.is_dir():
            raise ValueError(
                f"declared-tree supervisor requires existing absolute {name}"
            )
        roots.append({"role": role, "path": str(path.resolve(strict=True))})
    return roots


def derived_root_row_consistent(row: Mapping[str, object]) -> bool:
    """Whether a prelaunch derived-root row states its role's invariant.

    A build-output root is run-owned and was empty at launch. An attested
    environment root is shared, content-addressed custody: its launch-time
    listing is recorded so the receipt names which environments pre-existed.
    """
    role = row.get("role")
    if role in {BUILD_OUTPUT_ROLE, SCRATCH_OUTPUT_ROLE}:
        return (
            row.get("run_owned") is True
            and row.get("initial_entry_count") == 0
            and row.get("initial_manifest_sha256") == _canonical_payload_sha256([])
        )
    if role == ATTESTED_ENVIRONMENT_ROLE:
        entries = row.get("initial_entries")
        return (
            row.get("run_owned") is False
            and isinstance(entries, list)
            and all(isinstance(entry, str) for entry in entries)
            and entries == sorted(entries)
            and row.get("initial_entry_count") == len(entries)
            and row.get("initial_manifest_sha256") == _canonical_payload_sha256(entries)
        )
    return False


def _derived_root_provenance(
    *,
    descendants: object,
    env: Mapping[str, str],
    source_root: Path,
    result_path: Path,
    derived_environments: Sequence[Mapping[str, object]] = (),
) -> list[dict[str, object]]:
    roots = _supervisor_derived_roots(
        descendants=descendants,
        env=env,
        derived_environments=derived_environments,
    )
    source = source_root.resolve(strict=True)
    cas_root = Path(os.path.abspath(result_path.parent / "custody-cas"))
    admitted: list[dict[str, object]] = []
    for row in roots:
        path = Path(row["path"])
        lexical = Path(os.path.abspath(path))
        resolved = lexical.resolve(strict=True)
        if os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)):
            raise ValueError("derived executable root may not traverse a symlink")
        if (
            resolved == source
            or resolved.is_relative_to(source)
            or source.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps admitted source")
        if (
            resolved == cas_root
            or resolved.is_relative_to(cas_root)
            or cas_root.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps proof custody CAS")
        if resolved == Path(os.path.abspath(result_path)):
            raise ValueError("derived executable root overlaps terminal result")
        entries = sorted(entry.name for entry in resolved.iterdir())
        if row["role"] == ATTESTED_ENVIRONMENT_ROLE:
            admitted.append(
                {
                    **row,
                    "initial_entry_count": len(entries),
                    "initial_entries": entries,
                    "initial_manifest_sha256": _canonical_payload_sha256(entries),
                    "run_owned": False,
                }
            )
            continue
        if entries:
            raise ValueError(
                f"derived executable root is not fresh and empty: {resolved}"
            )
        admitted.append(
            {
                **row,
                "initial_entry_count": 0,
                "initial_manifest_sha256": _canonical_payload_sha256([]),
                "run_owned": True,
            }
        )
    return admitted


def _supervisor_policy(
    *,
    envelope: Mapping[str, object],
    execution_command: Sequence[str],
    execution_env: Mapping[str, str],
    cwd: Path,
    nonce: str,
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    platform_process_images: Sequence[Mapping[str, object]],
    derived_environments: Sequence[Mapping[str, object]] = (),
) -> dict[str, object]:
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no supervisor closure authority")
    descendants = closure.get("descendants")
    mode = "leaf" if descendants == "forbidden" else "declared-tree"
    root_role, fixed_images = _supervisor_fixed_images(
        toolchains,
        environment_executables,
        execution_command,
        platform_process_images,
    )
    return {
        "schema": "molt.proof-process-closure.v2",
        "nonce": nonce,
        "mode": mode,
        "cwd": str(cwd.resolve(strict=True)),
        "command": [str(value) for value in execution_command],
        "environment": dict(
            sorted(execution_env.items(), key=lambda item: item[0].casefold())
        ),
        "root_role": root_role,
        "fixed_images": fixed_images,
        "derived_roots": _supervisor_derived_roots(
            descendants=descendants,
            env=execution_env,
            derived_environments=derived_environments,
        ),
    }


def _validated_supervisor_receipt(
    *,
    binary: Path,
    policy_path: Path,
    receipt_path: Path,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    verified = command_identity._run_captured(
        (
            str(binary),
            "verify",
            "--policy",
            str(policy_path),
            "--receipt",
            str(receipt_path),
        ),
        cwd=cwd,
        env=env,
    )
    if verified.returncode != 0:
        raise ValueError(
            "native proof supervisor receipt verification failed: "
            + (verified.stderr.strip() or verified.stdout.strip())
        )
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native proof supervisor returned no readable receipt"
        ) from exc
    if not isinstance(receipt, dict):
        raise ValueError("native proof supervisor receipt is not an object")
    return receipt


def capture_process_image_inventory(
    *,
    binary: Path,
    role: str,
    executable: Path,
    probe_args: Sequence[str],
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Observe one bounded toolchain probe without granting proof authority."""

    if not probe_args or not all(
        isinstance(value, str) and value for value in probe_args
    ):
        raise ValueError("process-image probe arguments must be non-empty strings")
    launcher = process_image_capture.capture_image(
        f"{role}-launcher", executable, preserve_path=True
    )
    command = [str(executable.resolve(strict=True)), *probe_args]
    with tempfile.TemporaryDirectory(prefix="molt-process-image-inventory-") as raw:
        root = Path(raw).resolve()
        policy_path = root / "policy.json"
        receipt_path = root / "receipt.json"
        policy = {
            "schema": "molt.proof-process-closure.v2",
            "nonce": secrets.token_hex(32),
            "mode": "inventory-tree",
            "cwd": str(cwd.resolve(strict=True)),
            "command": command,
            "environment": dict(
                sorted(env.items(), key=lambda item: item[0].casefold())
            ),
            "root_role": launcher["role"],
            "fixed_images": [
                {
                    "role": launcher["role"],
                    "path": launcher["path"],
                    "sha256": launcher["sha256"],
                }
            ],
            "derived_roots": [],
        }
        _atomic_json(policy_path, policy)
        completed = command_identity._run_captured(
            (
                str(binary),
                "inventory",
                "--policy",
                str(policy_path),
                "--receipt",
                str(receipt_path),
            ),
            cwd=cwd,
            env=env,
            timeout=30.0,
        )
        if completed.returncode != 0:
            detail = completed.stderr.strip() or completed.stdout.strip()
            if receipt_path.is_file():
                try:
                    failed_receipt = json.loads(
                        receipt_path.read_text(encoding="utf-8")
                    )
                except (OSError, json.JSONDecodeError):
                    failed_receipt = None
                if isinstance(failed_receipt, Mapping):
                    diagnostics = [
                        str(value)
                        for field in ("errors", "violations")
                        for value in failed_receipt.get(field, [])
                    ]
                    if diagnostics:
                        detail = "; ".join(diagnostics)
            raise ValueError(
                f"{role} process-image inventory failed: {detail or completed.returncode}"
            )
        receipt = _validated_supervisor_receipt(
            binary=binary,
            policy_path=policy_path,
            receipt_path=receipt_path,
            cwd=cwd,
            env=env,
        )
        if (
            receipt.get("complete") is not True
            or receipt.get("state") != "COMPLETE"
            or receipt.get("root_exit_code") != 0
            or receipt.get("errors") != []
            or receipt.get("violations") != []
        ):
            raise ValueError(f"{role} process-image inventory is incomplete")
        descriptor = receipt.get("event_log")
        if not isinstance(descriptor, Mapping):
            raise ValueError(f"{role} process-image inventory has no event log")
        file_name = descriptor.get("file")
        if not isinstance(file_name, str) or Path(file_name).name != file_name:
            raise ValueError(f"{role} process-image inventory event path is invalid")
        event_path = receipt_path.with_name(file_name).resolve(strict=True)
        rows: list[dict[str, object]] = []
        launcher_path = Path(str(launcher["path"]))
        for line in event_path.read_text(encoding="utf-8").splitlines():
            try:
                event = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(
                    f"{role} process-image inventory event is malformed"
                ) from exc
            image = event.get("image") if isinstance(event, Mapping) else None
            if not isinstance(image, Mapping):
                continue
            raw_path = image.get("path")
            digest = image.get("sha256")
            size = image.get("size_bytes")
            if (
                not isinstance(raw_path, str)
                or not isinstance(digest, str)
                or not isinstance(size, int)
            ):
                raise ValueError(
                    f"{role} process-image inventory identity is malformed"
                )
            observed = Path(raw_path)
            try:
                is_launcher = observed.samefile(launcher_path)
            except OSError as exc:
                raise ValueError(
                    f"{role} process-image inventory image is unavailable: {observed}"
                ) from exc
            captured = process_image_capture.capture_image(
                f"{role}-launcher" if is_launcher else f"{role}-runtime",
                observed,
            )
            if captured["sha256"] != digest or captured["size_bytes"] != size:
                raise ValueError(
                    f"{role} process-image inventory changed before capture: {observed}"
                )
            rows.append(captured)
        images = process_image_capture.canonical_images(rows)
        if not any(
            Path(str(image["path"])).samefile(launcher_path) for image in images
        ):
            raise ValueError(f"{role} process-image inventory omitted its launcher")
        telemetry = {
            "schema": "molt.proof-process-image-inventory.v1",
            "probe_argv_sha256": _canonical_payload_sha256(command),
            "stdout_sha256": hashlib.sha256(completed.stdout.encode()).hexdigest(),
            "stderr_sha256": hashlib.sha256(completed.stderr.encode()).hexdigest(),
            "observed_image_count": len(images),
            "receipt_identity_sha256": receipt.get("identity_sha256"),
        }
        return images, telemetry


def _publish_supervisor_event_artifact(
    *, receipt_path: Path, receipt: Mapping[str, object], cas_root: Path
) -> dict[str, object]:
    event_log = receipt.get("event_log")
    if not isinstance(event_log, Mapping):
        raise ValueError("native proof supervisor receipt has no event artifact")
    file_name = event_log.get("file")
    expected_sha256 = event_log.get("sha256")
    expected_bytes = event_log.get("bytes")
    expected_count = event_log.get("count")
    if (
        not isinstance(file_name, str)
        or Path(file_name).name != file_name
        or not isinstance(expected_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", expected_sha256) is None
        or not isinstance(expected_bytes, int)
        or isinstance(expected_bytes, bool)
        or expected_bytes < 0
        or not isinstance(expected_count, int)
        or isinstance(expected_count, bool)
        or expected_count < 0
    ):
        raise ValueError(
            "native proof supervisor event artifact descriptor is malformed"
        )
    event_path = receipt_path.with_name(file_name).resolve(strict=True)
    if event_path.parent != receipt_path.parent.resolve(strict=True):
        raise ValueError(
            "native proof supervisor event artifact escaped its receipt directory"
        )
    digest = hashlib.sha256()
    size = 0
    count = 0
    final_byte = None
    with event_path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
            count += chunk.count(b"\n")
            final_byte = chunk[-1]
    if (
        digest.hexdigest() != expected_sha256
        or size != expected_bytes
        or count != expected_count
        or (size > 0 and final_byte != ord("\n"))
    ):
        raise ValueError("native proof supervisor event artifact identity changed")
    artifact = custody_cas.put_file(
        cas_root, event_path, logical_name=file_name, executable=False
    ).as_dict()
    if (
        artifact.get("sha256") != expected_sha256
        or artifact.get("size_bytes") != expected_bytes
    ):
        raise ValueError("durable supervisor event artifact identity mismatch")
    return {
        "schema": "molt.proof-process-event-artifact.v1",
        "artifact": artifact,
        "count": expected_count,
        "bytes": expected_bytes,
        "sha256": expected_sha256,
    }


def _canonical_payload_sha256(payload: object) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _publish_live_custody_receipt(
    receipt: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    events = receipt.get("events")
    apparatus_events = receipt.get("apparatus_events")
    errors = receipt.get("errors")
    lifecycle = receipt.get("lifecycle")
    state = receipt.get("state")
    if (
        not isinstance(events, list)
        or not isinstance(apparatus_events, list)
        or not isinstance(errors, list)
        or not isinstance(lifecycle, list)
    ):
        raise ValueError("live custody receipt event authority is malformed")
    artifact_payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": "live-input-custody-events",
        "events": events,
        "apparatus_events": apparatus_events,
        "errors": errors,
    }
    artifact = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    if receipt.get("identity_sha256") != execution_custody.live_custody_identity_sha256(
        events=events,
        apparatus_events=apparatus_events,
        errors=errors,
        state=state,
        lifecycle=lifecycle,
    ):
        raise ValueError("live custody receipt identity is inconsistent")
    return {
        **{
            key: value
            for key, value in receipt.items()
            if key not in {"events", "apparatus_events", "errors"}
        },
        "event_artifact": artifact,
        "event_count": len(events),
        "error_count": len(errors),
    }


def execution_custody_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    custody_context = dict(context)
    for field in (
        "execution_custody_sha256",
        "guard_receipt",
        "terminal_evidence_sha256",
    ):
        custody_context.pop(field, None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": custody_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": custody_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def terminal_evidence_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    terminal_context = dict(context)
    terminal_context.pop("terminal_evidence_sha256", None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": terminal_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": terminal_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
