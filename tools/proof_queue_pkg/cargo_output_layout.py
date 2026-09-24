"""One physical Cargo-output layout, independent of canonical proof metadata.

An external root is an explicit immutable declaration, never an environment
fallback. Root object identity prevents drive-letter reuse from granting custody
of another filesystem. No directory is created by admission or validation.
"""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import shutil
import stat
import sys
from typing import Mapping, Sequence

from molt import file_publication
from molt.exact_json import canonical_json_bytes, canonical_json_sha256


ROOT_SCHEMA = "molt.proof-cargo-output-root.v1"
TARGET_LAYOUT = "molt.proof-cargo-target.v2"
_HISTORICAL_TARGET_LAYOUT = "molt.proof-cargo-target.v1"


def recorded_target_layout(record: Mapping[str, object]) -> str:
    """Absent means the historical nested address, never current acquisition."""
    version = record.get("cargo_target_layout", _HISTORICAL_TARGET_LAYOUT)
    if version not in (_HISTORICAL_TARGET_LAYOUT, TARGET_LAYOUT):
        raise ValueError("Cargo target layout version is unknown")
    return str(version)


def target_layout_fields(record: Mapping[str, object]) -> dict[str, str]:
    version = recorded_target_layout(record)
    # Do not change the shape of a historical immutable lifecycle projection.
    return {"cargo_target_layout": version} if "cargo_target_layout" in record else {}


def require_same_target_layout(
    left: Mapping[str, object], right: Mapping[str, object]
) -> None:
    if recorded_target_layout(left) != recorded_target_layout(right):
        raise ValueError("Cargo target layout binding mismatch")


def declare_root(raw: object) -> dict[str, object]:
    if not isinstance(raw, str) or not raw:
        raise ValueError("cargo_output_root must name an existing absolute directory")
    lexical = Path(raw)
    if (
        not lexical.is_absolute()
        or ".." in lexical.parts
        or lexical == Path(lexical.anchor)
    ):
        raise ValueError(
            "cargo_output_root must be an absolute non-volume-root directory"
        )
    root = file_publication.resolve_owned_path(lexical)
    try:
        metadata = root.stat()
    except OSError as exc:
        raise ValueError(
            "declared Cargo output root is unavailable; no fallback is permitted"
        ) from exc
    if not stat.S_ISDIR(metadata.st_mode) or not metadata.st_ino:
        raise ValueError(
            "cargo_output_root requires an existing identifiable directory"
        )
    return {
        "schema": ROOT_SCHEMA,
        "path": str(root),
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
    }


def declared_root(value: object) -> dict[str, object] | None:
    """Validate immutable syntax without requiring historical media online."""
    if value is None:
        return None
    if not isinstance(value, Mapping) or set(value) != {
        "schema",
        "path",
        "device",
        "inode",
    }:
        raise ValueError("Cargo output root declaration is malformed")
    if (
        value.get("schema") != ROOT_SCHEMA
        or type(value.get("device")) is not int
        or type(value.get("inode")) is not int
    ):
        raise ValueError("Cargo output root identity is malformed")
    raw = value.get("path")
    if not isinstance(raw, str) or not raw:
        raise ValueError("Cargo output root path is malformed")
    path = Path(raw)
    if (
        not path.is_absolute()
        or path == Path(path.anchor)
        or ".." in path.parts
        or str(path) != raw
        or value["inode"] <= 0
        or value["device"] < 0
    ):
        raise ValueError("Cargo output root identity is not canonical")
    return dict(value)


def validate_root(value: object) -> dict[str, object] | None:
    declaration = declared_root(value)
    if declaration is None:
        return None
    try:
        current = declare_root(declaration["path"])
    except (OSError, RuntimeError) as exc:
        raise ValueError(
            "declared Cargo output root is unavailable; no fallback is permitted"
        ) from exc
    if canonical_json_bytes(declaration) != canonical_json_bytes(current):
        raise ValueError("declared Cargo output root was replaced or remounted")
    return current


def same_root(left: object, right: object) -> bool:
    return canonical_json_bytes(left) == canonical_json_bytes(right)


def _directory_identity(path: Path) -> tuple[int, int] | None:
    try:
        metadata = path.stat(follow_symlinks=False)
    except FileNotFoundError:
        return None
    if not stat.S_ISDIR(metadata.st_mode) or not metadata.st_ino:
        raise ValueError(
            f"Cargo output boundary is not an identifiable directory: {path}"
        )
    return metadata.st_dev, metadata.st_ino


def _overlaps(left: Path, right: Path) -> bool:
    if left == right or left.is_relative_to(right) or right.is_relative_to(left):
        return True
    # Case-insensitive POSIX filesystems and bind mounts can expose one object
    # through different spellings. Compare each endpoint to the other ancestry,
    # not both ancestry sets (which would reject ordinary sibling directories).
    for endpoint, other in ((left, right), (right, left)):
        identity = _directory_identity(endpoint)
        if identity is not None and any(
            _directory_identity(ancestor) == identity
            for ancestor in (other, *other.parents)
        ):
            return True
    return False


@dataclass(frozen=True)
class CargoOutputLayout:
    result_root: Path
    declaration: Mapping[str, object] | None = None

    @classmethod
    def for_envelope(
        cls,
        envelope: Mapping[str, object],
        *,
        result_root: Path,
        source_root: Path | None = None,
    ) -> CargoOutputLayout:
        return cls.create(
            result_root=result_root,
            declaration=envelope.get("cargo_output_root"),
            source_root=source_root,
        )

    @classmethod
    def create(
        cls,
        *,
        result_root: Path,
        declaration: object = None,
        source_root: Path | None = None,
    ) -> CargoOutputLayout:
        metadata = file_publication.resolve_owned_path(result_root)
        root = validate_root(declaration)
        layout = cls(metadata, root)
        protected = [metadata] if root is not None else []
        if source_root is not None and root is not None:
            protected.append(source_root)
        layout.validate(protected_roots=protected)
        return layout

    @property
    def payload_root(self) -> Path:
        if self.declaration is None:
            return self.result_root
        namespace = hashlib.sha256(
            os.path.normcase(str(self.result_root)).encode()
        ).hexdigest()[:24]
        return Path(str(self.declaration["path"])) / "molt-proof-output" / namespace

    def validate(self, *, protected_roots: Sequence[Path] = ()) -> None:
        declaration = validate_root(self.declaration)
        root = (
            self.result_root if declaration is None else Path(str(declaration["path"]))
        )
        file_publication.resolve_owned_path(self.payload_root)
        for protected in protected_roots:
            # Protected inputs are not owned outputs: uv installations and
            # toolchain entrypoints may legitimately be aliases. Protect both
            # their names and destinations without admitting output-root links.
            lexical = Path(os.path.abspath(protected.expanduser()))
            canonical = lexical.resolve()
            if (
                root == lexical
                or root.is_relative_to(lexical)
                or lexical.is_relative_to(root)
                or _overlaps(root, canonical)
            ):
                raise ValueError(
                    f"Cargo output root overlaps protected source, metadata, or toolchain: {canonical}"
                )

    def validate_environment(self, env: Mapping[str, str]) -> None:
        """One pre-provision known-root boundary for parent and worker alike."""
        if self.declaration is None:
            return
        protected = [Path(sys.prefix), Path(sys.base_prefix)]
        protected.extend(
            Path(env[name])
            for name in ("CARGO_HOME", "RUSTUP_HOME", "VIRTUAL_ENV", "SCCACHE_DIR")
            if env.get(name)
        )
        protected.extend((Path.home() / ".cargo", Path.home() / ".rustup"))
        protected.extend(
            Path(path).parent
            for name in ("cargo", "rustc", "rustup", "uv")
            if (path := shutil.which(name, path=env.get("PATH"))) is not None
        )
        self.validate(protected_roots=protected)

    @property
    def targets_root(self) -> Path:
        root = (
            self.result_root
            if self.declaration is None
            else Path(str(self.declaration["path"]))
        )
        return file_publication.resolve_owned_path(root / "cargo-target")

    def target(
        self,
        input_sha256: str,
        generation_id: str,
        *,
        version: str = TARGET_LAYOUT,
    ) -> Path:
        if (
            re.fullmatch(r"[0-9a-f]{64}", input_sha256) is None
            or re.fullmatch(r"[0-9a-f]{16}", generation_id) is None
        ):
            raise ValueError(
                "Cargo output target requires canonical generation identity"
            )
        recorded_target_layout({"cargo_target_layout": version})
        if version == _HISTORICAL_TARGET_LAYOUT:
            return file_publication.resolve_owned_path(
                self.payload_root
                / "cargo-cache"
                / input_sha256
                / generation_id
                / "target"
            )
        # Physical addresses need one complete identity, not each identity
        # repeated as another directory. Metadata retains the explicit parts.
        # Keep native path spelling: normcase can alias case-sensitive roots.
        address = canonical_json_sha256(
            {
                "schema": TARGET_LAYOUT,
                "result_root": str(self.result_root),
                "input_sha256": input_sha256,
                "generation_id": generation_id,
            }
        )
        return file_publication.resolve_owned_path(self.targets_root / address)

    def admit_target_path(self, *, platform: str | None = None) -> None:
        """Reserve a documented tool-descendant budget before expensive capture.

        Relative MSVC inputs remain MAX_PATH-limited even when the same file
        opens by absolute path. This is a queue target-root admission budget,
        not a claim that every possible generated filename fits it.
        """
        if (sys.platform if platform is None else platform) != "win32":
            return
        target = self.target("0" * 64, "0" * 16)
        units = len(str(target).encode("utf-16-le", "surrogatepass")) // 2
        descendant_budget = 128
        if units + descendant_budget >= 260:
            raise ValueError(
                "Cargo target exceeds Windows tool path budget: "
                f"target={target}; target_utf16_units={units}; "
                f"reserved_descendant_units={descendant_budget}; limit=259; "
                "select a shorter --cargo-output-root; no fallback is permitted"
            )

    @property
    def selection(self) -> Path:
        return file_publication.resolve_owned_path(
            self.payload_root / "cargo-cache-selection"
        )

    @property
    def supervisor_target(self) -> Path:
        return file_publication.resolve_owned_path(
            self.payload_root / "proof-supervisor-target"
        )

    @property
    def temporary(self) -> Path:
        return file_publication.resolve_owned_path(self.payload_root / "tmp")

    def scratch(self, execution_nonce: str) -> Path:
        if re.fullmatch(r"[0-9a-f]{64}", execution_nonce) is None:
            raise ValueError("Cargo output scratch requires canonical execution nonce")
        return file_publication.resolve_owned_path(
            self.payload_root / "derived" / execution_nonce / "scratch"
        )

    def capacity_paths(self) -> tuple[Path, ...]:
        self.validate()
        return (
            self.targets_root,
            self.payload_root / "cargo-cache",
            self.supervisor_target,
            self.temporary,
        )
