"""Canonical PEP 508 build-environment provenance for source extensions."""

from __future__ import annotations

from collections.abc import Mapping, Sequence

from packaging.markers import default_environment
from packaging.requirements import InvalidRequirement, Requirement
from packaging.utils import canonicalize_name
from packaging.version import InvalidVersion, Version


class SourceBuildEnvironmentError(ValueError):
    pass


# Project-owned stable schema. Recording only the standard build-marker inputs
# avoids coupling seal identity to incidental keys added by packaging releases.
SOURCE_MARKER_ENVIRONMENT_FIELDS = (
    "implementation_name",
    "implementation_version",
    "os_name",
    "platform_machine",
    "platform_python_implementation",
    "platform_release",
    "platform_system",
    "platform_version",
    "python_full_version",
    "python_version",
    "sys_platform",
)


def canonical_source_marker_environment(
    environment: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source = default_environment() if environment is None else environment
    missing = [field for field in SOURCE_MARKER_ENVIRONMENT_FIELDS if field not in source]
    if missing:
        raise SourceBuildEnvironmentError(
            "source build marker environment is missing: " + ", ".join(missing)
        )
    return {field: str(source[field]) for field in SOURCE_MARKER_ENVIRONMENT_FIELDS}


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
    if not isinstance(payload, Mapping) or set(payload) != {
        "python",
        "requirements",
        "marker_environment",
        "active_requirements",
        "resolved",
    }:
        return ["extension-set manifest build_environment shape is invalid"]

    problems: list[str] = []
    python = payload.get("python")
    requirements = payload.get("requirements")
    raw_environment = payload.get("marker_environment")
    recorded_active = payload.get("active_requirements")
    resolved = payload.get("resolved")
    if not isinstance(python, Mapping) or set(python) != {
        "implementation",
        "version",
        "executable",
    } or not all(isinstance(value, str) and value for value in python.values()):
        problems.append("extension-set manifest build Python identity is invalid")
    if (
        not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item for item in requirements)
    ):
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    if not isinstance(raw_environment, Mapping) or set(raw_environment) != set(
        SOURCE_MARKER_ENVIRONMENT_FIELDS
    ) or not all(isinstance(value, str) for value in raw_environment.values()):
        problems.append("extension-set manifest marker environment is invalid")
        return problems
    try:
        active = active_source_build_requirements(requirements, raw_environment)
    except SourceBuildEnvironmentError:
        problems.append("extension-set manifest build requirements are invalid")
        return problems
    if isinstance(python, Mapping):
        executable = str(python.get("executable", ""))
        if (
            any(separator in executable for separator in ("/", "\\"))
            or str(python.get("implementation"))
            != str(raw_environment.get("implementation_name"))
            or str(python.get("version"))
            != str(raw_environment.get("python_full_version"))
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
        if not all(
            isinstance(item.get(field), str) and item.get(field)
            for field in ("requirement", "distribution", "version")
        ):
            problems.append(
                "extension-set manifest resolved requirement values are invalid"
            )
            continue
        raw = str(item["requirement"])
        resolved_requirements.append(raw)
        if index >= len(active) or raw != active[index][0]:
            continue
        requirement = active[index][1]
        if canonicalize_name(str(item["distribution"])) != canonicalize_name(
            requirement.name
        ):
            problems.append(
                f"extension-set manifest resolved distribution does not satisfy {raw!r}"
            )
            continue
        try:
            version = Version(str(item["version"]))
        except InvalidVersion:
            problems.append(
                f"extension-set manifest resolved version is invalid for {raw!r}"
            )
            continue
        if requirement.specifier and not requirement.specifier.contains(
            version, prereleases=True
        ):
            problems.append(
                f"extension-set manifest resolved version does not satisfy {raw!r}"
            )

    if resolved_requirements != expected_active:
        if (
            len(resolved_requirements) == len(expected_active)
            and sorted(resolved_requirements) == sorted(expected_active)
        ):
            problems.append(
                "extension-set manifest resolved requirements are out of source order"
            )
        else:
            problems.append(
                "extension-set manifest resolved requirements do not exactly cover "
                "the source requirement authority"
            )
    return problems
