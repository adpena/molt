"""Exact upstream Binaryen release and host-asset authority."""

from __future__ import annotations

from dataclasses import asdict, dataclass
from functools import lru_cache
import hashlib
from pathlib import Path
import platform
import re
import tomllib

from molt.binaryen_identity import MAX_TREE_BYTES, MAX_TREE_ENTRIES
from molt.exact_json import canonical_json_bytes
from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    llvm_host_architecture,
)
from molt.portable_paths import portable_relative_path
from molt.release_matrix import RUST_TARGET_BY_COORDINATE
from molt.source_root import compiler_source_root, source_file_revision


class BinaryenConfigError(RuntimeError):
    """Raised when the pinned Binaryen authority is incomplete or invalid."""


@dataclass(frozen=True, slots=True)
class BinaryenHostAsset:
    id: str
    version: str
    url: str
    size: int
    sha256: str
    archive_root: str
    executable: str
    tree_entries: int
    tree_total_bytes: int
    tree_sha256: str
    executable_sha256: str
    record_sha256: str

    @property
    def tree_includes_modes(self) -> bool:
        """Whether this host's canonical tree identity preserves POSIX modes."""

        return not self.id.startswith("windows-")


@dataclass(frozen=True, slots=True)
class BinaryenRelease:
    version: str
    provenance_url: str
    targets: tuple[BinaryenHostAsset, ...]
    record_sha256: str


@dataclass(frozen=True, slots=True)
class BinaryenManifest:
    schema_version: int
    release: BinaryenRelease
    digest: str


_REQUIRED_HOST_IDS = frozenset(
    f"{host_platform}-{'aarch64' if architecture == 'arm64' else architecture}"
    for host_platform, architecture in RUST_TARGET_BY_COORDINATE
)
_UPSTREAM_ASSET_COORDINATES = {
    "linux-x86_64": "x86_64-linux",
    "linux-aarch64": "aarch64-linux",
    "macos-x86_64": "x86_64-macos",
    "macos-aarch64": "arm64-macos",
    "windows-x86_64": "x86_64-windows",
    "windows-aarch64": "arm64-windows",
}
_SHA256_RE = re.compile(r"[0-9a-f]{64}")


def binaryen_manifest_path(root: Path | None = None) -> Path:
    resolved_root = root.resolve() if root is not None else compiler_source_root()
    return resolved_root / "config" / "binaryen_releases.toml"


@lru_cache(maxsize=8)
def _load_binaryen_manifest_cached(
    path_text: str, _revision: tuple[int, int, int]
) -> BinaryenManifest:
    path = Path(path_text)
    try:
        raw = path.read_bytes()
        payload = tomllib.loads(raw.decode("utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise BinaryenConfigError(
            f"invalid Binaryen release manifest {path}: {exc}"
        ) from exc

    if (
        set(payload)
        != {
            "schema_version",
            "version",
            "archive_root",
            "provenance_url",
            "targets",
        }
        or payload.get("schema_version") != 1
    ):
        raise BinaryenConfigError(
            f"unsupported or non-exact Binaryen release manifest schema at {path}"
        )
    version = payload["version"]
    archive_root = payload["archive_root"]
    provenance_url = payload["provenance_url"]
    targets = payload["targets"]
    if (
        not isinstance(version, str)
        or re.fullmatch(r"[1-9]\d*", version) is None
        or archive_root != f"binaryen-version_{version}"
        or provenance_url
        != (
            "https://api.github.com/repos/WebAssembly/binaryen/releases/tags/"
            f"version_{version}"
        )
        or not isinstance(targets, dict)
    ):
        raise BinaryenConfigError(f"invalid Binaryen release identity in {path}")
    if set(targets) != _REQUIRED_HOST_IDS:
        raise BinaryenConfigError(
            "Binaryen targets differ from the complete shipped release matrix in "
            f"{path}: expected {sorted(_REQUIRED_HOST_IDS)}, found {sorted(targets)}"
        )

    assets: list[BinaryenHostAsset] = []
    for target_id, target in targets.items():
        if not isinstance(target_id, str) or not isinstance(target, dict):
            raise BinaryenConfigError(f"invalid Binaryen target row in {path}")
        if set(target) != {
            "url",
            "size",
            "sha256",
            "executable",
            "tree_entries",
            "tree_total_bytes",
            "tree_sha256",
            "executable_sha256",
        }:
            raise BinaryenConfigError(
                f"Binaryen target {target_id} keys are not exact in {path}"
            )
        url = target["url"]
        size = target["size"]
        sha256 = target["sha256"]
        executable = target["executable"]
        tree_entries = target["tree_entries"]
        tree_total_bytes = target["tree_total_bytes"]
        tree_sha256 = target["tree_sha256"]
        executable_sha256 = target["executable_sha256"]
        upstream_coordinate = _UPSTREAM_ASSET_COORDINATES[target_id]
        expected_url = (
            "https://github.com/WebAssembly/binaryen/releases/download/"
            f"version_{version}/{archive_root}-{upstream_coordinate}.tar.gz"
        )
        expected_executable = (
            "bin/wasm-opt.exe" if target_id.startswith("windows-") else "bin/wasm-opt"
        )
        try:
            executable_path = portable_relative_path(executable)
        except ValueError as exc:
            raise BinaryenConfigError(
                f"Binaryen target {target_id} executable is not portable in {path}"
            ) from exc
        if (
            url != expected_url
            or type(size) is not int
            or size <= 0
            or not isinstance(sha256, str)
            or _SHA256_RE.fullmatch(sha256) is None
            or not isinstance(tree_sha256, str)
            or _SHA256_RE.fullmatch(tree_sha256) is None
            or type(tree_entries) is not int
            or not 0 < tree_entries <= MAX_TREE_ENTRIES
            or type(tree_total_bytes) is not int
            or not 0 < tree_total_bytes <= MAX_TREE_BYTES
            or not isinstance(executable_sha256, str)
            or _SHA256_RE.fullmatch(executable_sha256) is None
            or executable_path.as_posix() != expected_executable
        ):
            raise BinaryenConfigError(
                f"Binaryen target {target_id} identity is invalid in {path}"
            )
        record = {
            "id": target_id,
            "version": version,
            "url": url,
            "size": size,
            "sha256": sha256,
            "archive_root": archive_root,
            "executable": executable_path.as_posix(),
            "tree_entries": tree_entries,
            "tree_total_bytes": tree_total_bytes,
            "tree_sha256": tree_sha256,
            "executable_sha256": executable_sha256,
        }
        assets.append(
            BinaryenHostAsset(
                **record,
                record_sha256=hashlib.sha256(canonical_json_bytes(record)).hexdigest(),
            )
        )

    sorted_assets = tuple(sorted(assets, key=lambda asset: asset.id))
    release_record = {
        "version": version,
        "provenance_url": provenance_url,
        "targets": [
            {
                key: value
                for key, value in asdict(asset).items()
                if key != "record_sha256"
            }
            for asset in sorted_assets
        ],
    }
    release_record_sha256 = hashlib.sha256(
        canonical_json_bytes(release_record)
    ).hexdigest()
    return BinaryenManifest(
        schema_version=1,
        release=BinaryenRelease(
            version=version,
            provenance_url=provenance_url,
            targets=sorted_assets,
            record_sha256=release_record_sha256,
        ),
        digest=hashlib.sha256(raw).hexdigest(),
    )


def load_binaryen_manifest(root: Path | None = None) -> BinaryenManifest:
    path = binaryen_manifest_path(root)
    return _load_binaryen_manifest_cached(str(path), source_file_revision(path))


def binaryen_host_asset(
    root: Path,
    *,
    system: str | None = None,
    machine: str | None = None,
) -> BinaryenHostAsset:
    system_name = system or platform.system()
    platform_id = {
        "Linux": "linux",
        "Darwin": "macos",
        "Windows": "windows",
    }.get(system_name)
    raw_machine = machine or platform.machine()
    try:
        architecture = llvm_host_architecture(root, raw_machine)
    except LlvmToolchainConfigError as exc:
        raise BinaryenConfigError(
            f"cannot resolve Binaryen host architecture {raw_machine!r}: {exc}"
        ) from exc
    if platform_id is None or architecture is None:
        raise BinaryenConfigError(
            "unsupported Binaryen host coordinate: "
            f"system={system_name!r} machine={raw_machine!r}"
        )
    target_id = f"{platform_id}-{architecture.id}"
    asset = next(
        (
            item
            for item in load_binaryen_manifest(root).release.targets
            if item.id == target_id
        ),
        None,
    )
    if asset is None:
        raise BinaryenConfigError(
            f"Binaryen manifest has no host asset for {target_id}"
        )
    return asset
