"""Read-only requirement admission for locked and direct source builds."""

from __future__ import annotations

from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass
from importlib import metadata
import sysconfig

from packaging.requirements import Requirement
from packaging.specifiers import SpecifierSet
from packaging.utils import canonicalize_name
from packaging.version import Version


@dataclass(frozen=True)
class ResolvedBuildRequirement:
    requirement: str
    distribution: str
    version: str

    def manifest_payload(self) -> dict[str, str]:
        return {
            "requirement": self.requirement,
            "distribution": self.distribution,
            "version": self.version,
        }


def realized_build_requirement(
    raw: str,
    requirement: Requirement,
    distribution: Mapping[str, object] | None,
    *,
    activated_extras: Collection[str] = (),
) -> ResolvedBuildRequirement | None:
    """Admit exact version constraints and proven extra activation, never URLs."""
    if requirement.url is not None:
        raise ValueError(
            f"installed metadata cannot attest direct URL requirement {raw!r}"
        )
    if distribution is None:
        return None
    name, version = distribution.get("name"), distribution.get("version")
    if not isinstance(name, str) or not name.strip() or not isinstance(version, str):
        raise ValueError(
            f"installed build requirement {raw!r} has invalid Name/Version metadata"
        )
    parsed = Version(version)
    if (
        canonicalize_name(name) != canonicalize_name(requirement.name)
        or not requirement.specifier.contains(parsed, prereleases=True)
        or not {canonicalize_name(extra) for extra in requirement.extras}
        <= {canonicalize_name(extra) for extra in activated_extras}
    ):
        return None
    return ResolvedBuildRequirement(raw, name, version)


def current_build_requirements(
    requirements: Sequence[tuple[str, Requirement]],
    marker_environment: Mapping[str, str],
) -> tuple[ResolvedBuildRequirement, ...]:
    """Inspect the selected interpreter's own site metadata without importing tools.

    Direct builds have no frozen receipt. Resolve only their requested dependency
    closure, including extras, against installed metadata. No installer, network,
    tool import, or environment mutation is involved. Only interpreter-owned site
    directories are considered; callers separately validate actual tool imports.
    """
    paths = sorted({sysconfig.get_path(name) for name in ("purelib", "platlib")})
    installed: dict[str, metadata.Distribution] = {}
    for distribution in metadata.distributions(path=paths):
        name = distribution.metadata.get("Name")
        if not name:
            raise ValueError("installed build distribution has no Name metadata")
        key = canonicalize_name(name)
        if key in installed:
            raise ValueError(
                f"selected interpreter has duplicate build distribution {name!r}"
            )
        installed[key] = distribution
    checked: dict[str, set[str]] = {}

    def resolve(raw: str, requirement: Requirement) -> ResolvedBuildRequirement:
        key = canonicalize_name(requirement.name)
        distribution = installed.get(key)
        message = distribution.metadata if distribution is not None else None
        extras = {canonicalize_name(extra) for extra in requirement.extras}
        resolved = realized_build_requirement(
            raw,
            requirement,
            {"name": key, "version": message.get("Version")}
            if message is not None
            else None,
            activated_extras=message.get_all("Provides-Extra", [])
            if message is not None
            else (),
        )
        if resolved is None:
            raise ValueError(
                f"selected interpreter does not satisfy build requirement {raw!r}"
            )
        assert message is not None
        if not SpecifierSet(message.get("Requires-Python", "")).contains(
            marker_environment["python_full_version"], prereleases=True
        ):
            raise ValueError(
                f"installed build distribution {key!r} does not support the selected Python version"
            )
        contexts = {"", *extras} - checked.get(key, set())
        checked.setdefault(key, set()).update(contexts)
        for dependency in message.get_all("Requires-Dist", []) if contexts else ():
            dependency_requirement = Requirement(dependency)
            if dependency_requirement.marker is None or any(
                dependency_requirement.marker.evaluate(
                    environment={**marker_environment, "extra": extra}
                )
                for extra in sorted(contexts)
            ):
                resolve(dependency, dependency_requirement)
        return resolved

    return tuple(resolve(raw, requirement) for raw, requirement in requirements)
