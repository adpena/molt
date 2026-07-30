from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
from typing import Any, BinaryIO


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.cli.native_link_manifest import (  # noqa: E402
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
)
from molt.cli.runtime_paths import _runtime_lib_archive_name  # noqa: E402
from molt.cli.runtime_native_build import (  # noqa: E402
    _prepare_native_runtime_build,
)
from molt.cli.static_archive_identity import (  # noqa: E402
    StaticArchiveIdentityError,
    artifact_content_identity,
)
from molt.file_hashing import _sha256_file  # noqa: E402
from tools.artifact_publish import (  # noqa: E402
    fsync_file,
    publish_validated_outputs,
    staged_output_path,
)


SCHEMA_VERSION = 1
KIND = "molt_nightly_runtime_bundle"
MANIFEST_NAME = "nightly-runtime-manifest.json"
PROFILE = "dev-fast"
STDLIB_PROFILE = "full"
RUNTIME_ROLE = "runtime_archive"
LINK_ROLE = "native_link_manifest"
BACKEND_ROLE = "backend_executable"
_ROLES = (RUNTIME_ROLE, LINK_ROLE, BACKEND_ROLE)
_MAX_MANIFEST_BYTES = 1024 * 1024
_MAX_BUNDLE_PAYLOAD_BYTES = 2 * 1024 * 1024 * 1024
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_COMMIT_RE = re.compile(r"[0-9a-f]{40,64}")


class NightlyRuntimeBundleError(RuntimeError):
    """A Nightly runtime bundle is incomplete, corrupt, or identity-mismatched."""


@dataclass(frozen=True)
class BundleIdentity:
    source_commit: str
    platform_system: str
    platform_machine: str
    rustc_verbose: str
    cargo_version: str

    def __post_init__(self) -> None:
        if _COMMIT_RE.fullmatch(self.source_commit) is None:
            raise ValueError("source_commit must be a lowercase Git object id")
        if self.platform_system != "linux":
            raise ValueError("Nightly runtime bundles currently require Linux")
        if self.platform_machine not in {"x86_64", "aarch64"}:
            raise ValueError("Nightly runtime bundles require Linux x86_64 or aarch64")
        if not self.rustc_verbose.strip() or not self.cargo_version.strip():
            raise ValueError("toolchain identity must not be empty")

    def as_dict(self) -> dict[str, object]:
        return {
            "source_commit": self.source_commit,
            "platform": {
                "system": self.platform_system,
                "machine": self.platform_machine,
            },
            "toolchain": {
                "rustc_verbose": self.rustc_verbose,
                "cargo_version": self.cargo_version,
            },
        }


@dataclass(frozen=True)
class BundleInput:
    role: str
    source: Path
    archive_path: str
    mode: int


def _run_identity_command(
    argv: Sequence[str], *, cwd: Path, allow_empty: bool = False
) -> str:
    try:
        result = subprocess.run(
            list(argv),
            cwd=cwd,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="strict",
        )
    except (OSError, subprocess.CalledProcessError, UnicodeError) as exc:
        raise NightlyRuntimeBundleError(
            f"cannot establish bundle identity with {' '.join(argv)}: {exc}"
        ) from exc
    value = result.stdout.strip()
    if not value and not allow_empty:
        raise NightlyRuntimeBundleError(
            f"bundle identity command produced no output: {' '.join(argv)}"
        )
    return value


def collect_bundle_identity(project_root: Path) -> BundleIdentity:
    project_root = project_root.resolve(strict=True)
    status = _run_identity_command(
        ("git", "status", "--porcelain", "--untracked-files=no"),
        cwd=project_root,
        allow_empty=True,
    )
    if status:
        raise NightlyRuntimeBundleError(
            "refusing to publish a Nightly runtime bundle from tracked dirty sources"
        )
    source_commit = _run_identity_command(
        ("git", "rev-parse", "HEAD"), cwd=project_root
    )
    system = platform.system().lower()
    machine = platform.machine().lower()
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(machine, machine)
    return BundleIdentity(
        source_commit=source_commit,
        platform_system=system,
        platform_machine=machine,
        rustc_verbose=_run_identity_command(("rustc", "-vV"), cwd=project_root),
        cargo_version=_run_identity_command(("cargo", "--version"), cwd=project_root),
    )


def _require_regular_file(path: Path, *, role: str) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as exc:
        raise NightlyRuntimeBundleError(f"missing {role}: {path}: {exc}") from exc
    if not stat.S_ISREG(metadata.st_mode):
        raise NightlyRuntimeBundleError(f"{role} must be a regular file: {path}")
    if metadata.st_size <= 0:
        raise NightlyRuntimeBundleError(f"{role} must not be empty: {path}")
    return metadata


def _stat_stamp(metadata: os.stat_result) -> tuple[int, int, int, int, int]:
    return (
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
        metadata.st_dev,
        metadata.st_ino,
    )


def _require_unchanged(path: Path, stamp: tuple[int, int, int, int, int]) -> None:
    try:
        current = _stat_stamp(path.stat())
    except OSError as exc:
        raise NightlyRuntimeBundleError(
            f"artifact disappeared while bundling or verifying: {path}: {exc}"
        ) from exc
    if current != stamp:
        raise NightlyRuntimeBundleError(
            f"artifact changed while bundling or verifying: {path}"
        )


def select_bundle_inputs(
    target_root: Path,
    *,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
    profile: str = PROFILE,
) -> tuple[BundleInput, ...]:
    if profile != PROFILE:
        raise NightlyRuntimeBundleError(
            f"Nightly runtime bundle profile must be {PROFILE!r}, got {profile!r}"
        )
    profile_root = target_root / profile
    runtime_name = _runtime_lib_archive_name(STDLIB_PROFILE, "x86_64-unknown-linux-gnu")
    runtime = profile_root / runtime_name
    link_manifest = native_link_dependency_manifest_path(runtime)
    backend = profile_root / "molt-backend"
    runtime_metadata = _require_regular_file(runtime, role=RUNTIME_ROLE)
    _require_regular_file(link_manifest, role=LINK_ROLE)
    backend_metadata = _require_regular_file(backend, role=BACKEND_ROLE)
    if os.name == "posix" and runtime_metadata.st_mode & 0o111:
        raise NightlyRuntimeBundleError(
            f"runtime archive unexpectedly has executable bits: {runtime}"
        )
    if os.name == "posix" and backend_metadata.st_mode & 0o111 == 0:
        raise NightlyRuntimeBundleError(
            f"backend executable has no executable bit: {backend}"
        )
    try:
        read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            cargo_profile=profile,
            source_root=source_root,
            source_fingerprint=source_fingerprint,
        )
    except (OSError, ValueError, RuntimeError) as exc:
        raise NightlyRuntimeBundleError(
            f"native link metadata does not attest the selected runtime archive: {exc}"
        ) from exc
    return (
        BundleInput(RUNTIME_ROLE, runtime, f"{profile}/{runtime.name}", 0o644),
        BundleInput(
            LINK_ROLE,
            link_manifest,
            f"{profile}/{link_manifest.name}",
            0o644,
        ),
        BundleInput(BACKEND_ROLE, backend, f"{profile}/{backend.name}", 0o755),
    )


def current_runtime_source_fingerprint(
    project_root: Path,
    runtime_archive: Path,
    *,
    profile: str = PROFILE,
) -> dict[str, object]:
    plan = _prepare_native_runtime_build(
        runtime_archive,
        None,
        True,
        profile,
        project_root,
        None,
        stdlib_profile=STDLIB_PROFILE,
        extra_runtime_features=None,
        stage_timings_ms=None,
        runtime_state=None,
    )
    if plan is None:
        raise NightlyRuntimeBundleError(
            "cannot compute the canonical runtime source/config/toolchain fingerprint"
        )
    return dict(plan.source_fingerprint)


def _file_record(bundle_input: BundleInput) -> dict[str, object]:
    record: dict[str, object] = {
        "role": bundle_input.role,
        "path": bundle_input.archive_path,
        "size_bytes": bundle_input.source.stat().st_size,
        "sha256": _sha256_file(bundle_input.source),
        "mode": f"{bundle_input.mode:04o}",
    }
    if bundle_input.role == RUNTIME_ROLE:
        try:
            record["artifact_identity"] = artifact_content_identity(bundle_input.source)
        except StaticArchiveIdentityError as exc:
            raise NightlyRuntimeBundleError(
                f"selected runtime archive is invalid: {exc}"
            ) from exc
    return record


def build_manifest(
    inputs: Sequence[BundleInput],
    *,
    identity: BundleIdentity,
    runtime_source_fingerprint: Mapping[str, object],
    profile: str = PROFILE,
) -> dict[str, object]:
    if tuple(item.role for item in inputs) != _ROLES:
        raise NightlyRuntimeBundleError(
            f"bundle inputs must contain exactly the ordered roles {_ROLES!r}"
        )
    paths = [item.archive_path for item in inputs]
    if len(set(paths)) != len(paths):
        raise NightlyRuntimeBundleError("bundle input paths must be unique")
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "identity": identity.as_dict(),
        "profile": profile,
        "stdlib_profile": STDLIB_PROFILE,
        "runtime_source_fingerprint": dict(runtime_source_fingerprint),
        "files": [_file_record(item) for item in inputs],
    }


def _manifest_bytes(manifest: Mapping[str, object]) -> bytes:
    return (
        json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    ).encode("utf-8")


def _tar_info(name: str, *, size: int, mode: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.size = size
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.type = tarfile.REGTYPE
    return info


def pack_bundle(
    *,
    project_root: Path,
    target_root: Path,
    output: Path,
    manifest_output: Path,
    identity: BundleIdentity,
    profile: str = PROFILE,
) -> dict[str, object]:
    output = output.absolute()
    manifest_output = manifest_output.absolute()
    if output == manifest_output:
        raise NightlyRuntimeBundleError("archive and manifest output paths must differ")
    runtime_name = _runtime_lib_archive_name(STDLIB_PROFILE, "x86_64-unknown-linux-gnu")
    runtime_archive = target_root / profile / runtime_name
    source_fingerprint = current_runtime_source_fingerprint(
        project_root, runtime_archive, profile=profile
    )
    inputs = select_bundle_inputs(
        target_root,
        source_root=project_root,
        source_fingerprint=source_fingerprint,
        profile=profile,
    )
    input_stamps = {
        item.source: _stat_stamp(_require_regular_file(item.source, role=item.role))
        for item in inputs
    }
    manifest = build_manifest(
        inputs,
        identity=identity,
        runtime_source_fingerprint=source_fingerprint,
        profile=profile,
    )
    for source, stamp in input_stamps.items():
        _require_unchanged(source, stamp)
    encoded_manifest = _manifest_bytes(manifest)
    staged_archive = staged_output_path(output)
    staged_manifest = staged_output_path(manifest_output)
    try:
        with staged_archive.open("wb") as raw_archive:
            with tarfile.open(
                fileobj=raw_archive,
                mode="w",
                format=tarfile.USTAR_FORMAT,
            ) as bundle:
                bundle.addfile(
                    _tar_info(
                        MANIFEST_NAME,
                        size=len(encoded_manifest),
                        mode=0o644,
                    ),
                    io.BytesIO(encoded_manifest),
                )
                for item in inputs:
                    with item.source.open("rb") as source:
                        bundle.addfile(
                            _tar_info(
                                item.archive_path,
                                size=item.source.stat().st_size,
                                mode=item.mode,
                            ),
                            source,
                        )
            raw_archive.flush()
            os.fsync(raw_archive.fileno())
        for source, stamp in input_stamps.items():
            _require_unchanged(source, stamp)
        staged_manifest.write_bytes(encoded_manifest)
        fsync_file(staged_manifest)
        publish_validated_outputs(
            [(staged_archive, output), (staged_manifest, manifest_output)]
        )
    finally:
        for staged in (staged_archive, staged_manifest):
            try:
                staged.unlink()
            except FileNotFoundError:
                pass
    return manifest


def _strict_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _read_manifest_bytes(raw: bytes) -> Mapping[str, object]:
    if not raw or len(raw) > _MAX_MANIFEST_BYTES:
        raise NightlyRuntimeBundleError("bundle manifest size is invalid")
    try:
        payload = json.loads(
            raw.decode("utf-8", errors="strict"),
            object_pairs_hook=_strict_json_object,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise NightlyRuntimeBundleError(f"bundle manifest is invalid: {exc}") from exc
    if not isinstance(payload, dict):
        raise NightlyRuntimeBundleError("bundle manifest must be a JSON object")
    return payload


def _validated_member_name(name: str) -> str:
    if not name or "\\" in name:
        raise NightlyRuntimeBundleError(f"unsafe archive member path: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise NightlyRuntimeBundleError(f"unsafe archive member path: {name!r}")
    if path.as_posix() != name:
        raise NightlyRuntimeBundleError(f"non-canonical archive member path: {name!r}")
    return name


def _validated_identity(value: object) -> BundleIdentity:
    if not isinstance(value, dict) or set(value) != {
        "source_commit",
        "platform",
        "toolchain",
    }:
        raise NightlyRuntimeBundleError("bundle identity shape is invalid")
    platform_value = value.get("platform")
    toolchain = value.get("toolchain")
    if not isinstance(platform_value, dict) or set(platform_value) != {
        "system",
        "machine",
    }:
        raise NightlyRuntimeBundleError("bundle platform identity is invalid")
    if not isinstance(toolchain, dict) or set(toolchain) != {
        "rustc_verbose",
        "cargo_version",
    }:
        raise NightlyRuntimeBundleError("bundle toolchain identity is invalid")
    try:
        return BundleIdentity(
            source_commit=str(value.get("source_commit", "")),
            platform_system=str(platform_value.get("system", "")),
            platform_machine=str(platform_value.get("machine", "")),
            rustc_verbose=str(toolchain.get("rustc_verbose", "")),
            cargo_version=str(toolchain.get("cargo_version", "")),
        )
    except ValueError as exc:
        raise NightlyRuntimeBundleError(f"bundle identity is invalid: {exc}") from exc


def _validated_file_records(value: object) -> tuple[Mapping[str, object], ...]:
    if not isinstance(value, list) or len(value) != len(_ROLES):
        raise NightlyRuntimeBundleError("bundle must declare exactly three files")
    records: list[Mapping[str, object]] = []
    for expected_role, raw in zip(_ROLES, value, strict=True):
        if not isinstance(raw, dict):
            raise NightlyRuntimeBundleError("bundle file record must be an object")
        expected_fields = {"role", "path", "size_bytes", "sha256", "mode"}
        if expected_role == RUNTIME_ROLE:
            expected_fields.add("artifact_identity")
        if set(raw) != expected_fields or raw.get("role") != expected_role:
            raise NightlyRuntimeBundleError(
                f"invalid bundle file record for {expected_role}"
            )
        path = raw.get("path")
        size = raw.get("size_bytes")
        digest = raw.get("sha256")
        mode = raw.get("mode")
        if not isinstance(path, str):
            raise NightlyRuntimeBundleError("bundle file path must be a string")
        _validated_member_name(path)
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            raise NightlyRuntimeBundleError("bundle file size must be positive")
        if not isinstance(digest, str) or _SHA256_RE.fullmatch(digest) is None:
            raise NightlyRuntimeBundleError(
                "bundle file digest must be lowercase SHA-256"
            )
        expected_mode = "0755" if expected_role == BACKEND_ROLE else "0644"
        if mode != expected_mode:
            raise NightlyRuntimeBundleError(
                f"bundle file mode for {expected_role} must be {expected_mode}"
            )
        records.append(raw)
    paths = [str(record["path"]) for record in records]
    if len(set(paths)) != len(paths):
        raise NightlyRuntimeBundleError("bundle file paths are duplicated")
    return tuple(records)


def _validated_source_fingerprint(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != {
        "hash",
        "inputs_digest",
        "meta_digest",
        "rustc",
    }:
        raise NightlyRuntimeBundleError("runtime source fingerprint shape is invalid")
    result: dict[str, object] = {}
    for field in ("hash", "inputs_digest", "meta_digest"):
        raw = value.get(field)
        if field == "inputs_digest" and raw is None:
            result[field] = None
            continue
        if not isinstance(raw, str) or _SHA256_RE.fullmatch(raw) is None:
            raise NightlyRuntimeBundleError(
                f"runtime source fingerprint {field} is invalid"
            )
        result[field] = raw
    rustc = value.get("rustc")
    if not isinstance(rustc, str) or not rustc:
        raise NightlyRuntimeBundleError("runtime source fingerprint rustc is invalid")
    result["rustc"] = rustc
    return result


def validate_manifest(
    manifest: Mapping[str, object],
    *,
    expected_identity: BundleIdentity,
    expected_runtime_source_fingerprint: Mapping[str, object],
) -> tuple[Mapping[str, object], ...]:
    if set(manifest) != {
        "schema_version",
        "kind",
        "identity",
        "profile",
        "stdlib_profile",
        "runtime_source_fingerprint",
        "files",
    }:
        raise NightlyRuntimeBundleError("bundle manifest shape is invalid")
    if manifest.get("schema_version") != SCHEMA_VERSION or manifest.get("kind") != KIND:
        raise NightlyRuntimeBundleError("bundle manifest schema is unsupported")
    if manifest.get("profile") != PROFILE:
        raise NightlyRuntimeBundleError("bundle profile identity is invalid")
    if manifest.get("stdlib_profile") != STDLIB_PROFILE:
        raise NightlyRuntimeBundleError("bundle stdlib profile identity is invalid")
    actual_identity = _validated_identity(manifest.get("identity"))
    if actual_identity != expected_identity:
        raise NightlyRuntimeBundleError(
            "bundle source, platform, or toolchain identity does not match this job"
        )
    actual_source_fingerprint = _validated_source_fingerprint(
        manifest.get("runtime_source_fingerprint")
    )
    if actual_source_fingerprint != _validated_source_fingerprint(
        expected_runtime_source_fingerprint
    ):
        raise NightlyRuntimeBundleError(
            "bundle runtime source/config/toolchain fingerprint does not match this job"
        )
    records = _validated_file_records(manifest.get("files"))
    runtime_name = _runtime_lib_archive_name(STDLIB_PROFILE, "x86_64-unknown-linux-gnu")
    expected_paths = (
        f"{PROFILE}/{runtime_name}",
        f"{PROFILE}/{runtime_name}.native-link-deps.json",
        f"{PROFILE}/molt-backend",
    )
    if tuple(str(record["path"]) for record in records) != expected_paths:
        raise NightlyRuntimeBundleError("bundle contains a non-canonical payload path")
    return records


def _copy_member_exact(
    source: BinaryIO,
    destination: Path,
    *,
    expected_size: int,
    expected_sha256: str,
) -> None:
    digest = hashlib.sha256()
    copied = 0
    with destination.open("xb") as output:
        while True:
            block = source.read(1024 * 1024)
            if not block:
                break
            copied += len(block)
            if copied > expected_size:
                raise NightlyRuntimeBundleError("archive member exceeds declared size")
            digest.update(block)
            output.write(block)
        output.flush()
        os.fsync(output.fileno())
    if copied != expected_size:
        raise NightlyRuntimeBundleError("archive member size does not match manifest")
    if digest.hexdigest() != expected_sha256:
        raise NightlyRuntimeBundleError("archive member hash does not match manifest")


def _validate_staged_link_metadata(
    path: Path,
    *,
    runtime_identity: Mapping[str, object],
    source_fingerprint: Mapping[str, object],
) -> None:
    try:
        payload = json.loads(
            path.read_text(encoding="utf-8", errors="strict"),
            object_pairs_hook=_strict_json_object,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise NightlyRuntimeBundleError(
            f"extracted native link metadata is invalid: {exc}"
        ) from exc
    if not isinstance(payload, dict):
        raise NightlyRuntimeBundleError(
            "extracted native link metadata must be a JSON object"
        )
    source = payload.get("source")
    cargo = payload.get("cargo")
    if (
        payload.get("runtime") != runtime_identity
        or not isinstance(source, dict)
        or set(source) != {"fingerprint"}
        or _validated_source_fingerprint(source.get("fingerprint"))
        != _validated_source_fingerprint(source_fingerprint)
        or not isinstance(cargo, dict)
        or cargo.get("profile") != PROFILE
        or cargo.get("profile_dir") != PROFILE
        or cargo.get("target_triple") is not None
    ):
        raise NightlyRuntimeBundleError(
            "native link metadata does not match the runtime/source/profile identity"
        )


def _ensure_destination_has_no_symlink(
    destination: Path, relative: PurePosixPath
) -> None:
    current = destination
    if current.exists() and current.is_symlink():
        raise NightlyRuntimeBundleError(
            f"extraction destination is a symlink: {current}"
        )
    for part in relative.parts[:-1]:
        current = current / part
        if current.exists() and current.is_symlink():
            raise NightlyRuntimeBundleError(
                f"extraction destination component is a symlink: {current}"
            )


def verify_extract_bundle(
    *,
    archive: Path,
    destination: Path,
    expected_identity: BundleIdentity,
    expected_runtime_source_fingerprint: Mapping[str, object],
) -> Mapping[str, object]:
    archive_stamp = _stat_stamp(_require_regular_file(archive, role="bundle archive"))
    destination = destination.absolute()
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        bundle = tarfile.open(archive, mode="r:")
    except (OSError, tarfile.TarError) as exc:
        raise NightlyRuntimeBundleError(
            f"cannot read uncompressed bundle: {exc}"
        ) from exc
    with bundle:
        members: dict[str, tarfile.TarInfo] = {}
        total_size = 0
        for member in bundle:
            name = _validated_member_name(member.name)
            if name in members:
                raise NightlyRuntimeBundleError(f"duplicate archive member: {name}")
            if not member.isreg():
                raise NightlyRuntimeBundleError(
                    f"archive member is not a regular file: {name}"
                )
            if member.size <= 0:
                raise NightlyRuntimeBundleError(f"archive member is empty: {name}")
            total_size += member.size
            if total_size > _MAX_BUNDLE_PAYLOAD_BYTES:
                raise NightlyRuntimeBundleError("bundle payload exceeds safety limit")
            members[name] = member
        manifest_member = members.get(MANIFEST_NAME)
        if manifest_member is None:
            raise NightlyRuntimeBundleError("bundle manifest is missing")
        manifest_stream = bundle.extractfile(manifest_member)
        if manifest_stream is None:
            raise NightlyRuntimeBundleError("bundle manifest cannot be read")
        manifest = _read_manifest_bytes(manifest_stream.read(_MAX_MANIFEST_BYTES + 1))
        records = validate_manifest(
            manifest,
            expected_identity=expected_identity,
            expected_runtime_source_fingerprint=expected_runtime_source_fingerprint,
        )
        expected_names = {MANIFEST_NAME, *(str(record["path"]) for record in records)}
        if set(members) != expected_names:
            missing = sorted(expected_names - set(members))
            extra = sorted(set(members) - expected_names)
            raise NightlyRuntimeBundleError(
                f"bundle member closure mismatch; missing={missing}, extra={extra}"
            )
        if members[MANIFEST_NAME].mode != 0o644:
            raise NightlyRuntimeBundleError("bundle manifest archive mode is invalid")
        for record in records:
            member = members[str(record["path"])]
            if member.mode != int(str(record["mode"]), 8):
                raise NightlyRuntimeBundleError(
                    f"archive member mode does not match manifest: {record['path']}"
                )
        with tempfile.TemporaryDirectory(
            prefix=".nightly-runtime-extract-", dir=destination.parent
        ) as temporary:
            stage_root = Path(temporary)
            staged_pairs: list[tuple[Path, Path]] = []
            for record in records:
                relative = PurePosixPath(str(record["path"]))
                _ensure_destination_has_no_symlink(destination, relative)
                staged = stage_root.joinpath(*relative.parts)
                staged.parent.mkdir(parents=True, exist_ok=True)
                stream = bundle.extractfile(members[str(record["path"])])
                if stream is None:
                    raise NightlyRuntimeBundleError(
                        f"archive member cannot be read: {record['path']}"
                    )
                _copy_member_exact(
                    stream,
                    staged,
                    expected_size=int(record["size_bytes"]),
                    expected_sha256=str(record["sha256"]),
                )
                staged.chmod(int(str(record["mode"]), 8))
                final = destination.joinpath(*relative.parts)
                staged_pairs.append((staged, final))
            runtime_record = records[0]
            staged_runtime = staged_pairs[0][0]
            try:
                runtime_identity = artifact_content_identity(staged_runtime)
            except StaticArchiveIdentityError as exc:
                raise NightlyRuntimeBundleError(
                    f"extracted runtime archive is invalid: {exc}"
                ) from exc
            if runtime_identity != runtime_record.get("artifact_identity"):
                raise NightlyRuntimeBundleError(
                    "runtime archive semantic identity does not match manifest"
                )
            _validate_staged_link_metadata(
                staged_pairs[1][0],
                runtime_identity=runtime_identity,
                source_fingerprint=expected_runtime_source_fingerprint,
            )
            staged_manifest = stage_root / MANIFEST_NAME
            staged_manifest.write_bytes(_manifest_bytes(manifest))
            staged_manifest.chmod(0o644)
            fsync_file(staged_manifest)
            _ensure_destination_has_no_symlink(
                destination, PurePosixPath(MANIFEST_NAME)
            )
            staged_pairs.append((staged_manifest, destination / MANIFEST_NAME))
            _require_unchanged(archive, archive_stamp)
            for _staged, final in staged_pairs:
                relative = PurePosixPath(final.relative_to(destination).as_posix())
                _ensure_destination_has_no_symlink(destination, relative)
            publish_validated_outputs(staged_pairs)
    return manifest


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Pack or hydrate the exact portable Linux Nightly runtime bundle."
    )
    subparsers = parser.add_subparsers(dest="action", required=True)
    pack = subparsers.add_parser("pack")
    pack.add_argument("--project-root", type=Path, default=ROOT)
    pack.add_argument("--target-root", type=Path, required=True)
    pack.add_argument("--output", type=Path, required=True)
    pack.add_argument("--manifest-out", type=Path, required=True)
    extract = subparsers.add_parser("verify-extract")
    extract.add_argument("--project-root", type=Path, default=ROOT)
    extract.add_argument("--archive", type=Path, required=True)
    extract.add_argument("--destination", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    identity = collect_bundle_identity(args.project_root)
    if args.action == "pack":
        manifest = pack_bundle(
            project_root=args.project_root,
            target_root=args.target_root,
            output=args.output,
            manifest_output=args.manifest_out,
            identity=identity,
        )
    else:
        runtime_name = _runtime_lib_archive_name(
            STDLIB_PROFILE, "x86_64-unknown-linux-gnu"
        )
        runtime_source_fingerprint = current_runtime_source_fingerprint(
            args.project_root,
            args.destination / PROFILE / runtime_name,
        )
        manifest = verify_extract_bundle(
            archive=args.archive,
            destination=args.destination,
            expected_identity=identity,
            expected_runtime_source_fingerprint=runtime_source_fingerprint,
        )
    print(json.dumps(manifest, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
