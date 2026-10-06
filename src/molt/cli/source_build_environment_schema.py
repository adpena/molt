"""Source-build environment declarations and read-only manifest validation."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TypedDict, cast

from packaging.markers import default_environment
from packaging.requirements import InvalidRequirement, Requirement
from packaging.utils import canonicalize_name

from molt.cli.source_build_requirements import realized_build_requirement
from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_environment_custody import validate_python_environment_identity
from molt.python_runtime_identity import validate_python_runtime_identity
from molt.python_uv_lock_identity import (
    PYTHON_MARKER_ENVIRONMENT_FIELDS,
    environment_matches_lock_closure,
    validate_uv_lock_group_closure,
)


class SourceBuildEnvironmentError(ValueError):
    pass


SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION = 5


SOURCE_BUILD_ENVIRONMENT_MANIFEST = "molt-source-build-environment.json"


class _SourceBuildAddress(TypedDict):
    schema_version: int
    dependency_group: str
    dependency_group_requirements: list[str]
    lock_closure: dict[str, object]
    python_runtime: dict[str, object]
    uv: dict[str, str]


class _SourceBuildCustody(_SourceBuildAddress):
    environment_id: str


@dataclass(frozen=True)
class LockedSourceBuildEnvironment:
    root: Path
    python_executable: Path
    manifest_path: Path
    custody: Mapping[str, object]
    active: bool


def canonical_source_marker_environment(
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source = default_environment() if environment is None else environment
    missing = [
        field for field in PYTHON_MARKER_ENVIRONMENT_FIELDS if field not in source
    ]
    if missing:
        raise SourceBuildEnvironmentError(
            "source build marker environment is missing: " + ", ".join(missing)
        )
    return {field: str(source[field]) for field in PYTHON_MARKER_ENVIRONMENT_FIELDS}


def _marker_environment_matches_runtime(
    marker: Mapping[str, str], runtime: Mapping[str, object]
) -> bool:
    version = str(runtime.get("version", ""))
    version_parts = version.split(".")
    architecture = {
        "amd64": "x86_64",
        "x86_64": "x86_64",
        "arm64": "arm64",
        "aarch64": "arm64",
    }.get(marker.get("platform_machine", "").casefold())
    os_identity = {
        "windows": ("nt", "win32", "Windows"),
        "macos": ("posix", "darwin", "Darwin"),
        "linux": ("posix", "linux", "Linux"),
    }.get(str(runtime.get("operating_system", "")))
    return (
        len(version_parts) == 3
        and os_identity is not None
        and marker.get("implementation_name") == "cpython"
        and marker.get("platform_python_implementation") == "CPython"
        and marker.get("implementation_version") == version
        and marker.get("python_full_version") == version
        and marker.get("python_version") == ".".join(version_parts[:2])
        and architecture == runtime.get("architecture")
        and (
            marker.get("os_name"),
            marker.get("sys_platform"),
            marker.get("platform_system"),
        )
        == os_identity
    )


def active_source_build_requirements(
    requirements: Sequence[str], marker_environment: Mapping[str, str]
) -> tuple[tuple[str, Requirement], ...]:
    environment = canonical_source_marker_environment(marker_environment)
    active: list[tuple[str, Requirement]] = []
    for raw in requirements:
        try:
            requirement = Requirement(raw)
        except InvalidRequirement as exc:
            raise SourceBuildEnvironmentError(
                f"invalid source build requirement {raw!r}: {exc}"
            ) from exc
        if requirement.url is not None:
            raise SourceBuildEnvironmentError(
                "source build requirements with direct URLs cannot be revalidated "
                f"from installed distribution metadata: {raw!r}"
            )
        if requirement.marker is None or requirement.marker.evaluate(
            environment=environment
        ):
            active.append((raw, requirement))
    return tuple(active)


def source_build_environment_problems(payload: object) -> list[str]:
    expected_fields = {
        "python",
        "requirements",
        "marker_environment",
        "active_requirements",
        "resolved",
        "custody",
    }
    if not isinstance(payload, Mapping) or set(payload) != expected_fields:
        return ["extension-set manifest build_environment shape is invalid"]
    payload = cast(Mapping[str, object], payload)
    problems: list[str] = []
    validated_lock: Mapping[str, object] | None = None
    validated_environment: Mapping[str, object] | None = None

    custody = payload.get("custody")
    recipe_keys = {
        "schema_version",
        "environment_id",
        "dependency_group",
        "dependency_group_requirements",
        "lock_closure",
        "python_runtime",
        "uv",
    }
    if not isinstance(custody, Mapping) or set(custody) != {
        *recipe_keys,
        "realized_environment",
    }:
        problems.append("extension-set manifest build-environment custody is invalid")
        custody = {}
    else:
        custody = cast(Mapping[str, object], custody)
        recipe = {key: custody[key] for key in recipe_keys if key != "environment_id"}
        environment_id = custody.get("environment_id")
        if (
            type(custody.get("schema_version")) is not int
            or custody.get("schema_version") != SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION
            or not isinstance(environment_id, str)
            or len(environment_id) != 64
            or any(character not in "0123456789abcdef" for character in environment_id)
            or environment_id != canonical_json_sha256(recipe)
        ):
            problems.append(
                "extension-set manifest build-environment address digest is invalid"
            )
        lock_closure = custody.get("lock_closure")
        python_runtime = custody.get("python_runtime")
        realized = custody.get("realized_environment")
        try:
            validated_lock = validate_uv_lock_group_closure(lock_closure)
            validated_runtime = validate_python_runtime_identity(python_runtime)
            validated_environment = validate_python_environment_identity(realized)
        except PythonEnvironmentIdentityError:
            problems.append(
                "extension-set manifest build-environment content custody is invalid"
            )
        else:
            if (
                validated_environment.get("runtime") != validated_runtime
                or not environment_matches_lock_closure(
                    validated_environment, validated_lock
                )
                or validated_lock.get("dependency_group")
                != custody.get("dependency_group")
                or validated_lock.get("requirements")
                != custody.get("dependency_group_requirements")
            ):
                problems.append(
                    "extension-set manifest build-environment content differs from recipe"
                )
        uv = custody.get("uv")
        if (
            not isinstance(uv, Mapping)
            or set(uv) != {"executable", "version", "sha256"}
            or not all(
                isinstance(uv.get(field), str) and uv.get(field)
                for field in ("executable", "version")
            )
            or any(separator in str(uv.get("executable")) for separator in ("/", "\\"))
            or not isinstance(uv.get("sha256"), str)
            or len(str(uv.get("sha256"))) != 64
            or any(
                character not in "0123456789abcdef"
                for character in str(uv.get("sha256"))
            )
        ):
            problems.append("extension-set manifest uv custody is invalid")

    python = payload.get("python")
    requirements = payload.get("requirements")
    raw_environment = payload.get("marker_environment")
    recorded_active = payload.get("active_requirements")
    resolved = payload.get("resolved")
    if (
        not isinstance(python, Mapping)
        or set(python) != {"implementation", "version", "executable"}
        or not all(isinstance(value, str) and value for value in python.values())
    ):
        problems.append("extension-set manifest build Python identity is invalid")
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item for item in requirements)
    ):
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    requirements = [item for item in requirements if isinstance(item, str) and item]
    if (
        not isinstance(raw_environment, Mapping)
        or set(raw_environment) != set(PYTHON_MARKER_ENVIRONMENT_FIELDS)
        or not all(isinstance(value, str) for value in raw_environment.values())
    ):
        problems.append("extension-set manifest marker environment is invalid")
        return problems
    marker_environment = {
        str(key): value
        for key, value in raw_environment.items()
        if isinstance(key, str) and isinstance(value, str)
    }
    if (
        validated_lock is not None
        and validated_lock.get("marker_environment") != marker_environment
    ):
        problems.append(
            "extension-set manifest build-environment lock marker environment "
            "differs from the recorded marker environment"
        )
    if validated_environment is not None and not _marker_environment_matches_runtime(
        marker_environment, validated_environment
    ):
        problems.append(
            "extension-set manifest marker environment differs from the realized "
            "Python runtime"
        )
    try:
        active = active_source_build_requirements(requirements, marker_environment)
    except SourceBuildEnvironmentError:
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    if isinstance(python, Mapping):
        executable = str(python.get("executable", ""))
        realized = custody.get("realized_environment")
        selected = (
            realized.get("selected_executable")
            if isinstance(realized, Mapping)
            else None
        )
        selected_path = (
            str(selected.get("path", "")) if isinstance(selected, Mapping) else ""
        )
        if (
            any(separator in executable for separator in ("/", "\\"))
            or str(python.get("implementation"))
            != str(marker_environment.get("implementation_name"))
            or str(python.get("version"))
            != str(marker_environment.get("python_full_version"))
            or Path(selected_path).name != executable
        ):
            problems.append("extension-set manifest build Python identity is invalid")
    expected_active = [raw for raw, _requirement in active]
    if recorded_active != expected_active:
        problems.append(
            "extension-set manifest active requirements do not match the recorded "
            "marker environment"
        )
    if not isinstance(resolved, list) or not resolved:
        problems.append("extension-set manifest resolved requirements are empty")
        return problems

    raw_realized_distributions = (
        validated_environment.get("distributions")
        if validated_environment is not None
        else None
    )
    realized_distributions = (
        raw_realized_distributions
        if isinstance(raw_realized_distributions, list)
        else []
    )
    realized_versions: dict[str, str] = {}
    activated_extras = (
        {
            str(row["name"]): row["extras"]
            for row in cast(
                list[Mapping[str, object]], validated_lock.get("packages", [])
            )
        }
        if validated_lock is not None
        else {}
    )
    for row in realized_distributions:
        if not isinstance(row, Mapping):
            continue
        typed_row = cast(Mapping[str, object], row)
        name = typed_row.get("name")
        version = typed_row.get("version")
        if isinstance(name, str) and isinstance(version, str):
            realized_versions[canonicalize_name(name)] = version
    resolved_requirements: list[str] = []
    for index, item in enumerate(resolved):
        if not isinstance(item, Mapping) or set(item) != {
            "requirement",
            "distribution",
            "version",
        }:
            problems.append(
                "extension-set manifest resolved requirement shape is invalid"
            )
            continue
        item = cast(Mapping[str, object], item)
        if not all(
            isinstance(item.get(field), str) and item.get(field)
            for field in ("requirement", "distribution", "version")
        ):
            problems.append(
                "extension-set manifest resolved requirement values are invalid"
            )
            continue
        raw = cast(str, item["requirement"])
        distribution = cast(str, item["distribution"])
        raw_version = cast(str, item["version"])
        resolved_requirements.append(raw)
        if index >= len(active) or raw != active[index][0]:
            continue
        requirement = active[index][1]
        try:
            resolution = realized_build_requirement(
                raw,
                requirement,
                {"name": distribution, "version": raw_version},
                activated_extras=cast(
                    Sequence[str],
                    activated_extras.get(canonicalize_name(distribution), ()),
                ),
            )
        except ValueError:
            problems.append(
                f"extension-set manifest resolved version is invalid for {raw!r}"
            )
            continue
        if resolution is None:
            problems.append(
                f"extension-set manifest resolved version or extras does not satisfy {raw!r}"
            )
        if realized_versions.get(canonicalize_name(distribution)) != raw_version:
            problems.append(
                "extension-set manifest resolved distribution differs from the "
                f"realized environment for {raw!r}"
            )

    if resolved_requirements != expected_active:
        if len(resolved_requirements) == len(expected_active) and sorted(
            resolved_requirements
        ) == sorted(expected_active):
            problems.append(
                "extension-set manifest resolved requirements are out of source order"
            )
        else:
            problems.append(
                "extension-set manifest resolved requirements do not exactly cover "
                "the source requirement authority"
            )
    return problems
