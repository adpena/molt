"""Declared-input identity and ordinary selection of published Python images.

This is a metadata selection boundary. Proof ingress separately captures the
installed/runtime bytes and validates the full published image attestation.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path
import stat
import tomllib
from typing import TypedDict

from molt import python_environment_contract as _contract
from molt.exact_json import canonical_json_bytes, read_exact
from molt.file_publication import metadata_is_link_like
from molt.python_identity_common import _canonicalize_name, _valid_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)

DeclaredInputDigests = dict[str, str | None]


def valid_declared_input_digests(value: object) -> bool:
    return (
        isinstance(value, dict)
        and {"pyproject.toml", "uv.lock", ".python-version"}.issubset(value)
        and all(
            isinstance(key, str)
            and key
            and (_valid_sha256(digest) or (key == ".python-version" and digest is None))
            for key, digest in value.items()
        )
    )


@dataclass(frozen=True)
class SourceBuildInputs:
    digests: DeclaredInputDigests
    editable_binding: dict[str, object] | None
    dependency_groups: tuple[str, ...]
    _files: tuple[tuple[Path, StableRegularFileIdentity], ...]
    _absent: tuple[Path, ...]

    def verify(self) -> None:
        for lexical, identity in self._files:
            if lexical.resolve(strict=True) != identity.path:
                raise _contract.SourceBuildEnvironmentError(
                    "environment metadata input changed its source binding"
                )
            verify_stable_regular_file_identity(
                identity, label="environment declared input"
            )
        if any(os.path.lexists(path) for path in self._absent):
            raise _contract.SourceBuildEnvironmentError(
                "environment recipe inputs changed during publication"
            )


def capture_source_build_inputs(
    repo_root: Path, *, editable: bool
) -> SourceBuildInputs:
    """Read each declared file once, discovering metadata from current TOML.

    Metadata aliases within the source root retain their existing supported
    meaning. Publication fences both their resolution and physical target.
    """
    repo_root = repo_root.resolve(strict=True)
    digests: DeclaredInputDigests = {}
    files: list[tuple[Path, StableRegularFileIdentity]] = []
    absent: list[Path] = []

    def capture(
        relative: str, *, optional: bool = False, metadata: bool = False
    ) -> bytes:
        # Resolve metadata using its declared spelling: collapsing '..' before
        # following a symlink component changes the file the build backend sees.
        lexical = repo_root / relative
        if not metadata and not lexical.is_relative_to(repo_root):
            raise _contract.SourceBuildEnvironmentError(
                "editable metadata input escapes source root"
            )
        key = lexical.relative_to(repo_root).as_posix()
        if optional and not os.path.lexists(lexical):
            digests[key] = None
            absent.append(lexical)
            return b""
        target = lexical.resolve(strict=True) if metadata else lexical
        if not target.is_relative_to(repo_root):
            raise _contract.SourceBuildEnvironmentError(
                "editable metadata input escapes source root"
            )
        identity, data = capture_stable_regular_file(
            target, label="environment declared input"
        )
        files.append((lexical, identity))
        digests[key] = identity.sha256
        return data

    try:
        project_bytes = capture("pyproject.toml")
        document = tomllib.loads(project_bytes.decode("utf-8"))
        capture("uv.lock")
        capture(".python-version", optional=True)
        binding: dict[str, object] | None = None
        if editable:
            project = document.get("project", {})
            if (
                not isinstance(project, dict)
                or project.get("dynamic")
                or not isinstance(project.get("name"), str)
                or not isinstance(project.get("version"), str)
            ):
                raise _contract.SourceBuildEnvironmentError(
                    "development image requires declared static project metadata"
                )
            metadata_inputs = {"pyproject.toml": digests["pyproject.toml"]}
            for field in ("readme", "license"):
                declaration = project.get(field)
                filename = (
                    declaration
                    if isinstance(declaration, str) and field == "readme"
                    else declaration.get("file")
                    if isinstance(declaration, dict)
                    else None
                )
                if filename is not None:
                    if not isinstance(filename, str):
                        raise _contract.SourceBuildEnvironmentError(
                            "editable metadata file declaration is invalid"
                        )
                    capture(filename, metadata=True)
                    identity = files[-1][1]
                    metadata_inputs[identity.path.relative_to(repo_root).as_posix()] = (
                        identity.sha256
                    )
            name = project["name"]
            assert isinstance(name, str)
            binding = {
                "source_root": str(repo_root),
                "project": _canonicalize_name(name),
                "version": project["version"],
                "metadata_inputs": metadata_inputs,
            }
        groups = document.get("dependency-groups", {})
        result = SourceBuildInputs(
            digests,
            binding,
            tuple(sorted(groups)) if isinstance(groups, dict) else (),
            tuple(files),
            tuple(absent),
        )
        result.verify()
        return result
    except (OSError, UnicodeError, ValueError) as exc:
        raise _contract.SourceBuildEnvironmentError(
            f"cannot read source-build declared inputs: {exc}"
        ) from exc


class DevelopmentSelectionRecord(TypedDict):
    schema: str
    producer_contract: _contract.SourceBuildEnvironmentContract
    environment_id: str
    checkout: str
    python_version: str
    groups: list[str]
    declared_inputs: DeclaredInputDigests
    editable_binding: dict[str, object]


def _selection_record_limit(description: Mapping[str, object]) -> int:
    # Coordinates, declared filenames, metadata and group names have no fixed
    # path-length cap. Only the runtime-version scalar uses reserved overhead.
    return len(canonical_json_bytes({**description, "python_version": ""})) + 65536


def development_selection_record(
    root: Path, custody: Mapping[str, object]
) -> DevelopmentSelectionRecord:
    """Project only address-bound facts, before the image itself is captured."""
    binding = custody["editable_binding"]
    runtime = custody["python_runtime"]
    groups = custody["dependency_groups"]
    inputs = custody["declared_inputs"]
    if (
        not isinstance(binding, dict)
        or not isinstance(binding.get("source_root"), str)
        or not isinstance(runtime, dict)
        or not isinstance(groups, list)
        or not all(isinstance(group, str) for group in groups)
        or not isinstance(inputs, dict)
        or not all(
            isinstance(key, str) and (value is None or _valid_sha256(value))
            for key, value in inputs.items()
        )
        or not _contract.source_build_environment_contract_matches(
            custody["producer_contract"]
        )
    ):
        raise _contract.SourceBuildEnvironmentError(
            "invalid development selection recipe"
        )
    version = _contract.parse_canonical_python_runtime_version(runtime.get("version"))
    record: DevelopmentSelectionRecord = {
        "schema": _contract.DEVELOPMENT_ENVIRONMENT_SELECTION_SCHEMA,
        "producer_contract": _contract.source_build_environment_contract(),
        "environment_id": root.name,
        "checkout": binding["source_root"],
        "python_version": version.text,
        "groups": groups,
        "declared_inputs": inputs,
        "editable_binding": binding,
    }
    if len(canonical_json_bytes(record)) > _selection_record_limit(record):
        raise _contract.SourceBuildEnvironmentError(
            "development runtime version exceeds selection record budget"
        )
    return record


@dataclass(frozen=True)
class DevelopmentEnvironmentSelection:
    root: Path
    python_executable: Path
    python_version: _contract.CanonicalPythonRuntimeVersion
    groups: tuple[str, ...]


def development_coordinates(root: Path, repo_root: Path) -> tuple[Path, Path]:
    from molt.dx import checkout_component

    lexical = Path(os.path.abspath(root))
    repo_root = repo_root.resolve(strict=True)
    if root.is_symlink() or root.resolve(strict=True) != lexical:
        raise _contract.SourceBuildEnvironmentError(
            "development image selection must be its physical root"
        )
    if (
        not _valid_sha256(lexical.name)
        or lexical.parent.name != checkout_component(repo_root)
        or lexical.parent.parent.name != "development"
        or lexical.parent.parent.parent.name != "build-environments"
    ):
        raise _contract.SourceBuildEnvironmentError(
            "development image is outside its checkout generation namespace"
        )
    return lexical, repo_root


def read_development_environment_selection(
    root: Path,
    *,
    repo_root: Path,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
) -> DevelopmentEnvironmentSelection:
    """Select without a subprocess, image scan, write or full-manifest read."""
    from molt.python_environment_location import (
        environment_provisioning_record_path,
        environment_python_path,
        validate_managed_bytecode_prefix,
    )
    from molt.python_interpreter import (
        PythonInterpreterRequest,
        parse_python_interpreter_request,
    )

    try:
        root, repo_root = development_coordinates(root, repo_root)
        if os.path.lexists(environment_provisioning_record_path(root)):
            raise ValueError("development image publication is incomplete")
        manifest = root / _contract.SOURCE_BUILD_ENVIRONMENT_MANIFEST
        metadata = manifest.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata_is_link_like(metadata):
            raise ValueError("development image has no physical final manifest")
        inputs = capture_source_build_inputs(repo_root, editable=True)
        description = {
            "schema": _contract.DEVELOPMENT_ENVIRONMENT_SELECTION_SCHEMA,
            "producer_contract": _contract.source_build_environment_contract(),
            "environment_id": root.name,
            "checkout": str(repo_root),
            "python_version": "",
            "groups": list(inputs.dependency_groups),
            "declared_inputs": inputs.digests,
            "editable_binding": inputs.editable_binding,
        }
        payload = read_exact(
            root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION,
            max_bytes=_selection_record_limit(description),
            label="development selection record",
        )
        if (
            not isinstance(payload, dict)
            or set(payload) != set(DevelopmentSelectionRecord.__annotations__)
            or payload["schema"] != _contract.DEVELOPMENT_ENVIRONMENT_SELECTION_SCHEMA
            or not _contract.source_build_environment_contract_matches(
                payload["producer_contract"]
            )
            or payload["environment_id"] != root.name
            or payload["checkout"] != str(repo_root)
            or payload["declared_inputs"] != inputs.digests
            or payload["editable_binding"] != inputs.editable_binding
            or not isinstance(payload["groups"], list)
            or not all(isinstance(group, str) for group in payload["groups"])
            or payload["groups"] != sorted(set(payload["groups"]))
            or not set(payload["groups"]).issubset(inputs.dependency_groups)
            or (groups is not None and payload["groups"] != sorted(set(groups)))
        ):
            raise ValueError(
                "development selection record differs from current intent/inputs"
            )
        version = _contract.parse_canonical_python_runtime_version(
            payload["python_version"]
        )
        # The interpreter request type owns the supported physical-version
        # domain; the record parser separately preserves prerelease identity.
        PythonInterpreterRequest(*version.release)
        if python is not None:
            request = parse_python_interpreter_request(python)
            if version.is_prerelease or not request.matches_release(version.release):
                raise ValueError(
                    "selected development image has a different requested Python"
                )
        validate_managed_bytecode_prefix(root)
        inputs.verify()
        return DevelopmentEnvironmentSelection(
            root, environment_python_path(root), version, tuple(payload["groups"])
        )
    except (OSError, ValueError) as exc:
        raise _contract.SourceBuildEnvironmentError(
            f"cannot select published development image: {exc}; run canonical ensure"
        ) from exc
