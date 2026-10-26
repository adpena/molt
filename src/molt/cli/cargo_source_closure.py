from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Any, Sequence, TypedDict


def _dedupe_source_paths(paths: Sequence[Path]) -> list[Path]:
    deduped: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        deduped.append(path)
    return deduped


def _crate_build_inputs(
    crate_root: Path, data: dict[str, Any], project_root: Path
) -> tuple[Path, ...]:
    """The files outside a crate that its build script or ``include_str!`` reads.

    The crate declares them in ``[package.metadata.molt] build-inputs``,
    relative to its root as ``cargo:rerun-if-changed`` paths are, so the fact
    lives beside the code that reads it and every closure that reaches the
    crate carries it. Cargo's own reads verify the declaration after each
    build (``cargo_build_reads.uncovered_build_inputs``).
    """
    package = data.get("package", {})
    metadata = package.get("metadata", {}) if isinstance(package, dict) else {}
    molt = metadata.get("molt", {}) if isinstance(metadata, dict) else {}
    declared = molt.get("build-inputs", []) if isinstance(molt, dict) else []
    if not isinstance(declared, list) or not all(
        isinstance(item, str) and item for item in declared
    ):
        raise ValueError(
            f"{crate_root / 'Cargo.toml'}: package.metadata.molt.build-inputs "
            "must be a list of paths"
        )
    inputs: list[Path] = []
    for item in declared:
        path = Path(os.path.normpath(crate_root / item))
        if not Path(os.path.realpath(path)).is_relative_to(project_root):
            raise ValueError(
                f"{crate_root / 'Cargo.toml'}: build input escapes the source root: {item}"
            )
        if not path.exists():
            raise ValueError(
                f"{crate_root / 'Cargo.toml'}: declared build input does not exist: {item}"
            )
        inputs.append(path)
    return tuple(inputs)


def _crate_source_paths(crate_root: Path) -> tuple[Path, ...]:
    """Return the whole local crate as Cargo/build.rs input authority.

    Build scripts may consume headers, vendored sources, schemas, and other
    non-``src`` files. Enumerating three conventional paths silently excluded
    those inputs; the crate root is the systematic fail-closed closure.
    """

    return (crate_root,)


def _read_cargo_document(path: Path) -> dict[str, Any]:
    """Admit live TOML bytes; reuse parsing only by their content identity.

    Dependency topology and feature selection share this reader with the lock
    projection. Neither path metadata nor an old graph permits skipping a read.
    """
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION
    from molt.toolchain_identity import capture_stable_regular_file

    identity, raw = capture_stable_regular_file(
        path, label="Cargo source input", max_bytes=16 * 1024 * 1024
    )
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    digest = identity.sha256
    if transaction is not None:
        cached = transaction.cargo_documents.get(digest)
        if isinstance(cached, dict):
            return cached
    try:
        document = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"Invalid Cargo source input: {path}") from exc
    if transaction is not None:
        transaction.cargo_documents[digest] = document
    return document


def _manifest_dependency_tables(
    data: dict[str, Any], *, include_dev: bool = False
) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    kinds = (
        ("dependencies", "build-dependencies", "dev-dependencies")
        if include_dev
        else ("dependencies", "build-dependencies")
    )
    for key in kinds:
        table = data.get(key)
        if isinstance(table, dict):
            tables.append(table)
    target = data.get("target")
    if isinstance(target, dict):
        for target_table in target.values():
            if not isinstance(target_table, dict):
                continue
            for key in kinds:
                table = target_table.get(key)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def _local_path_dependencies(
    *,
    crate_root: Path,
    project_root: Path,
    data: dict[str, Any],
    workspace_dependencies: dict[str, Any],
    selected_optional_deps: set[str],
    child_features: dict[str, set[str]],
    conservative: bool = False,
) -> list[tuple[str, Path, tuple[str, ...]]]:
    deps: list[tuple[str, Path, tuple[str, ...]]] = []
    for table in _manifest_dependency_tables(data, include_dev=conservative):
        for dep_name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            dependency_base = crate_root
            if spec.get("workspace") is True:
                inherited = workspace_dependencies.get(dep_name)
                if isinstance(inherited, str):
                    continue  # Registry version, with no local source bytes.
                if not isinstance(inherited, dict):
                    raise ValueError(f"Missing Cargo workspace dependency: {dep_name}")
                inherited_features = inherited.get("features", [])
                local_features = spec.get("features", [])
                if any(
                    not isinstance(features, list)
                    or any(not isinstance(feature, str) for feature in features)
                    for features in (inherited_features, local_features)
                ):
                    raise ValueError(f"Invalid Cargo dependency features: {dep_name}")
                features = inherited_features + local_features
                spec = {**inherited, **spec, "features": features}
                dependency_base = project_root
            if "path" not in spec:
                continue
            dep_path = spec["path"]
            if not isinstance(dep_path, str) or not dep_path:
                raise ValueError(f"Invalid Cargo dependency path: {dep_name}")
            optional = bool(spec.get("optional", False))
            if optional and not conservative and dep_name not in selected_optional_deps:
                continue
            features = set(child_features.get(dep_name, set()))
            spec_features = spec.get("features", [])
            if isinstance(spec_features, list):
                features.update(
                    feature for feature in spec_features if isinstance(feature, str)
                )
            dep_root = (dependency_base / dep_path).resolve()
            deps.append((dep_name, dep_root, tuple(sorted(features))))
    return deps


def _feature_dependency_selection(
    data: dict[str, Any],
    requested_features: tuple[str, ...],
) -> tuple[set[str], dict[str, set[str]]]:
    features = data.get("features")
    if not isinstance(features, dict):
        return set(), {}
    if requested_features:
        pending = list(requested_features)
    else:
        default_features = features.get("default", [])
        pending = [item for item in default_features if isinstance(item, str)]
    seen_features: set[str] = set()
    selected_optional_deps: set[str] = set()
    child_features: dict[str, set[str]] = {}
    while pending:
        feature = pending.pop()
        if feature in seen_features:
            continue
        seen_features.add(feature)
        entries = features.get(feature, [])
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, str) or not entry:
                continue
            if entry.startswith("dep:"):
                selected_optional_deps.add(entry[4:])
                continue
            if "/" in entry:
                dep_name, child_feature = entry.split("/", 1)
                dep_name = dep_name.removesuffix("?")
                child_feature = child_feature.removesuffix("?")
                if dep_name and child_feature:
                    selected_optional_deps.add(dep_name)
                    child_features.setdefault(dep_name, set()).add(child_feature)
                continue
            pending.append(entry)
    return selected_optional_deps, child_features


def _selected_cargo_path_patches(
    project_root: Path, crate_root: Path, workspace: dict[str, Any]
) -> list[Path]:
    """Bind local overrides to Cargo's resolved package identities, not semver guesses.

    The lock closure is deliberately conservative over features and targets,
    just like the compiler dependency digest. Unreachable patches are validated
    as manifests but their source bytes are not inputs to this package.
    """
    patches = workspace.get("patch", {})
    if not isinstance(patches, dict):
        raise ValueError("Invalid Cargo patch table")
    local: dict[tuple[str, str], Path] = {}
    for registry, entries in patches.items():
        if not isinstance(registry, str) or not isinstance(entries, dict):
            raise ValueError("Invalid Cargo patch registry")
        for alias, spec in entries.items():
            if not isinstance(spec, dict):
                raise ValueError(f"Invalid Cargo patch: {alias}")
            if "path" not in spec:
                # Registry/git overrides have no local bytes; Cargo.lock binds
                # their selected source/checksum/revision through the same graph.
                continue
            path = spec["path"]
            if not isinstance(path, str) or not path or "git" in spec:
                raise ValueError(f"Invalid Cargo patch path: {alias}")
            patch_root = (project_root / path).resolve()
            if (
                not patch_root.is_relative_to(project_root)
                or patch_root == project_root
            ):
                raise ValueError(f"Cargo patch escapes the source root: {alias}")
            manifest = _read_cargo_document(patch_root / "Cargo.toml")
            package = manifest.get("package")
            if not isinstance(package, dict):
                raise ValueError(f"Cargo patch has no package: {alias}")
            name, version = package.get("name"), package.get("version")
            if (
                not isinstance(name, str)
                or not name
                or not isinstance(version, str)
                or not version
                or name != spec.get("package", alias)
            ):
                raise ValueError(f"Cargo patch package identity mismatch: {alias}")
            key = (name, version)
            if key in local and local[key] != patch_root:
                raise ValueError(f"Ambiguous Cargo path patch: {key}")
            local[key] = patch_root
    if not local:
        return []
    lock = _read_cargo_document(project_root / "Cargo.lock")
    resolved = _cargo_locked_dependency_projection(project_root, crate_root, lock=lock)
    # Cargo explicitly records unused overrides. The same name/version may
    # legitimately remain selected from another registry; do not infer registry
    # scope or version preference ourselves.
    patch_state = lock.get("patch", {})
    unused_records = (
        patch_state.get("unused", []) if isinstance(patch_state, dict) else None
    )
    if not isinstance(unused_records, list) or any(
        not isinstance(row, dict)
        or not isinstance(row.get("name"), str)
        or not isinstance(row.get("version"), str)
        for row in unused_records
    ):
        raise ValueError("Invalid Cargo.lock unused patch inventory")
    unused = {
        (row["name"], row["version"]) for row in unused_records if not row.get("source")
    }
    # An edited patch manifest can change its version while the old source-less
    # lock row remains. Require Cargo to classify every override as selected
    # somewhere in the workspace or explicitly unused before trusting the graph.
    locked_local = {
        (row.get("name"), row.get("version"))
        for row in lock["package"]
        if isinstance(row, dict)
        and isinstance(row.get("name"), str)
        and isinstance(row.get("version"), str)
        and "source" not in row
    }
    for key in local:
        if key not in locked_local and key not in unused:
            raise ValueError(f"Cargo.lock has not resolved the local patch: {key}")
    selected: list[Path] = []
    for package in resolved["packages"]:
        key = (package["name"], package["version"])
        patch_root = local.get(key)
        if patch_root is None:
            continue
        if package.get("source"):
            if key in unused:
                continue
            raise ValueError(f"Cargo.lock has not selected the local patch: {key}")
        if key in unused:
            raise ValueError(
                f"Cargo.lock both selects and excludes a local patch: {key}"
            )
        selected.append(patch_root)
    return selected


def _cargo_crate_source_closure(
    *,
    project_root: Path,
    crate_root: Path,
    crate_features: tuple[str, ...],
    extra_source_paths: Sequence[Path] = (),
) -> list[Path]:
    source_paths: list[Path] = []
    project_root_resolved = project_root.resolve()
    workspace_manifest = project_root_resolved / "Cargo.toml"
    # Standalone crate callers need no workspace override discovery.
    workspace = (
        _read_cargo_document(workspace_manifest) if workspace_manifest.exists() else {}
    )
    workspace_table = workspace.get("workspace", {})
    workspace_dependencies = (
        workspace_table.get("dependencies", {})
        if isinstance(workspace_table, dict)
        else {}
    )
    if not isinstance(workspace_dependencies, dict):
        raise ValueError("Invalid Cargo workspace dependencies")
    # Path patches may be reached through registry packages, so discover them
    # from the already-resolved lock graph, not just local manifest edges.
    pending = [(crate_root, crate_features, False)] + [
        (root, (), True)
        for root in _selected_cargo_path_patches(
            project_root_resolved, crate_root, workspace
        )
    ]
    seen: set[tuple[Path, tuple[str, ...], bool]] = set()
    while pending:
        current_root, current_features, conservative = pending.pop()
        key = (current_root, current_features, conservative)
        if key in seen:
            continue
        seen.add(key)
        source_paths.extend(_crate_source_paths(current_root))
        data = _read_cargo_document(current_root / "Cargo.toml")
        source_paths.extend(
            _crate_build_inputs(current_root, data, project_root_resolved)
        )
        selected_optional_deps, child_features = _feature_dependency_selection(
            data, current_features
        )
        for _dep_name, dep_root, dep_features in _local_path_dependencies(
            crate_root=current_root,
            project_root=project_root_resolved,
            data=data,
            workspace_dependencies=workspace_dependencies,
            selected_optional_deps=selected_optional_deps,
            child_features=child_features,
            conservative=conservative,
        ):
            if not dep_root.is_relative_to(project_root_resolved):
                raise ValueError(
                    f"Cargo path dependency escapes the source root: {dep_root}"
                )
            pending.append((dep_root, dep_features, conservative))
    source_paths.extend(extra_source_paths)
    return _dedupe_source_paths(source_paths)


def _cargo_locked_dependency_digest(project_root: Path, crate_root: Path) -> str:
    from molt.cli.compiler_identity import CompilerIdentityError

    try:
        return _project_cargo_locked_dependencies(project_root, crate_root)
    except (OSError, ValueError) as exc:
        raise CompilerIdentityError(f"Compiler dependency identity: {exc}") from exc


def _project_cargo_locked_dependencies(project_root: Path, crate_root: Path) -> str:
    from molt.exact_json import canonical_json_sha256

    return canonical_json_sha256(
        _cargo_locked_dependency_projection(project_root, crate_root)
    )


class CargoLockedProjection(TypedDict):
    schema: str
    root: tuple[str, str, str]
    lock_version: int
    packages: list[dict[str, Any]]


def _cargo_locked_dependency_projection(
    project_root: Path, crate_root: Path, *, lock: dict[str, Any] | None = None
) -> CargoLockedProjection:
    """Content identity of the root package's conservative Cargo.lock closure.

    Cargo remains the dependency resolver. Follow every recorded dependency,
    including optional, development and target edges; never infer active Cargo
    features from the lockfile. Unrelated workspace roots are not compiler inputs.
    """
    manifest = _read_cargo_document(crate_root / "Cargo.toml")
    if lock is None:
        lock = _read_cargo_document(project_root / "Cargo.lock")
    return cargo_locked_dependency_projection(manifest, lock)


def cargo_locked_dependency_projection(
    manifest: dict[str, Any], lock: dict[str, Any]
) -> CargoLockedProjection:
    """Project supplied immutable documents with the same source-identity rules.

    Historical callers supply Git objects; live source custody supplies files.
    The serialized schema and digest input are identical in both cases.
    """
    import re

    root_package = manifest.get("package")
    if not isinstance(root_package, dict):
        raise ValueError("Cargo dependency root has no package identity")
    name, version = root_package.get("name"), root_package.get("version")
    if not isinstance(name, str) or not isinstance(version, str):
        raise ValueError("Cargo dependency root requires an explicit name and version")
    root_key = (name, version, "")
    if type(lock.get("version")) is not int or lock["version"] not in {3, 4}:
        raise ValueError("Unsupported Cargo.lock version")
    packages = lock.get("package")
    if not isinstance(packages, list) or not packages:
        raise ValueError("Cargo.lock has no package inventory")
    by_key: dict[tuple[str, str, str], list[dict[str, Any]]] = {}
    by_name: dict[str, list[tuple[str, str, str]]] = {}
    for package in packages:
        # Index identities without validating unrelated workspace packages. Cargo
        # owns whole-lock validation; unreachable stale rows do not affect us.
        if not isinstance(package, dict):
            continue
        package_name = package.get("name")
        version = package.get("version")
        source = package.get("source", "")
        if not all(isinstance(value, str) for value in (package_name, version, source)):
            continue
        key = (package_name, version, source)
        by_key.setdefault(key, []).append(package)
        if key not in by_name.setdefault(package_name, []):
            by_name[package_name].append(key)
    if root_key not in by_key:
        raise ValueError(f"Cargo.lock is missing root package: {root_key}")

    def source_matches(actual: str, reference: str) -> bool:
        # Cargo serializes git SourceId references without the precise revision.
        # Keep URL/query (branch/tag/rev) identity; only omit the precise fragment
        # when the dependency reference itself omitted it.
        if reference.startswith("git+") and "#" not in reference:
            return actual.partition("#")[0] == reference
        return actual == reference

    def resolve(reference: object) -> tuple[str, str, str]:
        if not isinstance(reference, str):
            raise ValueError("Invalid Cargo.lock dependency reference")
        match = re.fullmatch(
            r"([A-Za-z0-9_-]+)(?: ([^ ()]+)(?: \(([^()]+)\))?)?", reference
        )
        if match is None:
            raise ValueError(f"Malformed Cargo.lock dependency: {reference!r}")
        dependency_name, dependency_version, dependency_source = match.groups()
        candidates = [
            key
            for key in by_name.get(dependency_name, [])
            if (dependency_version is None or key[1] == dependency_version)
            and (dependency_source is None or source_matches(key[2], dependency_source))
        ]
        if dependency_source is None:
            path_candidates = [key for key in candidates if not key[2]]
            if path_candidates:
                candidates = path_candidates
        if len(candidates) != 1:
            raise ValueError(
                f"Missing or ambiguous Cargo.lock dependency: {reference!r}"
            )
        return candidates[0]

    pending = [root_key]
    reached: dict[tuple[str, str, str], dict[str, Any]] = {}
    while pending:
        key = pending.pop()
        if key in reached:
            continue
        records = by_key[key]
        if len(records) != 1:
            raise ValueError(f"Duplicate Cargo.lock package identity: {key}")
        package = dict(records[0])
        if (
            not re.fullmatch(r"[A-Za-z0-9_-]+", key[0])
            or not key[1]
            or ("source" in package and not key[2])
        ):
            raise ValueError(f"Invalid Cargo.lock package identity: {key}")
        checksum = package.get("checksum")
        if checksum is not None and (
            not isinstance(checksum, str)
            or re.fullmatch(r"[0-9a-f]{64}", checksum) is None
        ):
            raise ValueError(f"Invalid Cargo.lock package checksum: {key}")
        dependencies = package.get("dependencies", [])
        if not isinstance(dependencies, list):
            raise ValueError(f"Invalid Cargo.lock dependencies: {key}")
        package["dependencies"] = sorted(resolve(item) for item in dependencies)
        pending.extend(package["dependencies"])
        if "replace" in package:
            package["replace"] = resolve(package["replace"])
            pending.append(package["replace"])
        reached[key] = package
    projected = [reached[key] for key in sorted(reached)]
    return {
        "schema": "molt.cargo-locked-dependency-closure.v2",
        "root": root_key,
        "lock_version": lock["version"],
        "packages": projected,
    }
