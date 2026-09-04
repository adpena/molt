#!/usr/bin/env python3
"""Assemble and verify the portable, source-addressed E1-E4 release bundle."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import stat
import subprocess
import uuid
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from molt.exact_json import ExactJsonError, loads_exact, write_exact
from molt.file_publication import durable_publish_directory_exclusive
from molt.toolchain_identity import stable_file_sha256
from molt.verified_subset import verified_subset_coordinates
from tools import pact_witness_receipt as pwr
from tools import perf_authority as pa
from tools import release_criterion_receipt as rcr
from tools.git_identity import is_git_object_id


ROOT = Path(__file__).resolve().parents[1]
SCHEMA_VERSION = 3
KIND = "molt-release-exit"
STATUS_PASS = "PASS"
STATUS_FAIL = "FAIL"
E2_SCOREBOARD_KIND = "cpython_floor_scoreboard"
E4_KINDS = frozenset(
    {
        rcr.KIND_CANONICALIZATION_CONTRACT,
        rcr.KIND_STRUCTURAL_AUDIT,
        rcr.KIND_DEGRADE_TO_SLOW_GATE,
        rcr.KIND_FAIL_CLOSED_GATE,
    }
)
BASE_EVIDENCE_ROLES = frozenset(
    {
        "e1_native",
        "e1_wasm",
        "e2_scoreboard",
        "e4_canonicalization_contract",
        "e4_degrade_to_slow_gate",
        "e4_fail_closed_gate",
        "e4_structural_audit",
    }
)
_ROOT_KEYS = frozenset(
    {"schema_version", "kind", "source_sha", "status", "registry", "evidence"}
)
_EVIDENCE_KEYS = frozenset({"role", "path", "sha256", "size"})
_REGISTRY_KEYS = frozenset({"target", "variant", "packages"})
_VARIANT_KEYS = frozenset({"cpython", "abi_tier", "target_triple"})
_PACKAGE_KEYS = frozenset({"version", "module_set", "identity_sha256"})
_HEX = frozenset("0123456789abcdef")
_WINDOWS_REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)


def _e3_role(coordinate_id: str) -> str:
    return f"e3_verified_subset.{coordinate_id}"


def _expected_evidence_roles() -> frozenset[str]:
    return frozenset(
        {
            *BASE_EVIDENCE_ROLES,
            *(_e3_role(coordinate.id) for coordinate in verified_subset_coordinates()),
        }
    )


@dataclass(frozen=True, slots=True)
class ReleaseGateReport:
    source_sha: str | None
    status: str | None
    passed: bool
    problems: tuple[str, ...]


def _load_json(path: Path, *, label: str) -> Mapping[str, Any]:
    try:
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(f"{label} is not valid exact JSON: {path}: {exc}") from exc
    if not isinstance(payload, Mapping):
        raise ValueError(f"{label} root must be an object: {path}")
    return payload


def _valid_source_sha(value: object) -> bool:
    return is_git_object_id(value)


def _sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in _HEX for character in value)
    )


def _nonnegative_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def _run_git(repo_root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            ["git", "-C", str(repo_root), *args],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ValueError(f"release source git query failed: {exc}") from exc


def _git_output(repo_root: Path, *args: str) -> str:
    result = _run_git(repo_root, *args)
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"git {' '.join(args)} failed: {detail}")
    return result.stdout.strip()


def _assert_clean_landed_source(repo_root: Path, source_sha: str) -> None:
    if not _valid_source_sha(source_sha):
        raise ValueError("release source_sha must be lowercase 40- or 64-hex")
    root = repo_root.resolve(strict=True)
    head = _git_output(root, "rev-parse", "--verify", "HEAD")
    if head != source_sha:
        raise ValueError(
            f"release assembly source mismatch: requested={source_sha} HEAD={head}"
        )
    dirty = _git_output(
        root,
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignore-submodules=none",
    )
    if dirty:
        entries = dirty.splitlines()
        preview = ", ".join(entries[:5])
        suffix = f" (+{len(entries) - 5} more)" if len(entries) > 5 else ""
        raise ValueError(
            f"release assembly requires a clean source checkout: {preview}{suffix}"
        )
    landed = _run_git(root, "merge-base", "--is-ancestor", source_sha, "origin/main")
    if landed.returncode != 0:
        if landed.returncode == 1:
            raise ValueError(
                f"release source_sha is not landed on origin/main: {source_sha}"
            )
        detail = landed.stderr.strip() or landed.stdout.strip()
        raise ValueError(f"cannot prove release source_sha is landed: {detail}")


def _shared_scientific_registry_coordinates(
    repo_root: Path = ROOT,
) -> dict[tuple[str, str, str], dict[str, Any]]:
    from molt.cli.source_extension_target import source_extension_target_is_wasm
    from molt.scientific_stack_versions import resolve_scientific_stack

    stack = resolve_scientific_stack(
        repo_root.resolve(strict=True) / "config" / "scientific_stack_versions.toml"
    )
    sets = {
        package: stack.extension_set(package, "pact-witness")
        for package in ("numpy", "scipy")
    }
    variants = {
        package: {
            expectation.variant.coordinate: expectation.expected_identity_sha256
            for expectation in extension_set.variants
        }
        for package, extension_set in sets.items()
    }
    coordinates = set(variants["numpy"]) & set(variants["scipy"])
    result = {
        coordinate: {
            "target": (
                "wasm" if source_extension_target_is_wasm(coordinate[2]) else "native"
            ),
            "variant": {
                "cpython": coordinate[0],
                "abi_tier": coordinate[1],
                "target_triple": coordinate[2],
            },
            "packages": {
                package: {
                    "version": extension_set.package_version,
                    "module_set": extension_set.name,
                    "identity_sha256": variants[package][coordinate],
                }
                for package, extension_set in sets.items()
            },
        }
        for coordinate in sorted(coordinates)
    }
    if len(result) != 2 or {item["target"] for item in result.values()} != {
        "native",
        "wasm",
    }:
        raise ValueError(
            "release E1 requires exactly one shared native and one shared WASM "
            f"registry coordinate; got {sorted(result)!r}"
        )
    return result


def _registry_snapshot(
    coordinates: Mapping[tuple[str, str, str], Mapping[str, Any]],
) -> list[dict[str, Any]]:
    return [
        {
            "target": item["target"],
            "variant": dict(item["variant"]),
            "packages": {
                package: dict(item["packages"][package])
                for package in ("numpy", "scipy")
            },
        }
        for _, item in sorted(coordinates.items())
    ]


def _validate_registry_snapshot(
    value: object,
) -> tuple[dict[tuple[str, str, str], dict[str, Any]], list[str]]:
    problems: list[str] = []
    coordinates: dict[tuple[str, str, str], dict[str, Any]] = {}
    if not isinstance(value, list):
        return {}, ["manifest registry must be a list"]
    rendered_coordinates: list[tuple[str, str, str]] = []
    targets: list[str] = []
    for index, item in enumerate(value):
        label = f"manifest registry[{index}]"
        if not isinstance(item, Mapping) or set(item) != _REGISTRY_KEYS:
            problems.append(f"{label} schema is invalid")
            continue
        target = item.get("target")
        if target not in {"native", "wasm"}:
            problems.append(f"{label}.target must be native or wasm")
        else:
            targets.append(target)
        variant = item.get("variant")
        coordinate: tuple[str, str, str] | None = None
        if not isinstance(variant, Mapping) or set(variant) != _VARIANT_KEYS:
            problems.append(f"{label}.variant schema is invalid")
        else:
            raw = tuple(
                variant.get(field) for field in ("cpython", "abi_tier", "target_triple")
            )
            if all(isinstance(part, str) and part for part in raw):
                coordinate = raw  # type: ignore[assignment]
                rendered_coordinates.append(coordinate)
            else:
                problems.append(f"{label}.variant values must be non-empty strings")
        packages = item.get("packages")
        if not isinstance(packages, Mapping) or set(packages) != {"numpy", "scipy"}:
            problems.append(f"{label}.packages must contain exactly numpy and scipy")
        else:
            for package in ("numpy", "scipy"):
                package_item = packages.get(package)
                package_label = f"{label}.packages.{package}"
                if (
                    not isinstance(package_item, Mapping)
                    or set(package_item) != _PACKAGE_KEYS
                ):
                    problems.append(f"{package_label} schema is invalid")
                    continue
                if not all(
                    isinstance(package_item.get(field), str) and package_item.get(field)
                    for field in ("version", "module_set")
                ):
                    problems.append(f"{package_label} version/module_set is invalid")
                if not _sha256(package_item.get("identity_sha256")):
                    problems.append(
                        f"{package_label}.identity_sha256 must be lowercase SHA-256"
                    )
        if coordinate is not None and isinstance(packages, Mapping):
            coordinates[coordinate] = {
                "target": target,
                "variant": dict(variant),
                "packages": {
                    package: dict(packages[package])
                    for package in ("numpy", "scipy")
                    if isinstance(packages.get(package), Mapping)
                },
            }
    if rendered_coordinates != sorted(set(rendered_coordinates)):
        problems.append("manifest registry coordinates must be sorted and unique")
    if len(value) != 2 or set(targets) != {"native", "wasm"}:
        problems.append(
            "manifest registry must contain exactly one native and one WASM coordinate"
        )
    return coordinates, problems


def _resolved_relative_file(
    root: Path,
    raw_path: object,
    *,
    label: str,
) -> tuple[Path | None, list[str]]:
    try:
        relative = pwr.portable_relative_path(raw_path)
    except ValueError:
        return None, [f"{label}.path must be a portable relative POSIX path"]
    try:
        resolved = root.joinpath(*relative.parts).resolve()
    except (OSError, ValueError) as exc:
        return None, [f"{label}.path cannot be resolved: {exc}"]
    if not resolved.is_relative_to(root):
        return None, [f"{label}.path escapes the bundle"]
    return resolved, []


def _validate_evidence_records(
    value: object,
    *,
    bundle_root: Path,
) -> tuple[dict[str, Path], list[str]]:
    problems: list[str] = []
    resolved_by_role: dict[str, Path] = {}
    roles: list[object] = []
    paths: list[str] = []
    if not isinstance(value, list):
        return {}, ["manifest evidence must be a list"]
    for index, item in enumerate(value):
        label = f"manifest evidence[{index}]"
        if not isinstance(item, Mapping) or set(item) != _EVIDENCE_KEYS:
            problems.append(f"{label} schema is invalid")
            continue
        role = item.get("role")
        roles.append(role)
        raw_path = item.get("path")
        if isinstance(raw_path, str):
            try:
                paths.append(pwr.portable_path_identity(raw_path))
            except ValueError:
                pass
        path, path_problems = _resolved_relative_file(
            bundle_root,
            raw_path,
            label=label,
        )
        problems.extend(path_problems)
        if not _sha256(item.get("sha256")):
            problems.append(f"{label}.sha256 must be lowercase SHA-256")
        if not _nonnegative_int(item.get("size")):
            problems.append(f"{label}.size must be a non-negative integer")
        if path is not None:
            if not path.is_file():
                problems.append(f"{label} artifact does not exist: {path}")
            else:
                if _nonnegative_int(item.get("size")):
                    if path.stat().st_size != item["size"]:
                        problems.append(f"{label} artifact size mismatch: {path}")
                if _sha256(item.get("sha256")):
                    if (
                        stable_file_sha256(
                            path,
                            label="release exit witness artifact",
                        )
                        != item["sha256"]
                    ):
                        problems.append(f"{label} artifact checksum mismatch: {path}")
        if isinstance(role, str) and role not in resolved_by_role and path is not None:
            resolved_by_role[role] = path
    string_roles = [role for role in roles if isinstance(role, str)]
    if len(string_roles) != len(set(string_roles)):
        problems.append("manifest evidence roles must not duplicate")
    if len(paths) != len(set(paths)):
        problems.append("manifest evidence paths must not duplicate")
    expected_roles = _expected_evidence_roles()
    if roles != sorted(expected_roles):
        problems.append(
            "manifest evidence roles must be sorted and exact: "
            f"expected={sorted(expected_roles)!r}, got={roles!r}"
        )
    return resolved_by_role, problems


def _typed_receipt_problems(
    path: Path,
    *,
    kind: str,
    source_sha: str,
    repo_root: Path,
    now: dt.datetime | None,
) -> tuple[Mapping[str, Any] | None, list[str]]:
    try:
        payload = _load_json(path, label=f"{kind} receipt")
    except ValueError as exc:
        return None, [str(exc)]
    return payload, list(
        rcr.validate_receipt(
            payload,
            expected_kind=kind,
            expected_source_sha=source_sha,
            repo_root=repo_root,
            verify_inputs=True,
            now=now,
        )
    )


def _derive_status(typed_receipts: Sequence[Mapping[str, Any]]) -> str:
    return (
        STATUS_PASS
        if all(receipt.get("status") == rcr.STATUS_PASS for receipt in typed_receipts)
        else STATUS_FAIL
    )


def _bundle_inventory_problems(
    bundle_root: Path,
    *,
    expected_files: set[Path],
) -> list[str]:
    """Compare a bundle to its manifest without following filesystem links."""

    problems: list[str] = []
    actual_files: set[Path] = set()
    root = bundle_root.absolute()

    def display(path: Path) -> str:
        try:
            return path.relative_to(root).as_posix()
        except ValueError:
            return str(path)

    def is_reparse(metadata: os.stat_result) -> bool:
        return bool(
            _WINDOWS_REPARSE_POINT
            and getattr(metadata, "st_file_attributes", 0) & _WINDOWS_REPARSE_POINT
        )

    try:
        root_metadata = root.lstat()
        root_link_like = (
            root.is_symlink() or root.is_junction() or is_reparse(root_metadata)
        )
        root_available = True
    except OSError as exc:
        problems.append(f"release bundle root cannot be inventoried: {root}: {exc}")
        root_link_like = False
        root_available = False
    if root_link_like:
        problems.append(
            "release bundle root must not be a symbolic link, junction, or reparse "
            f"point: {root}"
        )
    directories = [root] if root_available and not root_link_like else []
    while directories:
        directory = directories.pop()
        try:
            with os.scandir(directory) as stream:
                entries = sorted(stream, key=lambda item: item.name)
        except OSError as exc:
            problems.append(
                f"release bundle directory cannot be inventoried: {display(directory)}: {exc}"
            )
            continue
        for entry in entries:
            path = directory / entry.name
            try:
                metadata = entry.stat(follow_symlinks=False)
                link_like = (
                    entry.is_symlink() or path.is_junction() or is_reparse(metadata)
                )
            except OSError as exc:
                problems.append(
                    f"release bundle entry cannot be inventoried: {display(path)}: {exc}"
                )
                continue
            if link_like:
                problems.append(
                    "release bundle must not contain symbolic links, junctions, or "
                    f"reparse points: {display(path)}"
                )
                continue
            try:
                if entry.is_dir(follow_symlinks=False):
                    directories.append(path)
                    continue
                if not entry.is_file(follow_symlinks=False):
                    problems.append(
                        f"release bundle contains unsupported filesystem entry: {display(path)}"
                    )
                    continue
                resolved = path.resolve(strict=True)
            except (OSError, ValueError) as exc:
                problems.append(
                    f"release bundle entry cannot be resolved: {display(path)}: {exc}"
                )
                continue
            if not resolved.is_relative_to(root):
                problems.append(
                    f"release bundle file resolves outside the bundle: {display(path)} -> {resolved}"
                )
            actual_files.add(resolved)

    normalized_expected: set[Path] = set()
    for expected in expected_files:
        resolved = expected.resolve()
        if not resolved.is_relative_to(root):
            problems.append(f"release bundle bound file escapes the bundle: {resolved}")
        normalized_expected.add(resolved)
    missing = sorted(display(path) for path in normalized_expected - actual_files)
    unknown = sorted(display(path) for path in actual_files - normalized_expected)
    if missing:
        problems.append(f"release bundle is missing bound files: {missing!r}")
    if unknown:
        problems.append(f"release bundle contains unbound files: {unknown!r}")
    return problems


def verify_release_bundle(
    manifest_path: Path,
    *,
    repo_root: Path = ROOT,
    now: dt.datetime | None = None,
) -> ReleaseGateReport:
    """Purely verify bundle bytes plus source inputs bound by typed receipts."""

    manifest = manifest_path.absolute()
    try:
        payload = _load_json(manifest, label="release-exit manifest")
    except ValueError as exc:
        return ReleaseGateReport(None, None, False, (str(exc),))
    problems: list[str] = []
    if set(payload) != _ROOT_KEYS:
        problems.append(
            "release-exit manifest keys differ from schema: "
            f"missing={sorted(_ROOT_KEYS - set(payload), key=str)!r}, "
            f"unknown={sorted(set(payload) - _ROOT_KEYS, key=str)!r}"
        )
    if payload.get("schema_version") != SCHEMA_VERSION:
        problems.append(f"release-exit schema_version must be {SCHEMA_VERSION}")
    if payload.get("kind") != KIND:
        problems.append(f"release-exit kind must be {KIND!r}")
    source_sha = payload.get("source_sha")
    if not _valid_source_sha(source_sha):
        problems.append("release-exit source_sha must be lowercase 40- or 64-hex")
        expected_source_sha = ""
    else:
        expected_source_sha = source_sha
    status = payload.get("status")
    if status not in {STATUS_PASS, STATUS_FAIL}:
        problems.append("release-exit status must be PASS or FAIL")

    bundle_root = manifest.parent
    expected_bundle_files = {manifest.resolve()}
    registry, registry_problems = _validate_registry_snapshot(payload.get("registry"))
    problems.extend(registry_problems)
    try:
        canonical_registry = _shared_scientific_registry_coordinates(repo_root)
    except (OSError, ValueError) as exc:
        problems.append(f"cannot load canonical scientific registry: {exc}")
    else:
        canonical_snapshot = _registry_snapshot(canonical_registry)
        if payload.get("registry") != canonical_snapshot:
            problems.append(
                "release-exit registry differs from the checked-out canonical "
                "scientific registry"
            )
    evidence, evidence_problems = _validate_evidence_records(
        payload.get("evidence"),
        bundle_root=bundle_root,
    )
    problems.extend(evidence_problems)
    expected_bundle_files.update(evidence.values())

    typed_payloads: list[Mapping[str, Any]] = []
    e1_coordinates: set[tuple[str, str, str]] = set()
    for target in ("native", "wasm"):
        path = evidence.get(f"e1_{target}")
        if path is None:
            continue
        try:
            receipt = _load_json(path, label=f"E1 {target} receipt")
        except ValueError as exc:
            problems.append(str(exc))
            continue
        coordinate = pwr.acceptance_coordinate(receipt)
        expected = registry.get(coordinate) if coordinate is not None else None
        if expected is None:
            problems.append(
                f"E1 {target} receipt has no exact manifest registry coordinate"
            )
        else:
            e1_coordinates.add(coordinate)
            if expected.get("target") != target:
                problems.append(f"E1 {target} receipt target/registry mismatch")
            problems.extend(
                f"E1 {target}: {problem}"
                for problem in pwr.validate_acceptance_receipt(
                    receipt,
                    receipt_path=path,
                    expected=expected,
                    require_artifacts=True,
                )
            )
        git = receipt.get("git")
        receipt_sha = git.get("source_sha") if isinstance(git, Mapping) else None
        if receipt_sha != expected_source_sha:
            problems.append(f"E1 {target} source_sha differs from release source")
        try:
            expected_bundle_files.update(_e1_closure(path, receipt).values())
        except (OSError, ValueError) as exc:
            problems.append(f"E1 {target} artifact closure is invalid: {exc}")
    if e1_coordinates != set(registry):
        problems.append(
            "E1 receipts do not cover the exact manifest registry coordinates"
        )

    scoreboard_path = evidence.get("e2_scoreboard")
    if scoreboard_path is not None and expected_source_sha:
        try:
            scoreboard = _load_json(scoreboard_path, label="E2 scoreboard")
        except ValueError as exc:
            problems.append(str(exc))
        else:
            problems.extend(
                f"E2: {problem}"
                for problem in pa.release_scoreboard_problems(
                    scoreboard,
                    expected_source_sha=expected_source_sha,
                    now=now,
                )
            )

    expected_e3_coordinates = {
        coordinate.id: coordinate for coordinate in verified_subset_coordinates()
    }
    seen_e3_coordinates: set[str] = set()
    for coordinate_id in sorted(expected_e3_coordinates):
        role = _e3_role(coordinate_id)
        path = evidence.get(role)
        if path is None or not expected_source_sha:
            continue
        receipt, receipt_problems = _typed_receipt_problems(
            path,
            kind=rcr.KIND_VERIFIED_SUBSET,
            source_sha=expected_source_sha,
            repo_root=repo_root,
            now=now,
        )
        problems.extend(f"{role}: {problem}" for problem in receipt_problems)
        if receipt is None:
            continue
        facts = receipt.get("facts")
        coordinate = facts.get("coordinate") if isinstance(facts, Mapping) else None
        receipt_coordinate_id = (
            coordinate.get("id") if isinstance(coordinate, Mapping) else None
        )
        if receipt_coordinate_id != coordinate_id:
            problems.append(f"{role}: receipt coordinate does not match evidence role")
            continue
        seen_e3_coordinates.add(coordinate_id)
        typed_payloads.append(receipt)
    if seen_e3_coordinates != set(expected_e3_coordinates):
        problems.append("E3 receipts do not cover the exact verified-subset matrix")

    typed_roles = {
        "e4_canonicalization_contract": rcr.KIND_CANONICALIZATION_CONTRACT,
        "e4_degrade_to_slow_gate": rcr.KIND_DEGRADE_TO_SLOW_GATE,
        "e4_fail_closed_gate": rcr.KIND_FAIL_CLOSED_GATE,
        "e4_structural_audit": rcr.KIND_STRUCTURAL_AUDIT,
    }
    for role, kind in typed_roles.items():
        path = evidence.get(role)
        if path is None or not expected_source_sha:
            continue
        receipt, receipt_problems = _typed_receipt_problems(
            path,
            kind=kind,
            source_sha=expected_source_sha,
            repo_root=repo_root,
            now=now,
        )
        problems.extend(f"{role}: {problem}" for problem in receipt_problems)
        if receipt is not None:
            typed_payloads.append(receipt)

    derived_status = _derive_status(typed_payloads)
    expected_typed_receipt_count = len(expected_e3_coordinates) + len(typed_roles)
    if len(typed_payloads) != expected_typed_receipt_count:
        derived_status = STATUS_FAIL
    if status != derived_status:
        problems.append(
            "release-exit status is not derived from typed criterion receipts: "
            f"expected={derived_status}, got={status!r}"
        )
    problems.extend(
        _bundle_inventory_problems(
            bundle_root,
            expected_files={path.resolve() for path in expected_bundle_files},
        )
    )
    passed = not problems and status == STATUS_PASS
    return ReleaseGateReport(
        expected_source_sha or None,
        status if isinstance(status, str) else None,
        passed,
        tuple(problems),
    )


def _copy_exact(source: Path, destination: Path) -> Path:
    resolved = source.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError(f"release evidence is not a file: {resolved}")
    if destination.exists():
        raise ValueError(f"release bundle destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(resolved, destination)
    return destination.resolve(strict=True)


def _remove_staging_directory(stage: Path, *, canonical_root: Path) -> None:
    resolved_root = canonical_root.resolve(strict=True)
    resolved_stage = stage.resolve(strict=True)
    if (
        resolved_stage.parent != resolved_root
        or not resolved_stage.name.startswith(".")
        or not resolved_stage.name.endswith(".tmp")
    ):
        raise RuntimeError(
            f"refusing to remove non-staging release path: {resolved_stage}"
        )
    shutil.rmtree(resolved_stage)


def _source_relative_file(root: Path, raw_path: object, *, label: str) -> Path:
    try:
        relative = pwr.portable_relative_path(raw_path)
    except ValueError as exc:
        raise ValueError(f"{label} is not a portable relative POSIX path") from exc
    resolved_root = root.resolve()
    resolved = resolved_root.joinpath(*relative.parts).resolve(strict=True)
    if not resolved.is_relative_to(resolved_root) or not resolved.is_file():
        raise ValueError(f"{label} escapes its receipt directory")
    return resolved


def _e1_closure(
    receipt_path: Path,
    payload: Mapping[str, Any],
) -> dict[PurePosixPath, Path]:
    source_root = receipt_path.resolve(strict=True).parent
    closure: dict[PurePosixPath, Path] = {}
    identities: dict[str, tuple[PurePosixPath, Path]] = {}

    def bind(
        relative: PurePosixPath,
        source: Path,
        *,
        label: str,
        allow_exact_alias: bool = False,
    ) -> None:
        identity = pwr.portable_path_identity(relative.as_posix())
        prior = identities.get(identity)
        if prior is not None:
            prior_relative, prior_source = prior
            if (
                allow_exact_alias
                and prior_relative == relative
                and prior_source == source
            ):
                return
            raise ValueError(
                f"{label} collides under the portable filesystem identity: "
                f"{relative.as_posix()!r} versus {prior_relative.as_posix()!r}"
            )
        identities[identity] = (relative, source)
        closure[relative] = source

    bind(
        PurePosixPath("acceptance-receipt.json"),
        receipt_path.resolve(strict=True),
        label="E1 acceptance receipt",
    )
    records: list[Mapping[str, Any]] = []
    artifacts = payload.get("artifacts")
    if isinstance(artifacts, list):
        records.extend(item for item in artifacts if isinstance(item, Mapping))
    parity_gate = payload.get("parity_gate")
    if isinstance(parity_gate, Mapping):
        records.append(parity_gate)
    execution_manifest: tuple[PurePosixPath, Path] | None = None
    for record in records:
        relative = pwr.portable_relative_path(record.get("path"))
        source = _source_relative_file(
            source_root,
            relative.as_posix(),
            label="E1 artifact path",
        )
        bind(relative, source, label="E1 artifact closure path")
        if record.get("role") == "execution_manifest":
            execution_manifest = (relative, source)
    if execution_manifest is not None:
        manifest_relative, manifest_source = execution_manifest
        manifest_payload = _load_json(
            manifest_source,
            label="E1 WASM execution manifest",
        )
        modules = manifest_payload.get("modules")
        if not isinstance(modules, Mapping):
            raise ValueError("E1 WASM execution manifest modules must be an object")
        for label, descriptor in sorted(modules.items()):
            if not isinstance(descriptor, Mapping):
                raise ValueError(
                    f"E1 WASM execution manifest modules.{label} must be an object"
                )
            module_relative = pwr.portable_relative_path(descriptor.get("path"))
            source = _source_relative_file(
                manifest_source.parent,
                module_relative.as_posix(),
                label=f"E1 WASM execution manifest modules.{label}.path",
            )
            destination_relative = manifest_relative.parent / module_relative
            bind(
                destination_relative,
                source,
                label="E1 WASM module closure path",
                allow_exact_alias=True,
            )
    return closure


def _copy_e1_closure(
    receipt_path: Path,
    payload: Mapping[str, Any],
    *,
    destination_root: Path,
) -> Path:
    for relative, source in sorted(
        _e1_closure(receipt_path, payload).items(),
        key=lambda item: item[0].as_posix(),
    ):
        _copy_exact(source, destination_root.joinpath(*relative.parts))
    return destination_root / "acceptance-receipt.json"


def _evidence_record(role: str, path: Path, *, manifest_path: Path) -> dict[str, Any]:
    return pwr.artifact_receipt(role, path, receipt_path=manifest_path)


def assemble_release_bundle(
    *,
    source_sha: str,
    e1_receipts: Sequence[Path],
    e2_scoreboard: Path,
    e3_receipts: Sequence[Path],
    e4_receipts: Sequence[Path],
    repo_root: Path = ROOT,
    output_root: Path | None = None,
    now: dt.datetime | None = None,
) -> tuple[Path, ReleaseGateReport]:
    """Validate exact inputs and atomically assemble one canonical bundle."""

    root = repo_root.resolve(strict=True)
    _assert_clean_landed_source(root, source_sha)
    if len(e1_receipts) != 2:
        raise ValueError("release assembly requires exactly two E1 receipts")
    if len(e4_receipts) != 4:
        raise ValueError("release assembly requires exactly four E4 receipts")
    expected_e3_coordinates = {
        coordinate.id: coordinate for coordinate in verified_subset_coordinates()
    }
    if len(e3_receipts) != len(expected_e3_coordinates):
        raise ValueError(
            "release assembly requires the exact verified-subset E3 receipt count: "
            f"expected={len(expected_e3_coordinates)}, got={len(e3_receipts)}"
        )
    registry = _shared_scientific_registry_coordinates(root)

    e1_by_target: dict[str, tuple[Path, Mapping[str, Any]]] = {}
    seen_coordinates: set[tuple[str, str, str]] = set()
    for path in e1_receipts:
        resolved = path.resolve(strict=True)
        payload = _load_json(resolved, label="E1 acceptance receipt")
        coordinate = pwr.acceptance_coordinate(payload)
        expected = registry.get(coordinate) if coordinate is not None else None
        if expected is None:
            raise ValueError(
                f"E1 receipt has no shared registry coordinate: {resolved}"
            )
        problems = pwr.validate_acceptance_receipt(
            payload,
            receipt_path=resolved,
            expected=expected,
            require_artifacts=True,
        )
        if problems:
            raise ValueError("invalid E1 receipt: " + "; ".join(problems))
        git = payload.get("git")
        receipt_sha = git.get("source_sha") if isinstance(git, Mapping) else None
        if receipt_sha != source_sha:
            raise ValueError("E1 receipt source_sha differs from release source")
        target = payload.get("target")
        if not isinstance(target, str) or target in e1_by_target:
            raise ValueError("E1 receipts must contain unique native and WASM targets")
        e1_by_target[target] = (resolved, payload)
        assert coordinate is not None
        seen_coordinates.add(coordinate)
    if set(e1_by_target) != {"native", "wasm"} or seen_coordinates != set(registry):
        raise ValueError(
            "E1 receipts must cover exact shared native and WASM coordinates"
        )

    scoreboard_path = e2_scoreboard.resolve(strict=True)
    scoreboard = _load_json(scoreboard_path, label="E2 scoreboard")
    scoreboard_problems = pa.release_scoreboard_problems(
        scoreboard,
        expected_source_sha=source_sha,
        now=now,
    )
    if scoreboard_problems:
        raise ValueError("invalid E2 scoreboard: " + "; ".join(scoreboard_problems))

    e3_by_coordinate: dict[str, tuple[Path, Mapping[str, Any]]] = {}
    for path in e3_receipts:
        resolved = path.resolve(strict=True)
        payload, receipt_problems = _typed_receipt_problems(
            resolved,
            kind=rcr.KIND_VERIFIED_SUBSET,
            source_sha=source_sha,
            repo_root=root,
            now=now,
        )
        if payload is None or receipt_problems:
            raise ValueError("invalid E3 receipt: " + "; ".join(receipt_problems))
        facts = payload.get("facts")
        coordinate = facts.get("coordinate") if isinstance(facts, Mapping) else None
        coordinate_id = (
            coordinate.get("id") if isinstance(coordinate, Mapping) else None
        )
        if (
            not isinstance(coordinate_id, str)
            or coordinate_id not in expected_e3_coordinates
            or coordinate_id in e3_by_coordinate
        ):
            raise ValueError(
                "E3 receipts must contain each exact verified-subset coordinate once"
            )
        e3_by_coordinate[coordinate_id] = (resolved, payload)
    if set(e3_by_coordinate) != set(expected_e3_coordinates):
        raise ValueError("release assembly is missing an exact E3 matrix coordinate")

    typed_by_kind: dict[str, tuple[Path, Mapping[str, Any]]] = {}
    for path in e4_receipts:
        resolved = path.resolve(strict=True)
        payload = _load_json(resolved, label="E4 receipt")
        kind = payload.get("kind")
        if not isinstance(kind, str) or kind not in E4_KINDS or kind in typed_by_kind:
            raise ValueError("E4 receipts must contain each exact structural kind once")
        receipt_problems = rcr.validate_receipt(
            payload,
            expected_kind=kind,
            expected_source_sha=source_sha,
            repo_root=root,
            verify_inputs=True,
            now=now,
        )
        if receipt_problems:
            raise ValueError("invalid E4 receipt: " + "; ".join(receipt_problems))
        typed_by_kind[kind] = (resolved, payload)
    if set(typed_by_kind) != E4_KINDS:
        raise ValueError("release assembly is missing an exact E4 receipt kind")

    canonical_root = (
        root / "dist" / "release-exit" if output_root is None else output_root.resolve()
    )
    destination = canonical_root / source_sha
    manifest_path = destination / "release-exit.json"
    if destination.exists():
        raise ValueError(f"release bundle already exists: {destination}")
    canonical_root.mkdir(parents=True, exist_ok=True)
    stage = canonical_root / f".{source_sha}.{uuid.uuid4().hex}.tmp"
    stage.mkdir()
    stage_manifest = stage / "release-exit.json"
    try:
        evidence: list[dict[str, Any]] = []
        for target in ("native", "wasm"):
            source_path, payload = e1_by_target[target]
            copied = _copy_e1_closure(
                source_path,
                payload,
                destination_root=stage / "evidence" / "e1" / target,
            )
            evidence.append(
                _evidence_record(f"e1_{target}", copied, manifest_path=stage_manifest)
            )
        copied_scoreboard = _copy_exact(
            scoreboard_path,
            stage / "evidence" / "e2" / "scoreboard.json",
        )
        evidence.append(
            _evidence_record(
                "e2_scoreboard",
                copied_scoreboard,
                manifest_path=stage_manifest,
            )
        )
        for coordinate_id in sorted(e3_by_coordinate):
            source_path, _ = e3_by_coordinate[coordinate_id]
            copied = _copy_exact(
                source_path,
                stage / "evidence" / "e3" / f"{coordinate_id}.json",
            )
            evidence.append(
                _evidence_record(
                    _e3_role(coordinate_id), copied, manifest_path=stage_manifest
                )
            )
        typed_roles = {
            rcr.KIND_CANONICALIZATION_CONTRACT: "e4_canonicalization_contract",
            rcr.KIND_DEGRADE_TO_SLOW_GATE: "e4_degrade_to_slow_gate",
            rcr.KIND_FAIL_CLOSED_GATE: "e4_fail_closed_gate",
            rcr.KIND_STRUCTURAL_AUDIT: "e4_structural_audit",
        }
        for kind, role in sorted(typed_roles.items(), key=lambda item: item[1]):
            source_path, _ = typed_by_kind[kind]
            copied = _copy_exact(
                source_path,
                stage / "evidence" / "e4" / f"{kind}.json",
            )
            evidence.append(
                _evidence_record(role, copied, manifest_path=stage_manifest)
            )
        evidence.sort(key=lambda item: item["role"])
        typed_payloads = [
            *(payload for _, payload in e3_by_coordinate.values()),
            *(payload for _, payload in typed_by_kind.values()),
        ]
        manifest_payload = {
            "schema_version": SCHEMA_VERSION,
            "kind": KIND,
            "source_sha": source_sha,
            "status": _derive_status(typed_payloads),
            "registry": _registry_snapshot(registry),
            "evidence": evidence,
        }
        write_exact(stage_manifest, manifest_payload, exclusive=True)
        stage_report = verify_release_bundle(
            stage_manifest,
            repo_root=root,
            now=now,
        )
        if stage_report.problems:
            raise ValueError(
                "assembled release bundle failed self-verification: "
                + "; ".join(stage_report.problems)
            )
        _assert_clean_landed_source(root, source_sha)
        durable_publish_directory_exclusive(stage, destination)
    except BaseException:
        if stage.exists():
            _remove_staging_directory(stage, canonical_root=canonical_root)
        raise
    report = verify_release_bundle(manifest_path, repo_root=root, now=now)
    return manifest_path, report


def _print_report(report: ReleaseGateReport) -> None:
    if report.problems:
        for problem in report.problems:
            print(f"[release-exit] {problem}")
    if report.passed:
        print(
            f"[release-exit] PASS source_sha={report.source_sha}",
            flush=True,
        )
    elif not report.problems:
        print(
            f"[release-exit] FAIL source_sha={report.source_sha}",
            flush=True,
        )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    assemble = subparsers.add_parser(
        "assemble",
        help="assemble the canonical source-addressed release bundle",
    )
    assemble.add_argument("--source-sha", required=True)
    assemble.add_argument("--e1-receipt", type=Path, action="append", required=True)
    assemble.add_argument("--e2-scoreboard", type=Path, required=True)
    assemble.add_argument("--e3-receipt", type=Path, action="append", required=True)
    assemble.add_argument("--e4-receipt", type=Path, action="append", required=True)

    verify = subparsers.add_parser(
        "verify",
        help="purely verify a portable release bundle",
    )
    verify.add_argument("manifest", type=Path)

    args = parser.parse_args(argv)
    try:
        if args.command == "assemble":
            manifest, report = assemble_release_bundle(
                source_sha=args.source_sha,
                e1_receipts=args.e1_receipt,
                e2_scoreboard=args.e2_scoreboard,
                e3_receipts=args.e3_receipt,
                e4_receipts=args.e4_receipt,
            )
            print(f"release_exit_manifest={manifest}")
        else:
            report = verify_release_bundle(args.manifest)
    except (OSError, ValueError) as exc:
        print(f"[release-exit] {exc}")
        return 1
    _print_report(report)
    return 0 if report.passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
