from __future__ import annotations

import argparse
from collections.abc import Iterator, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import hashlib
import io
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
from typing import IO, TypedDict


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.cli.native_link_manifest import (  # noqa: E402
    NativeLinkDependencyManifestError,
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
    read_native_link_dependency_manifest_payload,
    validate_native_link_dependency_manifest,
)
from molt.cli.native_link_custody import (  # noqa: E402
    NativeLinkCustodyError,
    native_link_custody_archive_path,
    validate_native_link_custody,
    validate_native_link_custody_archive,
)
from molt.cli.runtime_artifact_selection import (  # noqa: E402
    RUNTIME_STATICLIB_ARTIFACTS,
)
from molt.cli.runtime_build_identity import (  # noqa: E402
    RuntimeBuildIdentity,
    require_native_runtime_staticlib_identity,
)
from molt.cli.runtime_paths import _runtime_lib_archive_name  # noqa: E402
from molt.cli.runtime_native_build import (  # noqa: E402
    current_native_runtime_build_identity,
)
from molt.cli.static_archive_identity import (  # noqa: E402
    StaticArchiveIdentityError,
    artifact_content_identity,
    validate_artifact_content_identity,
)
from molt.toolchain_identity import (  # noqa: E402
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.exact_json import dumps_exact, encode_exact, loads_exact  # noqa: E402
from molt.portable_paths import portable_relative_path  # noqa: E402
from molt.ustar import RegularUstarTarInfo  # noqa: E402
from tools.artifact_publish import (  # noqa: E402
    fsync_file,
    publish_validated_outputs,
    staged_output_path,
)
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import clean_checkout_status_arguments  # noqa: E402


_COMMANDS = CommandExecutor.for_file(__file__)
SCHEMA_VERSION = 3
KIND = "molt_nightly_runtime_bundle"
MANIFEST_NAME = "nightly-runtime-manifest.json"
PROFILE = "dev-fast"
STDLIB_PROFILE = "full"
RUNTIME_ROLE = "runtime_archive"
LINK_ROLE = "native_link_manifest"
CUSTODY_ROLE = "native_link_custody_archive"
BACKEND_ROLE = "backend_executable"
_REQUIRED_ROLES = (RUNTIME_ROLE, LINK_ROLE, BACKEND_ROLE)
_ROLES_WITH_CUSTODY = (RUNTIME_ROLE, LINK_ROLE, CUSTODY_ROLE, BACKEND_ROLE)
_MAX_MANIFEST_BYTES = 1024 * 1024
_MAX_BUNDLE_PAYLOAD_BYTES = 2 * 1024 * 1024 * 1024
# Each permitted USTAR member adds a header and at most one padding block;
# reserve the final record for end markers and record-alignment padding.
_MAX_BUNDLE_ARCHIVE_BYTES = (
    _MAX_BUNDLE_PAYLOAD_BYTES
    + (len(_ROLES_WITH_CUSTODY) + 1) * 2 * tarfile.BLOCKSIZE
    + tarfile.RECORDSIZE
)
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_COMMIT_RE = re.compile(r"[0-9a-f]{40,64}")
_CUSTODY_ARCHIVE_RE = re.compile(r"molt-native-link-custody-[0-9a-f]{64}\.tar")
_NATIVE_TARGET_CELLS = {
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
    "x86_64-apple-darwin": ("macos", "x86_64"),
    "aarch64-apple-darwin": ("macos", "aarch64"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64"),
    "aarch64-pc-windows-msvc": ("windows", "aarch64"),
}


class NightlyRuntimeBundleError(RuntimeError):
    """A Nightly runtime bundle is incomplete, corrupt, or identity-mismatched."""


@dataclass(frozen=True)
class BundleIdentity:
    source_commit: str
    target_triple: str

    def __post_init__(self) -> None:
        if _COMMIT_RE.fullmatch(self.source_commit) is None:
            raise ValueError("source_commit must be a lowercase Git object id")
        if self.target_triple not in _NATIVE_TARGET_CELLS:
            raise ValueError(
                f"unsupported Nightly runtime bundle target: {self.target_triple}"
            )

    @property
    def platform_system(self) -> str:
        return _NATIVE_TARGET_CELLS[self.target_triple][0]

    @property
    def platform_machine(self) -> str:
        return _NATIVE_TARGET_CELLS[self.target_triple][1]

    @classmethod
    def from_runtime(
        cls, source_commit: str, runtime_build_identity: RuntimeBuildIdentity
    ) -> BundleIdentity:
        runtime_build_identity = _validated_runtime_build_identity(
            runtime_build_identity, cargo_profile=PROFILE, target_triple=None
        )
        return cls(source_commit, runtime_build_identity.effective_target)

    def as_dict(self) -> dict[str, object]:
        return {
            "source_commit": self.source_commit,
            "platform": {
                "system": self.platform_system,
                "machine": self.platform_machine,
                "target_triple": self.target_triple,
            },
        }


@dataclass(frozen=True)
class BundleInput:
    role: str
    source: Path
    archive_path: str
    mode: int


class _BundleFileFields(TypedDict):
    role: str
    path: str
    size_bytes: int
    sha256: str
    mode: str


class BundleFileRecord(_BundleFileFields, total=False):
    artifact_identity: object


def _run_identity_command(
    argv: Sequence[str], *, cwd: Path, allow_empty: bool = False
) -> str:
    try:
        result = _COMMANDS.run(
            list(argv),
            cwd=cwd,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="strict",
        )
    except (OSError, subprocess.SubprocessError, UnicodeError) as exc:
        raise NightlyRuntimeBundleError(
            f"cannot establish bundle identity with {' '.join(argv)}: {exc}"
        ) from exc
    value = result.stdout.strip()
    if not value and not allow_empty:
        raise NightlyRuntimeBundleError(
            f"bundle identity command produced no output: {' '.join(argv)}"
        )
    return value


def collect_bundle_identity(
    project_root: Path, *, runtime_build_identity: RuntimeBuildIdentity
) -> BundleIdentity:
    project_root = project_root.resolve(strict=True)
    status = _run_identity_command(
        ("git", *clean_checkout_status_arguments()),
        cwd=project_root,
        allow_empty=True,
    )
    if status:
        raise NightlyRuntimeBundleError(
            "refusing to publish a Nightly runtime bundle from a dirty checkout"
        )
    source_commit = _run_identity_command(
        ("git", "rev-parse", "HEAD"), cwd=project_root
    )
    return BundleIdentity.from_runtime(source_commit, runtime_build_identity)


def capture_bundle_runtime_identity(
    project_root: Path, target_root: Path
) -> RuntimeBuildIdentity:
    """Capture the native producer once before projecting bundle metadata."""
    runtime = target_root / PROFILE / _runtime_lib_archive_name(STDLIB_PROFILE, None)
    try:
        return current_native_runtime_build_identity(
            project_root,
            runtime,
            target_triple=None,
            cargo_profile=PROFILE,
            stdlib_profile=STDLIB_PROFILE,
        )
    except (OSError, ValueError) as exc:
        raise NightlyRuntimeBundleError(
            "cannot compute the canonical runtime build identity"
        ) from exc


def _runtime_archive_name(identity: BundleIdentity) -> str:
    return _runtime_lib_archive_name(STDLIB_PROFILE, identity.target_triple)


def _backend_executable_name(identity: BundleIdentity) -> str:
    return (
        "molt-backend.exe" if identity.platform_system == "windows" else "molt-backend"
    )


def _require_regular_file(path: Path, *, role: str) -> os.stat_result:
    try:
        with open_stable_regular_file(path, label=role) as opened:
            metadata = opened.stat
    except (OSError, ValueError) as exc:
        raise NightlyRuntimeBundleError(f"missing {role}: {path}: {exc}") from exc
    if metadata.st_size <= 0:
        raise NightlyRuntimeBundleError(f"{role} must not be empty: {path}")
    return metadata


def _capture_file(
    path: Path,
    *,
    role: str,
    max_bytes: int = _MAX_BUNDLE_PAYLOAD_BYTES,
) -> StableRegularFileIdentity:
    try:
        with open_stable_regular_file(path, label=role) as opened:
            if opened.stat.st_size <= 0:
                raise NightlyRuntimeBundleError(f"{role} must not be empty: {path}")
            if opened.stat.st_size > max_bytes:
                raise NightlyRuntimeBundleError(
                    f"{role} exceeds safety limit of {max_bytes} bytes: {path}"
                )
            return stable_regular_file_identity(path, label=role)
    except (OSError, ValueError) as exc:
        raise NightlyRuntimeBundleError(
            f"cannot capture {role}: {path}: {exc}"
        ) from exc


def _require_unchanged(identity: StableRegularFileIdentity) -> None:
    try:
        verify_stable_regular_file_identity(identity, label="bundle input")
    except (OSError, ValueError) as exc:
        raise NightlyRuntimeBundleError(
            f"artifact changed while bundling or verifying: {identity.path}: {exc}"
        ) from exc


def select_bundle_inputs(
    target_root: Path,
    *,
    identity: BundleIdentity,
    runtime_build_identity: RuntimeBuildIdentity,
    profile: str = PROFILE,
) -> tuple[BundleInput, ...]:
    if profile != PROFILE:
        raise NightlyRuntimeBundleError(
            f"Nightly runtime bundle profile must be {PROFILE!r}, got {profile!r}"
        )
    _require_bundle_runtime_identity(identity, runtime_build_identity)
    profile_root = target_root / profile
    runtime_name = _runtime_archive_name(identity)
    runtime = profile_root / runtime_name
    link_manifest = native_link_dependency_manifest_path(runtime)
    backend = profile_root / _backend_executable_name(identity)
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
        manifest = read_native_link_dependency_manifest(
            runtime,
            target_triple=None,
            cargo_profile=profile,
            runtime_build_identity=runtime_build_identity,
        )
    except (OSError, ValueError, RuntimeError) as exc:
        raise NightlyRuntimeBundleError(
            f"native link metadata does not attest the selected runtime archive: {exc}"
        ) from exc
    inputs = [
        BundleInput(RUNTIME_ROLE, runtime, f"{profile}/{runtime.name}", 0o644),
        BundleInput(
            LINK_ROLE,
            link_manifest,
            f"{profile}/{link_manifest.name}",
            0o644,
        ),
    ]
    try:
        custody, entries = validate_native_link_custody(
            manifest.get("custody"),
            context=str(link_manifest),
        )
        if entries:
            custody_archive = native_link_custody_archive_path(runtime, custody)
            if custody_archive is None:
                raise NativeLinkCustodyError(
                    "native-link custody entries have no archive"
                )
            _require_regular_file(custody_archive, role=CUSTODY_ROLE)
            inputs.append(
                BundleInput(
                    CUSTODY_ROLE,
                    custody_archive,
                    f"{profile}/{custody_archive.name}",
                    0o644,
                )
            )
    except NativeLinkCustodyError as exc:
        raise NightlyRuntimeBundleError(
            f"native link custody does not attest the selected runtime archive: {exc}"
        ) from exc
    inputs.append(
        BundleInput(BACKEND_ROLE, backend, f"{profile}/{backend.name}", 0o755)
    )
    return tuple(inputs)


def _file_record(
    bundle_input: BundleInput, identity: StableRegularFileIdentity
) -> BundleFileRecord:
    _require_unchanged(identity)
    record: BundleFileRecord = {
        "role": bundle_input.role,
        "path": bundle_input.archive_path,
        "size_bytes": identity.size,
        "sha256": identity.sha256,
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
    runtime_build_identity: RuntimeBuildIdentity,
    input_identities: Mapping[Path, StableRegularFileIdentity],
    profile: str = PROFILE,
) -> dict[str, object]:
    _require_bundle_runtime_identity(identity, runtime_build_identity)
    roles = tuple(item.role for item in inputs)
    if roles not in (_REQUIRED_ROLES, _ROLES_WITH_CUSTODY):
        raise NightlyRuntimeBundleError(
            "bundle inputs must contain the exact ordered runtime, native-link, "
            "optional custody, and backend roles"
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
        "runtime_build_identity": runtime_build_identity.to_dict(),
        "files": [_file_record(item, input_identities[item.source]) for item in inputs],
    }


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
    target_root: Path,
    output: Path,
    manifest_output: Path,
    identity: BundleIdentity,
    runtime_build_identity: RuntimeBuildIdentity,
    profile: str = PROFILE,
) -> dict[str, object]:
    output = output.absolute()
    manifest_output = manifest_output.absolute()
    if output == manifest_output:
        raise NightlyRuntimeBundleError("archive and manifest output paths must differ")
    inputs = select_bundle_inputs(
        target_root,
        identity=identity,
        runtime_build_identity=runtime_build_identity,
        profile=profile,
    )
    input_identities = {
        item.source: _capture_file(item.source, role=item.role) for item in inputs
    }
    manifest = build_manifest(
        inputs,
        identity=identity,
        runtime_build_identity=runtime_build_identity,
        input_identities=input_identities,
        profile=profile,
    )
    for input_identity in input_identities.values():
        _require_unchanged(input_identity)
    encoded_manifest = encode_exact(manifest)
    if len(encoded_manifest) > _MAX_MANIFEST_BYTES:
        raise NightlyRuntimeBundleError("bundle manifest exceeds safety limit")
    if (
        len(encoded_manifest)
        + sum(identity.size for identity in input_identities.values())
        > _MAX_BUNDLE_PAYLOAD_BYTES
    ):
        raise NightlyRuntimeBundleError("bundle payload exceeds safety limit")
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
                    with open_stable_regular_file(
                        item.source, label=item.role
                    ) as opened:
                        _require_unchanged(input_identities[item.source])
                        bundle.addfile(
                            _tar_info(
                                item.archive_path,
                                size=input_identities[item.source].size,
                                mode=item.mode,
                            ),
                            opened.stream,
                        )
                        _require_unchanged(input_identities[item.source])
            raw_archive.flush()
            os.fsync(raw_archive.fileno())
        archive_identity = _capture_file(
            staged_archive,
            role="staged bundle archive",
            max_bytes=_MAX_BUNDLE_ARCHIVE_BYTES,
        )
        for input_identity in input_identities.values():
            _require_unchanged(input_identity)
        staged_manifest.write_bytes(encoded_manifest)
        fsync_file(staged_manifest)
        manifest_identity = _capture_file(
            staged_manifest,
            role="staged bundle manifest",
            max_bytes=_MAX_MANIFEST_BYTES,
        )
        if manifest_identity.sha256 != hashlib.sha256(encoded_manifest).hexdigest():
            raise NightlyRuntimeBundleError(
                "staged bundle manifest changed after writing"
            )
        _require_unchanged(archive_identity)
        _require_unchanged(manifest_identity)
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


def _read_manifest_bytes(raw: bytes) -> Mapping[str, object]:
    if not raw or len(raw) > _MAX_MANIFEST_BYTES:
        raise NightlyRuntimeBundleError("bundle manifest size is invalid")
    try:
        payload = loads_exact(raw.decode("utf-8", errors="strict"))
    except ValueError as exc:
        raise NightlyRuntimeBundleError(f"bundle manifest is invalid: {exc}") from exc
    if not isinstance(payload, dict):
        raise NightlyRuntimeBundleError("bundle manifest must be a JSON object")
    return payload


def _validated_member_name(name: str) -> str:
    try:
        return portable_relative_path(name).as_posix()
    except ValueError as exc:
        raise NightlyRuntimeBundleError(
            f"unsafe archive member path: {name!r}"
        ) from exc


def _identity_text(value: object) -> str:
    if not isinstance(value, str) or not value.strip():
        raise NightlyRuntimeBundleError(
            "bundle identity values must be non-empty strings"
        )
    return value


def _validated_identity(value: object) -> BundleIdentity:
    if not isinstance(value, dict) or set(value) != {"source_commit", "platform"}:
        raise NightlyRuntimeBundleError("bundle identity shape is invalid")
    platform_value = value.get("platform")
    if not isinstance(platform_value, dict) or set(platform_value) != {
        "system",
        "machine",
        "target_triple",
    }:
        raise NightlyRuntimeBundleError("bundle platform identity is invalid")
    try:
        identity = BundleIdentity(
            source_commit=_identity_text(value.get("source_commit")),
            target_triple=_identity_text(platform_value.get("target_triple")),
        )
    except ValueError as exc:
        raise NightlyRuntimeBundleError(f"bundle identity is invalid: {exc}") from exc
    if (
        _identity_text(platform_value.get("system")) != identity.platform_system
        or _identity_text(platform_value.get("machine")) != identity.platform_machine
    ):
        raise NightlyRuntimeBundleError(
            "bundle platform projection disagrees with target identity"
        )
    return identity


def _validated_file_records(value: object) -> tuple[BundleFileRecord, ...]:
    if not isinstance(value, list):
        raise NightlyRuntimeBundleError("bundle files must be an array")
    roles = tuple(raw.get("role") if isinstance(raw, dict) else None for raw in value)
    if roles not in (_REQUIRED_ROLES, _ROLES_WITH_CUSTODY):
        raise NightlyRuntimeBundleError(
            "bundle must declare the exact ordered runtime, native-link, optional "
            "custody, and backend file closure"
        )
    records: list[BundleFileRecord] = []
    for expected_role, raw in zip(roles, value, strict=True):
        assert isinstance(expected_role, str)
        if not isinstance(raw, dict):
            raise NightlyRuntimeBundleError("bundle file record must be an object")
        expected_fields = set(BundleFileRecord.__required_keys__)
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
        if not isinstance(mode, str) or mode != expected_mode:
            raise NightlyRuntimeBundleError(
                f"bundle file mode for {expected_role} must be {expected_mode}"
            )
        record: BundleFileRecord = {
            "role": expected_role,
            "path": path,
            "size_bytes": size,
            "sha256": digest,
            "mode": mode,
        }
        if expected_role == RUNTIME_ROLE:
            try:
                record["artifact_identity"] = validate_artifact_content_identity(
                    raw.get("artifact_identity")
                )
            except StaticArchiveIdentityError as exc:
                raise NightlyRuntimeBundleError(
                    f"bundle runtime content identity is invalid: {exc}"
                ) from exc
        records.append(record)
    paths = [record["path"] for record in records]
    if len(set(paths)) != len(paths):
        raise NightlyRuntimeBundleError("bundle file paths are duplicated")
    return tuple(records)


def _validated_runtime_build_identity(
    value: object,
    *,
    cargo_profile: str,
    target_triple: str | None,
) -> RuntimeBuildIdentity:
    try:
        return require_native_runtime_staticlib_identity(
            value,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        )
    except (TypeError, ValueError) as exc:
        raise NightlyRuntimeBundleError("runtime build identity is invalid") from exc


def _require_bundle_runtime_identity(
    identity: BundleIdentity, runtime_build_identity: RuntimeBuildIdentity
) -> None:
    try:
        projected = BundleIdentity.from_runtime(
            identity.source_commit, runtime_build_identity
        )
    except ValueError as exc:
        raise NightlyRuntimeBundleError(
            f"bundle runtime target is unsupported: {exc}"
        ) from exc
    if identity != projected:
        raise NightlyRuntimeBundleError(
            "bundle target does not match the captured runtime effective target"
        )


def validate_manifest(
    manifest: Mapping[str, object],
    *,
    expected_identity: BundleIdentity,
    expected_runtime_build_identity: RuntimeBuildIdentity,
) -> tuple[BundleFileRecord, ...]:
    if set(manifest) != {
        "schema_version",
        "kind",
        "identity",
        "profile",
        "stdlib_profile",
        "runtime_build_identity",
        "files",
    }:
        raise NightlyRuntimeBundleError("bundle manifest shape is invalid")
    schema_version = manifest.get("schema_version")
    if (
        type(schema_version) is not int
        or schema_version != SCHEMA_VERSION
        or manifest.get("kind") != KIND
    ):
        raise NightlyRuntimeBundleError("bundle manifest schema is unsupported")
    if manifest.get("profile") != PROFILE:
        raise NightlyRuntimeBundleError("bundle profile identity is invalid")
    if manifest.get("stdlib_profile") != STDLIB_PROFILE:
        raise NightlyRuntimeBundleError("bundle stdlib profile identity is invalid")
    actual_identity = _validated_identity(manifest.get("identity"))
    if actual_identity != expected_identity:
        raise NightlyRuntimeBundleError(
            "bundle source or target identity does not match this job"
        )
    actual_runtime_build_identity = _validated_runtime_build_identity(
        manifest.get("runtime_build_identity"),
        cargo_profile=PROFILE,
        target_triple=None,
    )
    expected_runtime_build_identity = _validated_runtime_build_identity(
        expected_runtime_build_identity,
        cargo_profile=PROFILE,
        target_triple=None,
    )
    if actual_runtime_build_identity != expected_runtime_build_identity:
        raise NightlyRuntimeBundleError(
            "bundle runtime build identity does not match this job"
        )
    _require_bundle_runtime_identity(actual_identity, actual_runtime_build_identity)
    records = _validated_file_records(manifest.get("files"))
    runtime_name = _runtime_archive_name(expected_identity)
    expected_paths: tuple[str, ...] = (
        f"{PROFILE}/{runtime_name}",
        f"{PROFILE}/{runtime_name}.native-link-deps.json",
        f"{PROFILE}/{_backend_executable_name(expected_identity)}",
    )
    if tuple(record["role"] for record in records) == _ROLES_WITH_CUSTODY:
        custody_path = records[2]["path"]
        custody_relative = PurePosixPath(custody_path)
        if (
            custody_relative.parent != PurePosixPath(PROFILE)
            or _CUSTODY_ARCHIVE_RE.fullmatch(custody_relative.name) is None
        ):
            raise NightlyRuntimeBundleError(
                "bundle native-link custody path is not canonical"
            )
        expected_paths = (*expected_paths[:2], custody_path, expected_paths[2])
    if tuple(record["path"] for record in records) != expected_paths:
        raise NightlyRuntimeBundleError("bundle contains a non-canonical payload path")
    return records


def _copy_member_exact(
    source: IO[bytes],
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
    runtime_build_identity: RuntimeBuildIdentity,
    custody_archive: Path | None,
    custody_archive_name: str | None,
) -> None:
    try:
        payload = read_native_link_dependency_manifest_payload(path)
        manifest, _items = validate_native_link_dependency_manifest(
            payload,
            runtime_identity=runtime_identity,
            context=str(path),
            target_triple=None,
            cargo_profile=PROFILE,
            runtime_build_identity=runtime_build_identity,
        )
        custody, entries = validate_native_link_custody(
            manifest.get("custody"),
            context=str(path),
        )
        if bool(entries) is not (custody_archive is not None):
            raise NativeLinkCustodyError(
                "bundle custody archive presence does not match the native-link manifest"
            )
        custody_record = custody.get("archive")
        expected_name = (
            custody_record.get("name") if isinstance(custody_record, Mapping) else None
        )
        if custody_archive_name != expected_name:
            raise NativeLinkCustodyError(
                "bundle custody archive name does not match the native-link manifest"
            )
        validate_native_link_custody_archive(
            custody_archive,
            custody,
            context=str(path),
        )
    except (NativeLinkCustodyError, NativeLinkDependencyManifestError) as exc:
        raise NightlyRuntimeBundleError(
            f"extracted native link metadata is invalid: {exc}"
        ) from exc


def _ensure_destination_has_no_link(destination: Path, relative: PurePosixPath) -> None:
    current = destination
    if current.is_symlink() or current.is_junction():
        raise NightlyRuntimeBundleError(
            f"extraction destination is link-like: {current}"
        )
    for part in relative.parts[:-1]:
        current = current / part
        if current.is_symlink() or current.is_junction():
            raise NightlyRuntimeBundleError(
                f"extraction destination component is link-like: {current}"
            )


@contextlib.contextmanager
def _open_stable_bundle(
    archive: Path,
) -> Iterator[tuple[tarfile.TarFile, StableRegularFileIdentity]]:
    try:
        identity = _capture_file(
            archive, role="bundle archive", max_bytes=_MAX_BUNDLE_ARCHIVE_BYTES
        )
        with open_stable_regular_file(archive, label="bundle archive") as opened:
            _require_unchanged(identity)
            with tarfile.open(
                fileobj=opened.stream, mode="r:", tarinfo=RegularUstarTarInfo
            ) as bundle:
                yield bundle, identity
            _require_unchanged(identity)
    except (OSError, ValueError, tarfile.TarError) as exc:
        raise NightlyRuntimeBundleError(
            f"cannot process uncompressed bundle {archive}: {exc}"
        ) from exc


def verify_extract_bundle(
    *,
    archive: Path,
    destination: Path,
    expected_identity: BundleIdentity,
    expected_runtime_build_identity: RuntimeBuildIdentity,
) -> Mapping[str, object]:
    destination = destination.absolute()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with _open_stable_bundle(archive) as (bundle, archive_identity):
        members: dict[str, tarfile.TarInfo] = {}
        total_size = 0
        runtime_name = _runtime_archive_name(expected_identity)
        permitted_names = {
            MANIFEST_NAME,
            f"{PROFILE}/{runtime_name}",
            f"{PROFILE}/{runtime_name}.native-link-deps.json",
            f"{PROFILE}/{_backend_executable_name(expected_identity)}",
        }
        for member in bundle:
            name = _validated_member_name(member.name)
            if name in members:
                raise NightlyRuntimeBundleError(f"duplicate archive member: {name}")
            relative = PurePosixPath(name)
            custody_member = (
                relative.parent == PurePosixPath(PROFILE)
                and _CUSTODY_ARCHIVE_RE.fullmatch(relative.name) is not None
            )
            if len(members) >= len(_ROLES_WITH_CUSTODY) + 1 or (
                name not in permitted_names and not custody_member
            ):
                raise NightlyRuntimeBundleError(
                    f"bundle member closure mismatch: unexpected archive member {name}"
                )
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
            expected_runtime_build_identity=expected_runtime_build_identity,
        )
        expected_names = {MANIFEST_NAME, *(record["path"] for record in records)}
        if set(members) != expected_names:
            missing = sorted(expected_names - set(members))
            extra = sorted(set(members) - expected_names)
            raise NightlyRuntimeBundleError(
                f"bundle member closure mismatch; missing={missing}, extra={extra}"
            )
        if members[MANIFEST_NAME].mode != 0o644:
            raise NightlyRuntimeBundleError("bundle manifest archive mode is invalid")
        for record in records:
            member = members[record["path"]]
            if member.mode != int(record["mode"], 8):
                raise NightlyRuntimeBundleError(
                    f"archive member mode does not match manifest: {record['path']}"
                )
        staged_pairs: list[tuple[Path, Path]] = []
        staged_identities: list[StableRegularFileIdentity] = []
        created_parents: set[Path] = set()
        try:
            for record in records:
                relative = PurePosixPath(record["path"])
                _ensure_destination_has_no_link(destination, relative)
                final = destination.joinpath(*relative.parts)
                cursor = final.parent
                while not cursor.exists():
                    created_parents.add(cursor)
                    if cursor.parent == cursor:
                        break
                    cursor = cursor.parent
                staged = staged_output_path(
                    final,
                    purpose="nightly-hydrate",
                    suffix=final.suffix or ".tmp",
                )
                staged_pairs.append((staged, final))
                stream = bundle.extractfile(members[record["path"]])
                if stream is None:
                    raise NightlyRuntimeBundleError(
                        f"archive member cannot be read: {record['path']}"
                    )
                _copy_member_exact(
                    stream,
                    staged,
                    expected_size=record["size_bytes"],
                    expected_sha256=record["sha256"],
                )
                staged.chmod(int(record["mode"], 8))
                fsync_file(staged)
                staged_identity = _capture_file(staged, role=record["role"])
                if (
                    staged_identity.size != record["size_bytes"]
                    or staged_identity.sha256 != record["sha256"]
                ):
                    raise NightlyRuntimeBundleError(
                        "staged bundle member changed after extraction"
                    )
                staged_identities.append(staged_identity)
            staged_by_role = {
                record["role"]: staged
                for record, (staged, _final) in zip(records, staged_pairs, strict=True)
            }
            runtime_record = records[0]
            staged_runtime = staged_by_role[RUNTIME_ROLE]
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
                staged_by_role[LINK_ROLE],
                runtime_identity=runtime_identity,
                runtime_build_identity=expected_runtime_build_identity,
                custody_archive=staged_by_role.get(CUSTODY_ROLE),
                custody_archive_name=next(
                    (
                        PurePosixPath(record["path"]).name
                        for record in records
                        if record["role"] == CUSTODY_ROLE
                    ),
                    None,
                ),
            )
            staged_manifest = staged_output_path(
                destination / MANIFEST_NAME,
                purpose="nightly-hydrate",
            )
            staged_pairs.append((staged_manifest, destination / MANIFEST_NAME))
            encoded_manifest = encode_exact(manifest)
            staged_manifest.write_bytes(encoded_manifest)
            staged_manifest.chmod(0o644)
            fsync_file(staged_manifest)
            manifest_identity = _capture_file(
                staged_manifest, role="bundle manifest", max_bytes=_MAX_MANIFEST_BYTES
            )
            if manifest_identity.sha256 != hashlib.sha256(encoded_manifest).hexdigest():
                raise NightlyRuntimeBundleError(
                    "staged bundle manifest changed after writing"
                )
            staged_identities.append(manifest_identity)
            _ensure_destination_has_no_link(destination, PurePosixPath(MANIFEST_NAME))
            _require_unchanged(archive_identity)
            for _staged, final in staged_pairs:
                relative = PurePosixPath(final.relative_to(destination).as_posix())
                _ensure_destination_has_no_link(destination, relative)
            payload_pairs = {
                record["role"]: pair
                for record, pair in zip(records, staged_pairs[:-1], strict=True)
            }
            publication_order = (CUSTODY_ROLE, RUNTIME_ROLE, BACKEND_ROLE, LINK_ROLE)
            for identity in staged_identities:
                _require_unchanged(identity)
            publish_validated_outputs(
                [
                    payload_pairs[role]
                    for role in publication_order
                    if role in payload_pairs
                ]
                + [staged_pairs[-1]]
            )
        finally:
            for staged, _final in staged_pairs:
                with contextlib.suppress(OSError):
                    staged.unlink()
            for directory in sorted(
                created_parents,
                key=lambda path: len(path.parts),
                reverse=True,
            ):
                with contextlib.suppress(OSError):
                    directory.rmdir()
    return manifest


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Pack or hydrate an exact portable native Nightly runtime bundle for "
            "Linux, macOS, or Windows."
        )
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
    target_root = args.target_root if args.action == "pack" else args.destination
    runtime_build_identity = capture_bundle_runtime_identity(
        args.project_root, target_root
    )
    identity = collect_bundle_identity(
        args.project_root, runtime_build_identity=runtime_build_identity
    )
    if args.action == "pack":
        manifest = pack_bundle(
            target_root=args.target_root,
            output=args.output,
            manifest_output=args.manifest_out,
            identity=identity,
            runtime_build_identity=runtime_build_identity,
        )
    else:
        manifest = verify_extract_bundle(
            archive=args.archive,
            destination=args.destination,
            expected_identity=identity,
            expected_runtime_build_identity=runtime_build_identity,
        )
    sys.stdout.write(dumps_exact(manifest, indent=None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
