"""Pack and admit the Nightly native toolchain bundle.

Nightly builds the host-native dev runtime and backend once, then every shard
job reuses them. The CLI owns which files make up that toolchain and how a
checkout admits them (``molt.cli.native_toolchain_transfer``); this tool only
transports them: an uncompressed USTAR archive with an exact manifest that
binds the source commit, target, runtime build identity and every member's
size, digest and mode. ``pack`` runs in the environment of the build it
exports. ``verify-extract`` checks the archive, stages each member privately,
and hands them to the CLI import, which refuses a toolchain built from other
inputs than this checkout.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterator, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import hashlib
import io
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
from typing import IO, TypedDict


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from molt.artifact_publication import (  # noqa: E402
    fsync_file,
    publish_validated_outputs,
    staged_output_path,
)
from molt.cli.native_toolchain_transfer import (  # noqa: E402
    OPTIONAL_ROLES,
    REQUIRED_ROLES,
    NativeToolchainSelection,
    NativeToolchainTransferError,
    TransferMember,
    export_native_toolchain,
    import_native_toolchain,
)
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity  # noqa: E402
from molt.exact_json import dumps_exact, encode_exact, loads_exact  # noqa: E402
from molt.portable_paths import (  # noqa: E402
    portable_path_component,
    portable_relative_path,
)
from molt.toolchain_identity import (  # noqa: E402
    StableRegularFileIdentity,
    open_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.ustar import RegularUstarTarInfo  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402
from tools.git_identity import clean_checkout_status_arguments  # noqa: E402


_COMMANDS = CommandExecutor.for_file(__file__)
SCHEMA_VERSION = 4
KIND = "molt_nightly_runtime_bundle"
MANIFEST_NAME = "nightly-runtime-manifest.json"
_ROLES = (*REQUIRED_ROLES, *OPTIONAL_ROLES)
_MAX_MANIFEST_BYTES = 1024 * 1024
_MAX_BUNDLE_PAYLOAD_BYTES = 2 * 1024 * 1024 * 1024
# Each permitted USTAR member adds a header and at most one padding block;
# reserve the final record for end markers and record-alignment padding.
_MAX_BUNDLE_ARCHIVE_BYTES = (
    _MAX_BUNDLE_PAYLOAD_BYTES
    + (len(_ROLES) + 1) * 2 * tarfile.BLOCKSIZE
    + tarfile.RECORDSIZE
)
_SHA256_RE = re.compile(r"[0-9a-f]{64}")
_COMMIT_RE = re.compile(r"[0-9a-f]{40,64}")
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
            runtime_build_identity
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


class BundleFileRecord(TypedDict):
    role: str
    name: str
    size_bytes: int
    sha256: str
    mode: str


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


def _validated_runtime_build_identity(value: object) -> RuntimeBuildIdentity:
    """Shape only: the CLI import compares it with its own current capture."""
    try:
        return RuntimeBuildIdentity.from_dict(value)
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


def _member_mode(member: TransferMember) -> int:
    return 0o755 if member.executable else 0o644


def _member_path(record: Mapping[str, object]) -> str:
    return f"{record['role']}/{record['name']}"


def build_manifest(
    members: Sequence[TransferMember],
    *,
    identity: BundleIdentity,
    runtime_build_identity: RuntimeBuildIdentity,
    member_identities: Mapping[Path, StableRegularFileIdentity],
) -> dict[str, object]:
    _require_bundle_runtime_identity(identity, runtime_build_identity)
    files: list[BundleFileRecord] = []
    for member in members:
        captured = member_identities[member.path]
        _require_unchanged(captured)
        files.append(
            {
                "role": member.role,
                "name": member.path.name,
                "size_bytes": captured.size,
                "sha256": captured.sha256,
                "mode": f"{_member_mode(member):04o}",
            }
        )
    manifest: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "kind": KIND,
        "identity": identity.as_dict(),
        "runtime_build_identity": runtime_build_identity.to_dict(),
        "files": files,
    }
    _validated_file_records(files)
    return manifest


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
    members: Sequence[TransferMember],
    output: Path,
    manifest_output: Path,
    identity: BundleIdentity,
    runtime_build_identity: RuntimeBuildIdentity,
) -> dict[str, object]:
    output = output.absolute()
    manifest_output = manifest_output.absolute()
    if output == manifest_output:
        raise NightlyRuntimeBundleError("archive and manifest output paths must differ")
    member_identities = {
        member.path: _capture_file(member.path, role=member.role) for member in members
    }
    if os.name == "posix":
        for member in members:
            executable = bool(member.path.stat().st_mode & 0o111)
            if executable is not member.executable:
                raise NightlyRuntimeBundleError(
                    f"{member.role} executable bits disagree with its role: "
                    f"{member.path}"
                )
    manifest = build_manifest(
        members,
        identity=identity,
        runtime_build_identity=runtime_build_identity,
        member_identities=member_identities,
    )
    encoded_manifest = encode_exact(manifest)
    if len(encoded_manifest) > _MAX_MANIFEST_BYTES:
        raise NightlyRuntimeBundleError("bundle manifest exceeds safety limit")
    if (
        len(encoded_manifest)
        + sum(captured.size for captured in member_identities.values())
        > _MAX_BUNDLE_PAYLOAD_BYTES
    ):
        raise NightlyRuntimeBundleError("bundle payload exceeds safety limit")
    records = manifest["files"]
    assert isinstance(records, list)
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
                    _tar_info(MANIFEST_NAME, size=len(encoded_manifest), mode=0o644),
                    io.BytesIO(encoded_manifest),
                )
                for member, record in zip(members, records, strict=True):
                    captured = member_identities[member.path]
                    with open_stable_regular_file(
                        member.path, label=member.role
                    ) as opened:
                        _require_unchanged(captured)
                        bundle.addfile(
                            _tar_info(
                                _member_path(record),
                                size=captured.size,
                                mode=_member_mode(member),
                            ),
                            opened.stream,
                        )
                        _require_unchanged(captured)
            raw_archive.flush()
            os.fsync(raw_archive.fileno())
        archive_identity = _capture_file(
            staged_archive,
            role="staged bundle archive",
            max_bytes=_MAX_BUNDLE_ARCHIVE_BYTES,
        )
        for captured in member_identities.values():
            _require_unchanged(captured)
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
            with contextlib.suppress(FileNotFoundError):
                staged.unlink()
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
    records: list[BundleFileRecord] = []
    for raw in value:
        if not isinstance(raw, dict) or set(raw) != set(
            BundleFileRecord.__required_keys__
        ):
            raise NightlyRuntimeBundleError("bundle file record shape is invalid")
        role = raw.get("role")
        name = raw.get("name")
        size = raw.get("size_bytes")
        digest = raw.get("sha256")
        mode = raw.get("mode")
        if role not in _ROLES:
            raise NightlyRuntimeBundleError(f"unknown bundle file role: {role!r}")
        try:
            portable_path_component(name)
        except ValueError as exc:
            raise NightlyRuntimeBundleError(
                f"bundle file name is not portable: {name!r}"
            ) from exc
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            raise NightlyRuntimeBundleError("bundle file size must be positive")
        if not isinstance(digest, str) or _SHA256_RE.fullmatch(digest) is None:
            raise NightlyRuntimeBundleError(
                "bundle file digest must be lowercase SHA-256"
            )
        if mode not in {"0644", "0755"}:
            raise NightlyRuntimeBundleError(f"bundle file mode is invalid: {mode!r}")
        assert isinstance(role, str) and isinstance(name, str)
        records.append(
            {
                "role": role,
                "name": name,
                "size_bytes": size,
                "sha256": digest,
                "mode": mode,
            }
        )
    roles = [record["role"] for record in records]
    if len(set(roles)) != len(roles):
        raise NightlyRuntimeBundleError("bundle file roles are duplicated")
    missing = [role for role in REQUIRED_ROLES if role not in roles]
    if missing:
        raise NightlyRuntimeBundleError(f"bundle is missing roles: {missing}")
    return tuple(records)


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
    actual_identity = _validated_identity(manifest.get("identity"))
    if actual_identity != expected_identity:
        raise NightlyRuntimeBundleError(
            "bundle source or target identity does not match this job"
        )
    actual_runtime_build_identity = _validated_runtime_build_identity(
        manifest.get("runtime_build_identity")
    )
    if actual_runtime_build_identity != _validated_runtime_build_identity(
        expected_runtime_build_identity
    ):
        raise NightlyRuntimeBundleError(
            "bundle runtime build identity does not match this job"
        )
    _require_bundle_runtime_identity(actual_identity, actual_runtime_build_identity)
    return _validated_file_records(manifest.get("files"))


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
    selection: NativeToolchainSelection,
    expected_identity: BundleIdentity,
    expected_runtime_build_identity: RuntimeBuildIdentity,
) -> Mapping[str, object]:
    """Verify the archive, stage its members privately, and let the CLI admit them."""
    with _open_stable_bundle(archive) as (bundle, archive_identity):
        members: dict[str, tarfile.TarInfo] = {}
        total_size = 0
        for member in bundle:
            name = _validated_member_name(member.name)
            if name in members:
                raise NightlyRuntimeBundleError(f"duplicate archive member: {name}")
            if len(members) > len(_ROLES):
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
        if manifest_member.mode != 0o644:
            raise NightlyRuntimeBundleError("bundle manifest archive mode is invalid")
        manifest_stream = bundle.extractfile(manifest_member)
        if manifest_stream is None:
            raise NightlyRuntimeBundleError("bundle manifest cannot be read")
        manifest = _read_manifest_bytes(manifest_stream.read(_MAX_MANIFEST_BYTES + 1))
        records = validate_manifest(
            manifest,
            expected_identity=expected_identity,
            expected_runtime_build_identity=expected_runtime_build_identity,
        )
        expected_names = {MANIFEST_NAME, *(_member_path(record) for record in records)}
        if set(members) != expected_names:
            missing = sorted(expected_names - set(members))
            extra = sorted(set(members) - expected_names)
            raise NightlyRuntimeBundleError(
                f"bundle member closure mismatch; missing={missing}, extra={extra}"
            )
        with tempfile.TemporaryDirectory(prefix="molt-nightly-toolchain-") as temp:
            staged: dict[str, Path] = {}
            for record in records:
                member = members[_member_path(record)]
                if member.mode != int(record["mode"], 8):
                    raise NightlyRuntimeBundleError(
                        "archive member mode does not match manifest: "
                        f"{_member_path(record)}"
                    )
                stream = bundle.extractfile(member)
                if stream is None:
                    raise NightlyRuntimeBundleError(
                        f"archive member cannot be read: {_member_path(record)}"
                    )
                role_root = Path(temp) / record["role"]
                role_root.mkdir()
                path = role_root / record["name"]
                _copy_member_exact(
                    stream,
                    path,
                    expected_size=record["size_bytes"],
                    expected_sha256=record["sha256"],
                )
                path.chmod(int(record["mode"], 8))
                staged[record["role"]] = path
            _require_unchanged(archive_identity)
            try:
                import_native_toolchain(selection, staged)
            except NativeToolchainTransferError as exc:
                raise NightlyRuntimeBundleError(
                    f"the CLI refused the bundled toolchain: {exc}"
                ) from exc
    return manifest


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Pack or admit the exact native Nightly toolchain bundle for Linux, "
            "macOS, or Windows."
        )
    )
    subparsers = parser.add_subparsers(dest="action", required=True)
    pack = subparsers.add_parser("pack")
    pack.add_argument("--project-root", type=Path, default=ROOT)
    pack.add_argument("--output", type=Path, required=True)
    pack.add_argument("--manifest-out", type=Path, required=True)
    extract = subparsers.add_parser("verify-extract")
    extract.add_argument("--project-root", type=Path, default=ROOT)
    extract.add_argument("--archive", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        selection = NativeToolchainSelection.current(args.project_root)
        runtime_build_identity = selection.runtime_build_identity()
        identity = collect_bundle_identity(
            args.project_root, runtime_build_identity=runtime_build_identity
        )
        if args.action == "pack":
            manifest = pack_bundle(
                members=export_native_toolchain(selection),
                output=args.output,
                manifest_output=args.manifest_out,
                identity=identity,
                runtime_build_identity=runtime_build_identity,
            )
        else:
            manifest = verify_extract_bundle(
                archive=args.archive,
                selection=selection,
                expected_identity=identity,
                expected_runtime_build_identity=runtime_build_identity,
            )
    except (NativeToolchainTransferError, NightlyRuntimeBundleError) as exc:
        print(f"nightly-runtime-bundle: {exc}", file=sys.stderr)
        return 1
    sys.stdout.write(dumps_exact(manifest, indent=None))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
