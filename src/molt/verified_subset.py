"""Exact policy and release coordinates for Molt's verified subset."""

from __future__ import annotations

import platform as platform_module
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.file_publication import is_link_like
from molt.portable_paths import portable_relative_path
from molt.release_matrix import RELEASE_TARGETS, SUPPORTED_CPYTHON_VERSIONS
from molt.target_python import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS

ROOT = Path(__file__).resolve().parents[2]
POLICY_PATH = ROOT / "config" / "verified_subset.toml"
SCHEMA = "molt.verified-subset.v1"
_POLICY_KEYS = frozenset(
    {
        "schema",
        "reference_cpython",
        "differential_suites",
        "excluded_verification_scopes",
    }
)
_FALLBACK_POLICY = "error"
_ABI = "cpython-language"
_CONCURRENCY = "gil"
_SUPPORTED_BACKENDS = ("native", "wasm")


@dataclass(frozen=True, slots=True)
class VerifiedSubsetSuite:
    path: str
    recursive: bool
    cpython_equivalence_floor: int

    def as_record(self) -> dict[str, object]:
        return {
            "cpython_equivalence_floor": self.cpython_equivalence_floor,
            "path": self.path,
            "recursive": self.recursive,
        }


@dataclass(frozen=True, slots=True)
class VerifiedSubsetPolicy:
    python_versions: tuple[str, ...]
    reference_cpython: tuple[str, ...]
    excluded_verification_scopes: tuple[str, ...]
    fallback_policy: str
    abi: str
    concurrency: str
    backends: tuple[str, ...]
    suites: tuple[VerifiedSubsetSuite, ...]

    @property
    def suite_selectors(self) -> tuple[tuple[str, bool], ...]:
        return tuple((suite.path, suite.recursive) for suite in self.suites)


@dataclass(frozen=True, slots=True)
class VerifiedSubsetCoordinate:
    id: str
    python: str
    reference_python: str
    abi: str
    backend: str
    concurrency: str
    platform: str
    arch: str
    rust_target: str
    runner: str

    def as_record(self) -> dict[str, str]:
        return {
            "id": self.id,
            "python": self.python,
            "reference_python": self.reference_python,
            "abi": self.abi,
            "backend": self.backend,
            "concurrency": self.concurrency,
            "platform": self.platform,
            "arch": self.arch,
            "rust_target": self.rust_target,
            "runner": self.runner,
        }


def _exact_string(value: object, *, field: str) -> str:
    if not isinstance(value, str) or not value or value.strip() != value:
        raise ValueError(f"verified-subset {field} must be a non-empty exact string")
    return value


def _exact_string_list(value: object, *, field: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValueError(f"verified-subset {field} must be an array")
    items = tuple(_exact_string(item, field=field) for item in value)
    if not items or items != tuple(sorted(set(items))):
        raise ValueError(
            f"verified-subset {field} must be non-empty, sorted, and unique"
        )
    return items


def _exact_suite_list(value: object) -> tuple[VerifiedSubsetSuite, ...]:
    if not isinstance(value, list) or not value:
        raise ValueError(
            "verified-subset differential_suites must be a non-empty array"
        )
    suites: list[VerifiedSubsetSuite] = []
    for index, item in enumerate(value):
        keys = {"cpython_equivalence_floor", "path", "recursive"}
        if not isinstance(item, Mapping) or set(item) != keys:
            raise ValueError(
                "verified-subset differential_suites entries must contain exactly "
                "cpython_equivalence_floor, path, and recursive"
            )
        suite_path = _exact_string(
            item.get("path"), field=f"differential_suites[{index}].path"
        )
        recursive = item.get("recursive")
        if not isinstance(recursive, bool):
            raise ValueError(
                f"verified-subset differential_suites[{index}].recursive "
                "must be boolean"
            )
        equivalence_floor = item.get("cpython_equivalence_floor")
        if isinstance(equivalence_floor, bool) or not isinstance(
            equivalence_floor, int
        ):
            raise ValueError(
                "verified-subset differential_suites"
                f"[{index}].cpython_equivalence_floor must be an integer"
            )
        if equivalence_floor <= 0:
            raise ValueError(
                "verified-subset suite CPython-equivalence floors must be positive"
            )
        suites.append(
            VerifiedSubsetSuite(
                path=suite_path,
                recursive=recursive,
                cpython_equivalence_floor=equivalence_floor,
            )
        )
    ordered = tuple(sorted(suites, key=lambda suite: suite.path))
    if tuple(suites) != ordered or len({suite.path for suite in suites}) != len(suites):
        raise ValueError(
            "verified-subset differential_suites must be path-sorted and unique"
        )
    return ordered


def load_verified_subset_policy(path: Path = POLICY_PATH) -> VerifiedSubsetPolicy:
    with path.open("rb") as stream:
        document = tomllib.load(stream)
    if set(document) != _POLICY_KEYS or document.get("schema") != SCHEMA:
        raise ValueError("verified-subset policy schema or keys are not exact")
    if tuple(SUPPORTED_CPYTHON_VERSIONS) != SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS:
        raise ValueError(
            "generated release Python versions drifted from TargetPythonVersion"
        )
    suites = _exact_suite_list(document.get("differential_suites"))
    policy_root = path.resolve(strict=True).parents[1]
    for suite in suites:
        relative = portable_relative_path(suite.path)
        candidate = policy_root.joinpath(*relative.parts)
        resolved = candidate.resolve(strict=True)
        if (
            is_link_like(candidate)
            or candidate.absolute() != resolved
            or not resolved.is_relative_to(policy_root)
            or not resolved.is_dir()
        ):
            raise ValueError(
                f"verified-subset suite is not a source directory: {suite.path}"
            )
    reference_cpython = _exact_string_list(
        document.get("reference_cpython"), field="reference_cpython"
    )
    reference_by_minor: dict[str, str] = {}
    for version in reference_cpython:
        parts = version.split(".")
        if len(parts) != 3 or not all(part.isdigit() for part in parts):
            raise ValueError(
                "verified-subset reference_cpython must contain final micro versions"
            )
        minor = ".".join(parts[:2])
        if minor in reference_by_minor:
            raise ValueError("verified-subset reference_cpython minors must be unique")
        reference_by_minor[minor] = version
    if set(reference_by_minor) != set(SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS):
        raise ValueError(
            "verified-subset reference_cpython must cover every target Python minor"
        )
    excluded_verification_scopes = _exact_string_list(
        document.get("excluded_verification_scopes"),
        field="excluded_verification_scopes",
    )
    if excluded_verification_scopes != (
        "capability_policy",
        "dynamic_execution_policy",
    ):
        raise ValueError(
            "verified-subset exclusions must be exactly the capability-policy and "
            "dynamic-execution-policy scopes"
        )
    return VerifiedSubsetPolicy(
        python_versions=SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS,
        reference_cpython=tuple(
            reference_by_minor[minor]
            for minor in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
        ),
        excluded_verification_scopes=excluded_verification_scopes,
        fallback_policy=_FALLBACK_POLICY,
        abi=_ABI,
        concurrency=_CONCURRENCY,
        backends=_SUPPORTED_BACKENDS,
        suites=suites,
    )


def _release_target_records() -> tuple[Mapping[str, Any], ...]:
    records = tuple(dict(item) for item in RELEASE_TARGETS)
    ids = [str(item.get("id")) for item in records]
    if not records or len(ids) != len(set(ids)):
        raise ValueError("release targets must be non-empty and unique")
    return tuple(sorted(records, key=lambda item: str(item["id"])))


def verified_subset_coordinates(
    policy: VerifiedSubsetPolicy | None = None,
) -> tuple[VerifiedSubsetCoordinate, ...]:
    resolved_policy = policy or load_verified_subset_policy()
    coordinates: list[VerifiedSubsetCoordinate] = []
    reference_by_minor = dict(
        zip(
            resolved_policy.python_versions,
            resolved_policy.reference_cpython,
            strict=True,
        )
    )
    for target in _release_target_records():
        for python_version in resolved_policy.python_versions:
            python_tag = python_version.replace(".", "")
            for backend in resolved_policy.backends:
                coordinate_id = (
                    f"{target['id']}-py{python_tag}-{resolved_policy.abi}-"
                    f"{resolved_policy.concurrency}-{backend}"
                )
                coordinates.append(
                    VerifiedSubsetCoordinate(
                        id=coordinate_id,
                        python=python_version,
                        reference_python=reference_by_minor[python_version],
                        abi=resolved_policy.abi,
                        backend=backend,
                        concurrency=resolved_policy.concurrency,
                        platform=str(target["platform"]),
                        arch=str(target["arch"]),
                        rust_target=str(target["rust_target"]),
                        runner=str(target["runner"]),
                    )
                )
    coordinates.sort(key=lambda item: item.id)
    ids = [coordinate.id for coordinate in coordinates]
    expected_count = (
        len(_release_target_records())
        * len(resolved_policy.python_versions)
        * len(resolved_policy.backends)
    )
    if len(coordinates) != expected_count or ids != sorted(set(ids)):
        raise ValueError("verified-subset coordinate closure is not exact")
    return tuple(coordinates)


def verified_subset_coordinate_by_id(
    coordinate_id: str,
) -> VerifiedSubsetCoordinate:
    matches = [
        coordinate
        for coordinate in verified_subset_coordinates()
        if coordinate.id == coordinate_id
    ]
    if len(matches) != 1:
        raise ValueError(f"unknown verified-subset coordinate: {coordinate_id}")
    return matches[0]


def current_host_coordinate() -> tuple[str, str]:
    platform_name = {
        "darwin": "macos",
        "linux": "linux",
        "windows": "windows",
    }.get(platform_module.system().strip().lower())
    raw_arch = platform_module.machine().strip().lower()
    architecture = {
        "amd64": "x86_64",
        "x86_64": "x86_64",
        "aarch64": "aarch64",
        "arm64": "arm64",
    }.get(raw_arch)
    if platform_name is None or architecture is None:
        raise ValueError(
            "current host is outside the verified-subset release matrix: "
            f"platform={platform_module.system()!r}, arch={raw_arch!r}"
        )
    if platform_name in {"macos", "windows"} and architecture == "aarch64":
        architecture = "arm64"
    if platform_name == "linux" and architecture == "arm64":
        architecture = "aarch64"
    return platform_name, architecture


def require_current_host(coordinate: VerifiedSubsetCoordinate) -> None:
    current = current_host_coordinate()
    expected = (coordinate.platform, coordinate.arch)
    if current != expected:
        raise ValueError(
            f"verified-subset coordinate {coordinate.id} requires host "
            f"{expected[0]}/{expected[1]}, current host is {current[0]}/{current[1]}"
        )
