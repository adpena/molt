from __future__ import annotations

import hashlib
import tomllib
from pathlib import Path
from typing import Any, Sequence


def _dedupe_source_paths(paths: Sequence[Path]) -> list[Path]:
    deduped: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        deduped.append(path)
    return deduped


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
    from molt.toolchain_identity import open_stable_regular_file

    with open_stable_regular_file(path, label="Cargo source document") as opened:
        if opened.stat.st_size > 16 * 1024 * 1024:
            raise ValueError(f"Cargo source input exceeds size policy: {path}")
        raw = opened.stream.read()
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    digest = hashlib.sha256(raw).hexdigest()
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


def _manifest_dependency_tables(data: dict[str, Any]) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    for key in ("dependencies", "build-dependencies"):
        table = data.get(key)
        if isinstance(table, dict):
            tables.append(table)
    target = data.get("target")
    if isinstance(target, dict):
        for target_table in target.values():
            if not isinstance(target_table, dict):
                continue
            for key in ("dependencies", "build-dependencies"):
                table = target_table.get(key)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def _local_path_dependencies(
    *,
    crate_root: Path,
    data: dict[str, Any],
    selected_optional_deps: set[str],
    child_features: dict[str, set[str]],
) -> list[tuple[str, Path, tuple[str, ...]]]:
    deps: list[tuple[str, Path, tuple[str, ...]]] = []
    for table in _manifest_dependency_tables(data):
        for dep_name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            dep_path = spec.get("path")
            if not isinstance(dep_path, str) or not dep_path:
                continue
            optional = bool(spec.get("optional", False))
            if optional and dep_name not in selected_optional_deps:
                continue
            features = set(child_features.get(dep_name, set()))
            spec_features = spec.get("features", [])
            if isinstance(spec_features, list):
                features.update(
                    feature for feature in spec_features if isinstance(feature, str)
                )
            dep_root = (crate_root / dep_path).resolve()
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


def _cargo_crate_source_closure(
    *,
    project_root: Path,
    crate_root: Path,
    crate_features: tuple[str, ...],
    extra_source_paths: Sequence[Path] = (),
) -> list[Path]:
    source_paths: list[Path] = []
    project_root_resolved = project_root.resolve()
    pending: list[tuple[Path, tuple[str, ...]]] = [(crate_root, crate_features)]
    seen: set[tuple[Path, tuple[str, ...]]] = set()
    while pending:
        current_root, current_features = pending.pop()
        key = (current_root, current_features)
        if key in seen:
            continue
        seen.add(key)
        source_paths.extend(_crate_source_paths(current_root))
        data = _read_cargo_document(current_root / "Cargo.toml")
        selected_optional_deps, child_features = _feature_dependency_selection(
            data, current_features
        )
        for _dep_name, dep_root, dep_features in _local_path_dependencies(
            crate_root=current_root,
            data=data,
            selected_optional_deps=selected_optional_deps,
            child_features=child_features,
        ):
            if (
                project_root_resolved in dep_root.parents
                or dep_root == project_root_resolved
            ):
                pending.append((dep_root, dep_features))
    source_paths.extend(extra_source_paths)
    return _dedupe_source_paths(source_paths)


def _cargo_locked_dependency_digest(project_root: Path, crate_root: Path) -> str:
    from molt.cli.compiler_identity import CompilerIdentityError

    try:
        return _project_cargo_locked_dependencies(project_root, crate_root)
    except (OSError, ValueError) as exc:
        raise CompilerIdentityError(f"Compiler dependency identity: {exc}") from exc


def _project_cargo_locked_dependencies(project_root: Path, crate_root: Path) -> str:
    """Content identity of the root package's conservative Cargo.lock closure.

    Cargo remains the dependency resolver. Follow every recorded dependency,
    including optional, development and target edges; never infer active Cargo
    features from the lockfile. Unrelated workspace roots are not compiler inputs.
    """
    import re

    from molt.exact_json import canonical_json_sha256

    manifest = _read_cargo_document(crate_root / "Cargo.toml")
    root_package = manifest.get("package")
    if not isinstance(root_package, dict):
        raise ValueError("Cargo dependency root has no package identity")
    name, version = root_package.get("name"), root_package.get("version")
    if not isinstance(name, str) or not isinstance(version, str):
        raise ValueError("Cargo dependency root requires an explicit name and version")
    root_key = (name, version, "")
    lock = _read_cargo_document(project_root / "Cargo.lock")
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
            key for key in by_name.get(dependency_name, [])
            if (dependency_version is None or key[1] == dependency_version)
            and (dependency_source is None or source_matches(key[2], dependency_source))
        ]
        if dependency_source is None:
            path_candidates = [key for key in candidates if not key[2]]
            if path_candidates:
                candidates = path_candidates
        if len(candidates) != 1:
            raise ValueError(f"Missing or ambiguous Cargo.lock dependency: {reference!r}")
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
        if not re.fullmatch(r"[A-Za-z0-9_-]+", key[0]) or not key[1] or ("source" in package and not key[2]):
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
    return canonical_json_sha256({
        "schema": "molt.cargo-locked-dependency-closure.v2",
        "root": root_key,
        "lock_version": lock["version"],
        "packages": projected,
    })
