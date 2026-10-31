"""One physical Cargo-output layout, independent of canonical proof metadata.

An external root is an explicit immutable declaration, never an environment
fallback. Root object identity prevents drive-letter reuse from granting custody
of another filesystem. No directory is created by admission or validation.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
import hashlib
import os
from pathlib import Path
import re
import shutil
import stat
import sys
from typing import Mapping, Sequence

from molt import custody_layout, file_publication
from molt.dx import artifact_root, checkout_component, storage_path
from molt.durable_file_identity import (
    DurableFileIdentity,
    file_identity,
)
from molt.exact_json import canonical_json_sha256


ROOT_SCHEMA = "molt.proof-cargo-output-root.v2"
TARGET_LAYOUT = "molt.proof-cargo-target.v2"
_HISTORICAL_TARGET_LAYOUT = "molt.proof-cargo-target.v1"


def implementation_source_root() -> Path:
    """Own bootstrap placement independently of the admitted guest source."""
    return Path(__file__).resolve().parents[2]


def recorded_target_layout(record: Mapping[str, object]) -> str:
    """Absent means the historical nested address, never current acquisition."""
    version = record.get("cargo_target_layout", _HISTORICAL_TARGET_LAYOUT)
    if version not in (_HISTORICAL_TARGET_LAYOUT, TARGET_LAYOUT):
        raise ValueError("Cargo target layout version is unknown")
    return str(version)


def target_layout_fields(record: Mapping[str, object]) -> dict[str, str]:
    version = recorded_target_layout(record)
    # Do not change the shape of a historical immutable lifecycle projection.
    fields = {"cargo_target_layout": version} if "cargo_target_layout" in record else {}
    workspace = record.get("workspace_sha256")
    if workspace is not None:
        if (
            not isinstance(workspace, str)
            or re.fullmatch(r"[0-9a-f]{64}", workspace) is None
        ):
            raise ValueError("Cargo workspace identity is malformed")
        fields["workspace_sha256"] = workspace
    return fields


def require_same_target_layout(
    left: Mapping[str, object], right: Mapping[str, object]
) -> None:
    if recorded_target_layout(left) != recorded_target_layout(right) or left.get(
        "workspace_sha256"
    ) != right.get("workspace_sha256"):
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
        "identity": file_identity(root, expected=metadata).payload(),
    }


def declared_root(value: object) -> dict[str, object] | None:
    """Validate immutable syntax without requiring historical media online."""
    if value is None:
        return None
    if not isinstance(value, Mapping) or set(value) != {"schema", "path", "identity"}:
        raise ValueError("Cargo output root declaration is malformed")
    if value.get("schema") != ROOT_SCHEMA:
        raise ValueError("Cargo output root identity is malformed")
    DurableFileIdentity.from_payload(value["identity"])
    raw = value.get("path")
    if not isinstance(raw, str) or not raw:
        raise ValueError("Cargo output root path is malformed")
    path = Path(raw)
    if (
        not path.is_absolute()
        or path == Path(path.anchor)
        or ".." in path.parts
        or str(path) != raw
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
    if declaration["path"] != current["path"] or DurableFileIdentity.from_payload(
        declaration["identity"]
    ) != DurableFileIdentity.from_payload(current["identity"]):
        raise ValueError("declared Cargo output root was replaced or remounted")
    return current


def same_root(left: object, right: object) -> bool:
    # A malformed declaration names no root, so it matches nothing; each
    # caller then refuses with its own binding mismatch.
    try:
        a, b = declared_root(left), declared_root(right)
    except ValueError:
        return False
    if a is None or b is None:
        return a is b
    return a["path"] == b["path"] and DurableFileIdentity.from_payload(
        a["identity"]
    ) == DurableFileIdentity.from_payload(b["identity"])


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
    supervisor_declaration: Mapping[str, object] | None = None

    @classmethod
    def for_envelope(
        cls,
        envelope: Mapping[str, object],
        *,
        result_root: Path,
        source_root: Path | None = None,
        supervisor_artifact_root: object = None,
    ) -> CargoOutputLayout:
        return cls.create(
            result_root=result_root,
            declaration=envelope.get("cargo_output_root"),
            source_root=source_root,
            supervisor_artifact_root=supervisor_artifact_root,
        )

    @classmethod
    def create(
        cls,
        *,
        result_root: Path,
        declaration: object = None,
        source_root: Path | None = None,
        supervisor_artifact_root: object = None,
    ) -> CargoOutputLayout:
        metadata = file_publication.resolve_owned_path(result_root)
        root = validate_root(declaration)
        supervisor_root = validate_root(supervisor_artifact_root)
        if (
            root is not None
            and supervisor_root is not None
            and not same_root(root, supervisor_root)
        ):
            raise ValueError(
                "supervisor artifact root differs from declared Cargo output root"
            )
        layout = cls(metadata, root, supervisor_root)
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
        return (
            Path(str(self.declaration["path"])) / "molt-proof-output" / "v2" / namespace
        )

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
        self.admit_cargo_path(target, platform=platform)

    def admit_supervisor_target_path(self, *, platform: str | None = None) -> None:
        """Apply the same native tool budget to control-plane Cargo builds."""
        if (sys.platform if platform is None else platform) != "win32":
            return
        self.admit_cargo_path(self.supervisor_target, platform=platform)

    @staticmethod
    def admit_cargo_path(target: Path, *, platform: str | None = None) -> None:
        if (sys.platform if platform is None else platform) != "win32":
            return
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
    def supervisor_artifact_root(self) -> dict[str, object]:
        """The selected control-plane storage declaration, never payload env."""
        if self.supervisor_declaration is not None:
            selected = validate_root(self.supervisor_declaration)
            assert selected is not None
            return selected
        return self._select_supervisor_artifact_root(os.environ)

    def _select_supervisor_artifact_root(
        self, environment: Mapping[str, str]
    ) -> dict[str, object]:
        if self.declaration is not None:
            selected = validate_root(self.declaration)
            assert selected is not None
            return selected
        return declare_root(
            str(artifact_root(implementation_source_root(), environment))
        )

    def bind_supervisor_artifact_root(
        self, environment: Mapping[str, str]
    ) -> CargoOutputLayout:
        """Freeze the parent's selection before launching a scrubbed child."""
        bound = replace(
            self,
            supervisor_declaration=self._select_supervisor_artifact_root(environment),
        )
        # Refuse source/receipt overlap at the same owner used by the child.
        _ = bound.supervisor_store
        return bound

    @property
    def supervisor_store(self) -> Path:
        # Bootstrap source belongs to this implementation, never the guest cwd
        # or a cached Python fixture. Result paths cannot fragment this store.
        source = implementation_source_root()
        root = Path(str(self.supervisor_artifact_root["path"]))
        store = file_publication.resolve_owned_path(
            storage_path(source, "proof-supervisor", {"MOLT_EXT_ROOT": str(root)})
            / checkout_component(source)
        )
        if any(
            _overlaps(store, protected)
            for protected in (
                source,
                self.result_root,
                *custody_layout.enclosing_checkouts(store),
            )
        ):
            raise ValueError(
                "supervisor store overlaps source or receipt custody; "
                "bind an external artifact root with `python -m "
                "molt.checkout_provisioning artifact-root --bind /absolute/root` "
                "or select an external --cargo-output-root"
            )
        return store

    @property
    def supervisor_target(self) -> Path:
        return file_publication.resolve_owned_path(self.supervisor_store / "target")

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
