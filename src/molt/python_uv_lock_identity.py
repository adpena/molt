"""Marker-selected uv.lock package and wheel identity authority."""

from __future__ import annotations

import tomllib
import urllib.parse
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import cast

from molt.exact_json import canonical_json_sha256
from molt.python_identity_common import (
    identity_validator,
    PythonEnvironmentIdentityError,
    _canonicalize_name,
    _valid_sha256,
)

UV_LOCK_GROUP_CLOSURE_SCHEMA = "molt.uv-lock-group-closure.v2"
PYTHON_MARKER_ENVIRONMENT_FIELDS = (
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
_LOCK_CLOSURE_FIELDS = frozenset(
    {
        "schema",
        "lock_version",
        "lock_revision",
        "requires_python",
        "dependency_group",
        "requirements",
        "project_requirements",
        "marker_environment",
        "packages",
        "closure_sha256",
    }
)


def _marker_applies(raw: object, environment: Mapping[str, str]) -> bool:
    from packaging.markers import InvalidMarker, Marker

    if raw is None:
        return True
    if not isinstance(raw, str) or not raw.strip():
        raise PythonEnvironmentIdentityError("uv.lock dependency marker is invalid")
    try:
        return Marker(raw).evaluate(environment=environment)
    except InvalidMarker as exc:
        raise PythonEnvironmentIdentityError(
            f"uv.lock dependency marker is invalid: {raw!r}"
        ) from exc


def _row_applies(row: Mapping[str, object], environment: Mapping[str, str]) -> bool:
    raw = row.get("resolution-markers")
    if raw is None:
        return True
    if not isinstance(raw, list) or not raw:
        raise PythonEnvironmentIdentityError("uv.lock resolution markers are invalid")
    return sum(_marker_applies(item, environment) for item in raw) > 0


def _extras(raw: object) -> set[str]:
    if (
        not isinstance(raw, list)
        or not all(
            isinstance(item, str) and item and item == _canonicalize_name(item)
            for item in raw
        )
        or len(set(raw)) != len(raw)
    ):
        raise PythonEnvironmentIdentityError("uv.lock dependency extras are invalid")
    return set(cast(list[str], raw))


def _locked_artifact(row: Mapping[str, object]) -> dict[str, object]:
    from packaging.tags import sys_tags
    from packaging.utils import parse_wheel_filename
    from packaging.version import InvalidVersion, Version

    rank = {tag: index for index, tag in enumerate(sys_tags())}
    candidates: list[tuple[int, str, Mapping[str, object]]] = []
    wheels = row.get("wheels")
    if not isinstance(wheels, list):
        raise PythonEnvironmentIdentityError(
            f"locked package {row.get('name')!r} has no wheel closure"
        )
    for raw in wheels:
        if not isinstance(raw, Mapping) or not isinstance(raw.get("url"), str):
            continue
        raw = cast(Mapping[str, object], raw)
        filename = urllib.parse.unquote(
            Path(urllib.parse.urlparse(str(raw["url"])).path).name
        )
        try:
            parsed_name, parsed_version, _build, tags = parse_wheel_filename(filename)
        except ValueError:
            continue
        try:
            locked_version = Version(str(row.get("version", "")))
        except InvalidVersion as exc:
            raise PythonEnvironmentIdentityError(
                f"locked package {row.get('name')!r} has an invalid version"
            ) from exc
        if (
            _canonicalize_name(parsed_name)
            != _canonicalize_name(str(row.get("name", "")))
            or parsed_version != locked_version
        ):
            continue
        supported = [rank[tag] for tag in tags if tag in rank]
        if supported:
            candidates.append((min(supported), filename, raw))
    if not candidates:
        raise PythonEnvironmentIdentityError(
            f"locked package {row.get('name')!r} has no wheel for the active OS/arch/ABI"
        )
    candidates.sort(key=lambda item: (item[0], item[1]))
    best_rank = candidates[0][0]
    best = [item for item in candidates if item[0] == best_rank]
    if len(best) != 1:
        raise PythonEnvironmentIdentityError(
            f"locked package {row.get('name')!r} has ambiguous best-matching wheels"
        )
    _rank, filename, artifact = best[0]
    raw_hash = artifact.get("hash")
    size = artifact.get("size")
    if (
        not isinstance(raw_hash, str)
        or not raw_hash.startswith("sha256:")
        or not _valid_sha256(raw_hash.removeprefix("sha256:"))
        or type(size) is not int
        or size <= 0
    ):
        raise PythonEnvironmentIdentityError(
            f"locked package {row.get('name')!r} wheel identity is invalid"
        )
    return {
        "filename": filename,
        "size": size,
        "sha256": raw_hash.removeprefix("sha256:"),
    }


@identity_validator("uv.lock group selection")
def selected_uv_lock_group_closure(
    repo_root: Path,
    dependency_group: str,
    requirements: Sequence[str],
    *,
    marker_environment: Mapping[str, str] | None = None,
) -> dict[str, object]:
    """Match uv sync --group GROUP --no-install-project, including root dependencies."""

    from packaging.markers import default_environment
    from packaging.specifiers import InvalidSpecifier, SpecifierSet
    from packaging.version import InvalidVersion, Version

    try:
        document = tomllib.loads((repo_root / "uv.lock").read_text(encoding="utf-8"))
        project = tomllib.loads(
            (repo_root / "pyproject.toml").read_text(encoding="utf-8")
        )["project"]
    except (OSError, UnicodeError, KeyError, tomllib.TOMLDecodeError) as exc:
        raise PythonEnvironmentIdentityError(
            f"cannot read uv project lock authority: {exc}"
        ) from exc
    raw_environment = dict(
        default_environment() if marker_environment is None else marker_environment
    )
    if set(raw_environment) != set(PYTHON_MARKER_ENVIRONMENT_FIELDS) or not all(
        isinstance(value, str) for value in raw_environment.values()
    ):
        raise PythonEnvironmentIdentityError(
            "uv.lock marker environment shape is invalid"
        )
    environment = cast(dict[str, str], raw_environment)
    lock_version = document.get("version")
    lock_revision = document.get("revision")
    requires_python = document.get("requires-python")
    if (
        type(lock_version) is not int
        or lock_version != 1
        or type(lock_revision) is not int
        or lock_revision not in {1, 2, 3}
        or not isinstance(requires_python, str)
        or not requires_python
    ):
        raise PythonEnvironmentIdentityError("uv.lock schema is unsupported")
    try:
        supported_python = SpecifierSet(requires_python)
        selected_python = Version(environment["python_full_version"])
    except (InvalidSpecifier, InvalidVersion) as exc:
        raise PythonEnvironmentIdentityError(
            "uv.lock Python version authority is invalid"
        ) from exc
    if not supported_python.contains(selected_python, prereleases=True):
        raise PythonEnvironmentIdentityError(
            "selected Python does not satisfy uv.lock requires-python"
        )
    resolution_markers = document.get("resolution-markers")
    if lock_revision >= 3:
        if (
            not isinstance(resolution_markers, list)
            or not resolution_markers
            or not all(isinstance(item, str) and item for item in resolution_markers)
            or sum(_marker_applies(item, environment) for item in resolution_markers)
            != 1
        ):
            raise PythonEnvironmentIdentityError(
                "uv.lock resolution-marker partition is invalid"
            )
    elif resolution_markers is not None and (
        not isinstance(resolution_markers, list)
        or not resolution_markers
        or not all(isinstance(item, str) and item for item in resolution_markers)
    ):
        raise PythonEnvironmentIdentityError("uv.lock resolution markers are invalid")
    packages = document.get("package")
    project_name = project.get("name") if isinstance(project, Mapping) else None
    if not isinstance(packages, list) or not isinstance(project_name, str):
        raise PythonEnvironmentIdentityError("uv project lock authority is malformed")
    rows = [row for row in packages if isinstance(row, Mapping)]
    project_rows = [
        row
        for row in rows
        if _canonicalize_name(str(row.get("name", "")))
        == _canonicalize_name(project_name)
        and isinstance(row.get("dev-dependencies"), Mapping)
        and dependency_group in row["dev-dependencies"]  # type: ignore[operator]
    ]
    if len(project_rows) != 1:
        raise PythonEnvironmentIdentityError(
            f"uv.lock has {len(project_rows)} project rows for group {dependency_group!r}"
        )
    project_requirements = project.get("dependencies", [])
    project_edges = project_rows[0].get("dependencies", [])
    if (
        not isinstance(project_requirements, list)
        or not all(isinstance(item, str) and item for item in project_requirements)
        or not isinstance(project_edges, list)
    ):
        raise PythonEnvironmentIdentityError("uv.lock project dependencies are invalid")
    group_edges = project_rows[0]["dev-dependencies"][dependency_group]  # type: ignore[index]
    if not isinstance(group_edges, list):
        raise PythonEnvironmentIdentityError(
            f"uv.lock group {dependency_group!r} has no dependency closure"
        )
    rows_by_name: dict[str, list[Mapping[str, object]]] = {}
    for row in rows:
        raw_name = row.get("name")
        if isinstance(raw_name, str):
            rows_by_name.setdefault(_canonicalize_name(raw_name), []).append(row)

    def select(
        edge: object, context: Mapping[str, str]
    ) -> tuple[Mapping[str, object], set[str]] | None:
        if not isinstance(edge, Mapping) or not isinstance(edge.get("name"), str):
            raise PythonEnvironmentIdentityError("uv.lock dependency edge is malformed")
        edge = cast(Mapping[str, object], edge)
        extras = _extras(edge.get("extra", []))
        if "version" in edge and not isinstance(edge["version"], str):
            raise PythonEnvironmentIdentityError(
                "uv.lock dependency version is invalid"
            )
        if "source" in edge and not isinstance(edge["source"], Mapping):
            raise PythonEnvironmentIdentityError("uv.lock dependency source is invalid")
        if not _marker_applies(edge.get("marker"), context):
            return None
        name = _canonicalize_name(str(edge["name"]))
        candidates = list(rows_by_name.get(name, ()))
        if isinstance(edge.get("version"), str):
            candidates = [
                row for row in candidates if row.get("version") == edge["version"]
            ]
        if isinstance(edge.get("source"), Mapping):
            candidates = [
                row for row in candidates if row.get("source") == edge["source"]
            ]
        candidates = [row for row in candidates if _row_applies(row, environment)]
        if len(candidates) != 1:
            raise PythonEnvironmentIdentityError(
                f"uv.lock dependency {name!r} resolves to {len(candidates)} active rows"
            )
        return candidates[0], extras

    pending: list[tuple[object, str | None, Mapping[str, str]]] = [
        (edge, None, environment) for edge in [*project_edges, *group_edges]
    ]
    selected: dict[str, Mapping[str, object]] = {}
    activated: dict[str, set[str]] = {}
    graph: dict[str, dict[tuple[str, str], set[str]]] = {}
    while pending:
        edge, parent, context = pending.pop()
        selection = select(edge, context)
        if selection is None:
            continue
        row, extras = selection
        name = _canonicalize_name(str(row["name"]))
        if parent is not None:
            graph[parent].setdefault((context.get("extra", ""), name), set()).update(
                extras
            )
        previous = selected.get(name)
        if previous is not None:
            if previous != row:
                raise PythonEnvironmentIdentityError(
                    f"uv.lock selected two rows for dependency {name!r}"
                )
        else:
            selected[name] = row
            activated[name] = set()
            graph[name] = {}
        new_contexts = (extras - activated[name]) | (
            {""} if previous is None else set()
        )
        activated[name].update(extras)
        dependencies = row.get("dependencies", [])
        if not isinstance(dependencies, list):
            raise PythonEnvironmentIdentityError(
                f"uv.lock dependency list is malformed for {name!r}"
            )
        optional = row.get("optional-dependencies", {})
        if not isinstance(optional, Mapping):
            raise PythonEnvironmentIdentityError(
                f"uv.lock optional dependencies are malformed for {name!r}"
            )
        for extra in sorted(new_contexts):
            # uv can retain an extra request with no optional edges (including
            # empty extras). The lock graph, not wheel metadata guessed here,
            # owns which additional dependencies that request activates.
            extra_dependencies = optional.get(extra, []) if extra else []
            if not isinstance(extra_dependencies, list):
                raise PythonEnvironmentIdentityError(
                    f"uv.lock optional dependency list is malformed for {name!r}"
                )
            extra_context = {**environment, "extra": extra}
            pending.extend(
                (dependency, name, extra_context)
                for dependency in [*dependencies, *extra_dependencies]
            )
    locked_packages: list[dict[str, object]] = []
    for name, row in sorted(selected.items()):
        version = row.get("version")
        source = row.get("source")
        if (
            not isinstance(version, str)
            or not isinstance(source, Mapping)
            or set(source) != {"registry"}
            or not isinstance(source.get("registry"), str)
            or not source.get("registry")
        ):
            raise PythonEnvironmentIdentityError(
                f"uv.lock package row is incomplete for {name!r}"
            )
        locked_packages.append(
            {
                "name": name,
                "version": version,
                "source": dict(source),
                "artifact": _locked_artifact(row),
                "extras": sorted(activated[name]),
                "dependencies": [
                    {
                        "name": dependency,
                        "extras": sorted(extras),
                        "when_extra": when_extra,
                    }
                    for (when_extra, dependency), extras in sorted(graph[name].items())
                ],
            }
        )
    material = {
        "schema": UV_LOCK_GROUP_CLOSURE_SCHEMA,
        "lock_version": lock_version,
        "lock_revision": lock_revision,
        "requires_python": requires_python,
        "dependency_group": dependency_group,
        "requirements": list(requirements),
        "project_requirements": project_requirements,
        "marker_environment": environment,
        "packages": locked_packages,
    }
    return validate_uv_lock_group_closure(
        {**material, "closure_sha256": canonical_json_sha256(material)}
    )


@identity_validator("uv.lock group closure")
def validate_uv_lock_group_closure(payload: object) -> dict[str, object]:
    if (
        not isinstance(payload, dict)
        or set(payload) != _LOCK_CLOSURE_FIELDS
        or payload.get("schema") != UV_LOCK_GROUP_CLOSURE_SCHEMA
    ):
        raise PythonEnvironmentIdentityError("uv.lock group closure shape is invalid")
    payload = cast(dict[str, object], payload)
    material = dict(payload)
    digest = material.pop("closure_sha256", None)
    if not _valid_sha256(digest) or digest != canonical_json_sha256(material):
        raise PythonEnvironmentIdentityError("uv.lock group closure digest is invalid")
    packages = payload.get("packages")
    requirements = payload.get("requirements")
    project_requirements = payload.get("project_requirements")
    raw_marker_environment = payload.get("marker_environment")
    if (
        type(payload.get("lock_version")) is not int
        or payload.get("lock_version") != 1
        or type(payload.get("lock_revision")) is not int
        or payload.get("lock_revision") not in {1, 2, 3}
        or not isinstance(payload.get("requires_python"), str)
        or not payload.get("requires_python")
        or not isinstance(payload.get("dependency_group"), str)
        or not payload.get("dependency_group")
        or not isinstance(requirements, list)
        or not requirements
        or not all(isinstance(item, str) and item for item in requirements)
        or not isinstance(project_requirements, list)
        or not all(isinstance(item, str) and item for item in project_requirements)
        or not isinstance(raw_marker_environment, Mapping)
        or not isinstance(packages, list)
    ):
        raise PythonEnvironmentIdentityError("uv.lock group closure is empty")
    marker_mapping = cast(Mapping[str, object], raw_marker_environment)
    if set(marker_mapping) != set(PYTHON_MARKER_ENVIRONMENT_FIELDS) or not all(
        isinstance(value, str) for value in marker_mapping.values()
    ):
        raise PythonEnvironmentIdentityError(
            "uv.lock marker environment shape is invalid"
        )
    from packaging.requirements import InvalidRequirement, Requirement
    from packaging.specifiers import InvalidSpecifier, SpecifierSet
    from packaging.utils import parse_wheel_filename
    from packaging.version import InvalidVersion, Version

    marker_environment = {str(key): str(value) for key, value in marker_mapping.items()}
    try:
        requires_python = SpecifierSet(str(payload["requires_python"]))
        selected_python = Version(marker_environment["python_full_version"])
    except (InvalidSpecifier, InvalidVersion) as exc:
        raise PythonEnvironmentIdentityError(
            "uv.lock Python version authority is invalid"
        ) from exc
    if not requires_python.contains(selected_python, prereleases=True):
        raise PythonEnvironmentIdentityError(
            "uv.lock marker Python does not satisfy requires-python"
        )
    names: set[str] = set()
    package_order: list[str] = []
    versions: dict[str, Version] = {}
    package_extras: dict[str, set[str]] = {}
    package_edges: dict[str, list[Mapping[str, object]]] = {}
    for package in packages:
        if not isinstance(package, Mapping):
            raise PythonEnvironmentIdentityError("uv.lock package closure is invalid")
        package = cast(Mapping[str, object], package)
        name = package.get("name")
        artifact = package.get("artifact")
        source = package.get("source")
        if (
            set(package)
            != {"name", "version", "source", "artifact", "extras", "dependencies"}
            or not isinstance(name, str)
            or not name
            or name != _canonicalize_name(name)
            or name in names
            or not isinstance(package.get("version"), str)
            or not package.get("version")
            or not isinstance(source, Mapping)
            or not isinstance(artifact, Mapping)
        ):
            raise PythonEnvironmentIdentityError("uv.lock package closure is invalid")
        source = cast(Mapping[str, object], source)
        artifact = cast(Mapping[str, object], artifact)
        artifact_size = artifact.get("size")
        if (
            set(source) != {"registry"}
            or not isinstance(source.get("registry"), str)
            or not source.get("registry")
            or set(artifact) != {"filename", "size", "sha256"}
            or not isinstance(artifact.get("filename"), str)
            or not artifact.get("filename")
            or any(
                separator in str(artifact.get("filename")) for separator in ("/", "\\")
            )
            or type(artifact_size) is not int
            or artifact_size <= 0
            or not _valid_sha256(artifact.get("sha256"))
        ):
            raise PythonEnvironmentIdentityError("uv.lock package closure is invalid")
        try:
            wheel_name, wheel_version, _build, _tags = parse_wheel_filename(
                str(artifact["filename"])
            )
            locked_version = Version(str(package["version"]))
        except (InvalidVersion, ValueError) as exc:
            raise PythonEnvironmentIdentityError(
                "uv.lock package wheel identity is invalid"
            ) from exc
        if _canonicalize_name(wheel_name) != name or wheel_version != locked_version:
            raise PythonEnvironmentIdentityError(
                "uv.lock package wheel identity differs from its package row"
            )
        names.add(name)
        package_order.append(name)
        versions[name] = locked_version
        extras = _extras(package.get("extras"))
        if package.get("extras") != sorted(extras):
            raise PythonEnvironmentIdentityError(
                "uv.lock package extras are not canonical"
            )
        package_extras[name] = extras
        dependencies = package.get("dependencies")
        if not isinstance(dependencies, list):
            raise PythonEnvironmentIdentityError(
                "uv.lock package dependency graph is invalid"
            )
        edge_order: list[tuple[str, str]] = []
        edges: list[Mapping[str, object]] = []
        for edge in dependencies:
            if not isinstance(edge, Mapping) or set(edge) != {
                "name",
                "extras",
                "when_extra",
            }:
                raise PythonEnvironmentIdentityError(
                    "uv.lock package dependency edge is invalid"
                )
            dependency = edge.get("name")
            when_extra = edge.get("when_extra")
            if (
                not isinstance(dependency, str)
                or not dependency
                or dependency != _canonicalize_name(dependency)
                or not isinstance(when_extra, str)
                or (when_extra and when_extra not in extras)
                or edge.get("extras") != sorted(_extras(edge.get("extras")))
            ):
                raise PythonEnvironmentIdentityError(
                    "uv.lock package dependency edge is invalid"
                )
            edge_order.append((when_extra, dependency))
            edges.append(cast(Mapping[str, object], edge))
        if edge_order != sorted(set(edge_order)):
            raise PythonEnvironmentIdentityError(
                "uv.lock package dependency graph is not canonical"
            )
        package_edges[name] = edges
    if package_order != sorted(package_order):
        raise PythonEnvironmentIdentityError("uv.lock package closure is not canonical")
    roots: list[tuple[str, set[str]]] = []
    for raw in [*project_requirements, *requirements]:
        try:
            requirement = Requirement(str(raw))
        except InvalidRequirement as exc:
            raise PythonEnvironmentIdentityError(
                f"uv.lock group requirement is invalid: {raw!r}"
            ) from exc
        if requirement.url is not None:
            raise PythonEnvironmentIdentityError(
                "uv.lock group requirement contains a direct URL"
            )
        if requirement.marker is not None and not requirement.marker.evaluate(
            environment=marker_environment
        ):
            continue
        selected_version = versions.get(_canonicalize_name(requirement.name))
        if selected_version is None or (
            requirement.specifier
            and not requirement.specifier.contains(selected_version, prereleases=True)
        ):
            raise PythonEnvironmentIdentityError(
                f"uv.lock group closure does not satisfy {raw!r}"
            )
        roots.append(
            (
                _canonicalize_name(requirement.name),
                {_canonicalize_name(extra) for extra in requirement.extras},
            )
        )
    reached: dict[str, set[str]] = {}
    pending = roots
    while pending:
        name, extras = pending.pop()
        if name not in names or not extras <= package_extras[name]:
            raise PythonEnvironmentIdentityError(
                "uv.lock dependency graph has a missing package or extra"
            )
        previous = reached.get(name)
        contexts = (extras - (previous or set())) | (
            {""} if previous is None else set()
        )
        reached.setdefault(name, set()).update(extras)
        for edge in package_edges[name]:
            if edge["when_extra"] in contexts:
                pending.append((str(edge["name"]), _extras(edge["extras"])))
    if reached != package_extras:
        raise PythonEnvironmentIdentityError(
            "uv.lock dependency graph contains unreachable packages or extras"
        )
    return payload


def environment_matches_lock_closure(
    environment: Mapping[str, object], lock_closure: Mapping[str, object]
) -> bool:
    distributions = environment.get("distributions")
    packages = lock_closure.get("packages")
    if not isinstance(distributions, list) or not isinstance(packages, list):
        return False
    actual = sorted(
        (str(row.get("name")), str(row.get("version")))
        for row in distributions
        if isinstance(row, Mapping)
    )
    expected = sorted(
        (str(row.get("name")), str(row.get("version")))
        for row in packages
        if isinstance(row, Mapping)
    )
    return (
        len(actual) == len(distributions)
        and len(expected) == len(packages)
        and actual == expected
    )
