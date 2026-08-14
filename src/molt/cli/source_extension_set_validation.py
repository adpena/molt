"""Canonical validation of a registered source-extension package seal."""

from __future__ import annotations

import hashlib
import json
import platform
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, cast

from packaging.requirements import Requirement
from packaging.utils import canonicalize_name
from packaging.version import InvalidVersion, Version

from molt.cli.extension_manifest import _host_target_triple
from molt.cli.source_build_environment import source_build_environment_problems
from molt.cli.source_extension_manifest_codec import (
    _manifest_sequence,
    _object_unit_sha256,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_object_closure import (
    source_extension_object_closure_digest,
)
from molt.cli.source_extension_reproducibility import _require_location_neutral
from molt.cli.source_extension_set_identity import (
    SOURCE_EXTENSION_SET_SCHEMA_VERSION,
    _require_expected_source_extension_set_identity,
)
from molt.cli.source_extension_set_registry import (
    SourceExtensionRegistry,
    SourceExtensionSet,
    SourceExtensionVariant,
    load_source_extension_registry,
    require_registered_source_extension_set,
    source_extension_set_expected_identity,
)
from molt.cli.source_extension_target import (
    resolve_source_extension_target_plan,
    source_extension_artifact_suffix,
    source_extension_target_is_wasm,
)
from molt.cli.source_package_seal import (
    SourcePackageSeal,
    validate_source_package_relative_path,
    verify_source_package_seal,
)
from molt.cli.source_extension_toolchain import MOLT_PKGCONF_REQUIREMENT
from molt.file_hashing import _sha256_file


class SourceExtensionSetValidationError(ValueError):
    """A published package set violates its registered structural contract."""


def _source_extension_tool_role_contract(
    target_triple: str,
) -> tuple[dict[str, str], frozenset[str]]:
    identity_role_by_command = {
        "ar": "ar",
        "c": "cc",
        "cpp": "cxx",
        "ld": "wasm_ld",
        "nm": "nm",
        "ranlib": "ranlib",
        "strip": "strip",
    }
    required_commands = {"ar", "c", "nm"}
    if source_extension_target_is_wasm(target_triple):
        required_commands.add("ld")
    return identity_role_by_command, frozenset(required_commands)


def validate_source_extension_set_publish_root(
    *,
    publish_root: Path,
    extension_set: SourceExtensionSet,
    variant: SourceExtensionVariant,
    set_manifest: Mapping[str, Any],
) -> None:
    expected_manifest_keys = {
        "schema_version",
        "kind",
        "package",
        "package_version",
        "name",
        "seal_name",
        "cpython",
        "source_head",
        "submodules",
        "target_triple",
        "abi_tier",
        "build_environment",
        "meson",
        "target_metadata",
        "installed_package_files",
        "extensions",
    }
    if set(set_manifest) != expected_manifest_keys:
        raise SourceExtensionSetValidationError(
            "extension-set manifest keys differ from schema: "
            f"missing={sorted(expected_manifest_keys - set(set_manifest))!r}, "
            f"unknown={sorted(set(set_manifest) - expected_manifest_keys)!r}"
        )
    expected_set_contract = {
        "schema_version": SOURCE_EXTENSION_SET_SCHEMA_VERSION,
        "kind": "molt-source-extension-set",
        "package": extension_set.package,
        "package_version": extension_set.package_version,
        "name": extension_set.name,
        "seal_name": extension_set.seal_name,
        "cpython": variant.cpython,
        "source_head": extension_set.source.commit,
        "target_triple": variant.target_triple,
        "abi_tier": variant.abi_tier,
    }
    mismatches = [
        f"{field}: expected {expected!r}, got {set_manifest.get(field)!r}"
        for field, expected in expected_set_contract.items()
        if set_manifest.get(field) != expected
    ]
    if mismatches:
        raise SourceExtensionSetValidationError(
            "extension-set manifest differs from registered package-set authority: "
            + "; ".join(mismatches)
        )
    if not isinstance(set_manifest.get("submodules"), list):
        raise SourceExtensionSetValidationError(
            "extension-set manifest submodules must be a list"
        )
    for field in ("build_environment", "meson"):
        if not isinstance(set_manifest.get(field), Mapping):
            raise SourceExtensionSetValidationError(
                f"extension-set manifest {field} must be an object"
            )
    build_environment = set_manifest["build_environment"]
    build_problems = source_build_environment_problems(build_environment)
    if build_problems:
        raise SourceExtensionSetValidationError("; ".join(build_problems))
    try:
        _require_location_neutral(
            set_manifest,
            authority="source-extension set manifest",
        )
    except ValueError as exc:
        raise SourceExtensionSetValidationError(str(exc)) from exc

    meson = cast(Mapping[str, Any], set_manifest["meson"])
    expected_meson_keys = {
        "driver",
        "backend",
        "build_root",
        "setup_args",
        "intro_targets_sha256",
        "compile_commands_sha256",
        "intro_installed_sha256",
        "config_tool_cross_sha256",
        "config_tools",
        "pkg_config_requirement",
        "generated_inputs",
    }
    if set(meson) != expected_meson_keys:
        raise SourceExtensionSetValidationError(
            "extension-set Meson metadata keys differ from schema"
        )
    if (
        meson.get("build_root") != "@build"
        or meson.get("setup_args") != list(extension_set.meson_setup_args)
        or not isinstance(meson.get("driver"), Mapping)
        or not isinstance(meson.get("backend"), Mapping)
        or not isinstance(meson.get("config_tools"), list)
        or not isinstance(meson.get("generated_inputs"), list)
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson metadata differs from registered build contract"
        )
    resolved_build_requirements = {
        canonicalize_name(str(item["distribution"])): str(item["version"])
        for item in cast(Mapping[str, Any], build_environment)["resolved"]
        if isinstance(item, Mapping)
    }
    backend = cast(Mapping[str, Any], meson["backend"])
    custody = cast(Mapping[str, Any], build_environment)["custody"]
    group_requirements = cast(Mapping[str, Any], custody)[
        "dependency_group_requirements"
    ]
    ninja_requirements = tuple(
        requirement
        for requirement in (Requirement(str(raw)) for raw in group_requirements)
        if canonicalize_name(requirement.name) == "ninja"
    )
    backend_version = backend.get("version")
    try:
        parsed_backend_version = Version(str(backend_version))
    except InvalidVersion:
        parsed_backend_version = None
    backend_matches_custody = (
        len(ninja_requirements) == 1
        and parsed_backend_version is not None
        and ninja_requirements[0].specifier.contains(
            parsed_backend_version, prereleases=True
        )
    )
    if (
        set(backend) != {"distribution", "version", "path", "sha256"}
        or canonicalize_name(str(backend.get("distribution"))) != "ninja"
        or not backend_matches_custody
        or not isinstance(backend.get("path"), str)
        or not backend.get("path")
        or any(separator in backend["path"] for separator in ("/", "\\"))
        or not isinstance(backend.get("sha256"), str)
        or len(backend["sha256"]) != 64
        or any(character not in "0123456789abcdef" for character in backend["sha256"])
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson backend identity is invalid"
        )
    driver = cast(Mapping[str, Any], meson["driver"])
    driver_kind = driver.get("kind")
    if driver_kind == "build-environment":
        if (
            set(driver) != {"kind", "module", "distribution", "version"}
            or driver.get("module") != "mesonbuild.mesonmain"
            or canonicalize_name(str(driver.get("distribution"))) != "meson"
            or driver.get("version") != resolved_build_requirements.get("meson")
        ):
            raise SourceExtensionSetValidationError(
                "extension-set Meson driver identity is invalid"
            )
    elif driver_kind == "source-vendored":
        driver_path = driver.get("path")
        driver_sha256 = driver.get("sha256")
        if (
            set(driver) != {"kind", "path", "sha256"}
            or not isinstance(driver_path, str)
            or not driver_path
            or Path(driver_path).is_absolute()
            or ".." in Path(driver_path).parts
            or not isinstance(driver_sha256, str)
            or len(driver_sha256) != 64
            or any(character not in "0123456789abcdef" for character in driver_sha256)
        ):
            raise SourceExtensionSetValidationError(
                "extension-set Meson driver identity is invalid"
            )
    else:
        raise SourceExtensionSetValidationError(
            "extension-set Meson driver identity is invalid"
        )

    config_tools = cast(list[Any], meson["config_tools"])
    config_tool_names = tuple(
        item.get("name") if isinstance(item, Mapping) else None for item in config_tools
    )
    if config_tool_names != extension_set.required_config_tools:
        raise SourceExtensionSetValidationError(
            "extension-set Meson config tool set/order differs from registered "
            f"authority: expected {extension_set.required_config_tools!r}, "
            f"got {config_tool_names!r}"
        )
    pkgconf_requirement = Requirement(MOLT_PKGCONF_REQUIREMENT)
    for index, item in enumerate(config_tools):
        if not isinstance(item, Mapping) or set(item) != {
            "name",
            "path",
            "distribution",
            "version",
            "sha256",
        }:
            raise SourceExtensionSetValidationError(
                f"extension-set Meson config_tools[{index}] is invalid"
            )
        path = item.get("path")
        distribution = item.get("distribution")
        version = item.get("version")
        sha256 = item.get("sha256")
        if (
            not isinstance(path, str)
            or not path
            or any(separator in path for separator in ("/", "\\"))
            or not isinstance(distribution, str)
            or not distribution
            or not isinstance(version, str)
            or not version
            or not isinstance(sha256, str)
            or len(sha256) != 64
            or any(character not in "0123456789abcdef" for character in sha256)
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set Meson config_tools[{index}] is invalid"
            )
        normalized_distribution = canonicalize_name(distribution)
        if item["name"] == "pkg-config":
            try:
                pkgconf_version = Version(version)
            except InvalidVersion as exc:
                raise SourceExtensionSetValidationError(
                    "extension-set Meson pkg-config version is invalid"
                ) from exc
            if normalized_distribution != canonicalize_name(
                pkgconf_requirement.name
            ) or not pkgconf_requirement.specifier.contains(
                pkgconf_version, prereleases=True
            ):
                raise SourceExtensionSetValidationError(
                    "extension-set Meson pkg-config identity differs from custody"
                )
        elif resolved_build_requirements.get(normalized_distribution) != version:
            raise SourceExtensionSetValidationError(
                f"extension-set Meson config_tools[{index}] differs from resolved "
                "build custody"
            )
    expected_pkg_config_requirement = (
        MOLT_PKGCONF_REQUIREMENT if extension_set.use_pkg_config else None
    )
    if meson.get("pkg_config_requirement") != expected_pkg_config_requirement:
        raise SourceExtensionSetValidationError(
            "extension-set Meson pkg-config requirement differs from registered custody"
        )
    meson_files = {
        "intro_targets_sha256": "intro-targets.json",
        "compile_commands_sha256": "compile-commands.json",
        "intro_installed_sha256": "intro-installed.json",
    }
    meson_root = publish_root / "provenance" / "metadata" / "meson"
    for digest_name, filename in meson_files.items():
        metadata_file = meson_root / filename
        if not metadata_file.is_file() or meson.get(digest_name) != _sha256_file(
            metadata_file
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set Meson {digest_name} is false"
            )
    config_tool_cross = meson_root / "build-config-tools.cross"
    if extension_set.use_pkg_config:
        if (
            not isinstance(meson.get("pkg_config_requirement"), str)
            or not meson.get("pkg_config_requirement")
            or not config_tool_cross.is_file()
            or meson.get("config_tool_cross_sha256") != _sha256_file(config_tool_cross)
        ):
            raise SourceExtensionSetValidationError(
                "extension-set Meson config-tool custody is incomplete"
            )
    elif (
        meson.get("pkg_config_requirement") is not None
        or meson.get("config_tool_cross_sha256") is not None
        or meson.get("config_tools") != []
        or config_tool_cross.exists()
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson config-tool custody is unexpected"
        )

    target_metadata = set_manifest.get("target_metadata")
    if not isinstance(target_metadata, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set manifest target_metadata is missing"
        )
    expected_target_metadata_keys = {
        "schema_version",
        "kind",
        "target_triple",
        "target",
        "python",
        "abi",
        "toolchain",
        "meson_cross_properties",
        "paths",
        "env",
        "digests",
        "digest",
    }
    if set(target_metadata) != expected_target_metadata_keys:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata keys differ from schema"
        )
    metadata_contract = (
        target_metadata.get("schema_version"),
        target_metadata.get("kind"),
        target_metadata.get("target_triple"),
    )
    expected_metadata_contract = (
        3,
        "molt-source-extension-target-metadata",
        variant.target_triple,
    )
    if metadata_contract != expected_metadata_contract:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata contract differs from selected variant: "
            f"expected {expected_metadata_contract!r}, got {metadata_contract!r}"
        )
    expected_python = {
        "implementation": "cpython",
        "version": variant.cpython,
    }
    if target_metadata.get("python") != expected_python:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata Python authority differs from selected "
            f"variant: expected {expected_python!r}, got "
            f"{target_metadata.get('python')!r}"
        )
    target_facts = target_metadata.get("target")
    if not isinstance(target_facts, Mapping) or set(target_facts) != {
        "requested",
        "compiler_target_triple",
        "artifact_kind",
    }:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has an invalid target fact set"
        )
    requested_target = target_facts.get("requested")
    if not isinstance(requested_target, str) or not requested_target:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has no requested-target authority"
        )
    try:
        target_plan = resolve_source_extension_target_plan(
            requested_target,
            host_target_triple=_host_target_triple(),
            host_platform=sys.platform,
            host_arch=platform.machine(),
        )
    except ValueError as exc:
        raise SourceExtensionSetValidationError(
            f"extension-set target metadata requested target is invalid: {exc}"
        ) from exc
    expected_target_facts = {
        "requested": target_plan.requested,
        "compiler_target_triple": target_plan.compiler_target_triple,
        "artifact_kind": target_plan.artifact_kind,
    }
    if (
        target_plan.target_triple != variant.target_triple
        or dict(target_facts) != expected_target_facts
    ):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata facts differ from the canonical target "
            f"plan: expected target {variant.target_triple!r} and facts "
            f"{expected_target_facts!r}, got target {target_plan.target_triple!r} "
            f"and facts {dict(target_facts)!r}"
        )
    target_identity = dict(target_metadata)
    target_digest = target_identity.pop("digest", None)
    computed_target_digest = hashlib.sha256(
        json.dumps(
            target_identity,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    if target_digest != computed_target_digest:
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata identity checksum is false"
        )
    target_sidecar_path = (
        publish_root
        / "provenance"
        / "metadata"
        / "target"
        / "source-extension-target-metadata.json"
    )
    try:
        target_sidecar = json.loads(target_sidecar_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionSetValidationError(
            f"cannot read canonical target metadata sidecar: {exc}"
        ) from exc
    if target_sidecar != target_metadata:
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata differs from its canonical sidecar"
        )
    target_digests = target_metadata.get("digests")
    if not isinstance(target_digests, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set target_metadata digests are missing"
        )
    target_files = {
        "python_pc_sha256": (
            publish_root / "provenance/metadata/target/pkgconfig/python3.pc"
        ),
        "meson_cross_sha256": (publish_root / "provenance/metadata/target/meson.cross"),
    }
    for digest_name, target_file in target_files.items():
        if not target_file.is_file() or target_digests.get(digest_name) != _sha256_file(
            target_file
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set target_metadata {digest_name} is false"
            )
    toolchain = target_metadata.get("toolchain")
    tools = toolchain.get("tools") if isinstance(toolchain, Mapping) else None
    target_commands = (
        toolchain.get("commands") if isinstance(toolchain, Mapping) else None
    )
    tool_roles, required_command_roles = _source_extension_tool_role_contract(
        variant.target_triple
    )
    if not isinstance(tools, Mapping) or set(tools) != set(tool_roles.values()):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has an incomplete tool identity family"
        )
    if not isinstance(target_commands, Mapping):
        raise SourceExtensionSetValidationError(
            "extension-set target metadata has no command family"
        )
    unknown_command_roles = set(target_commands) - set(tool_roles)
    missing_command_roles = required_command_roles - set(target_commands)
    if unknown_command_roles or missing_command_roles:
        raise SourceExtensionSetValidationError(
            "extension-set target metadata command family differs from target "
            f"contract: missing={sorted(missing_command_roles)!r}, "
            f"unknown={sorted(unknown_command_roles)!r}"
        )
    for command_role, command in target_commands.items():
        tool_role = tool_roles[command_role]
        identity = tools.get(tool_role)
        if not (
            isinstance(identity, Mapping)
            and isinstance(identity.get("path"), str)
            and isinstance(identity.get("sha256"), str)
            and len(identity["sha256"]) == 64
            and all(character in "0123456789abcdef" for character in identity["sha256"])
            and isinstance(identity.get("command"), list)
            and identity.get("command")
            and isinstance(command, list)
            and command
            and command[0] == identity["command"][0]
        ):
            raise SourceExtensionSetValidationError(
                f"extension-set target metadata {command_role} identity is invalid"
            )

    installed_files = set_manifest.get("installed_package_files")
    if (
        not isinstance(installed_files, list)
        or not all(isinstance(item, str) and item for item in installed_files)
        or installed_files != sorted(set(installed_files))
    ):
        raise SourceExtensionSetValidationError(
            "extension-set installed package inventory is invalid"
        )
    missing_required = sorted(
        set(extension_set.required_installed_files) - set(installed_files)
    )
    if missing_required:
        raise SourceExtensionSetValidationError(
            "extension-set installed package inventory is missing configured files: "
            + ", ".join(missing_required)
        )
    missing_installed = [
        relative
        for relative in installed_files
        if not (
            publish_root
            / validate_source_package_relative_path(
                relative,
                field="extension-set installed_package_files entry",
            )
        ).is_file()
    ]
    if missing_installed:
        raise SourceExtensionSetValidationError(
            "extension-set installed package files are absent on disk: "
            + ", ".join(missing_installed)
        )
    package_root = publish_root / extension_set.package
    actual_installed = {
        path.relative_to(publish_root).as_posix()
        for path in package_root.rglob("*")
        if path.is_file()
        and not path.name.endswith(
            (
                ".molt.a",
                ".molt.wasm",
                ".molt.a.extension_manifest.json",
                ".molt.wasm.extension_manifest.json",
            )
        )
    }
    if set(installed_files) != actual_installed:
        raise SourceExtensionSetValidationError(
            "extension-set installed package inventory differs from bytes: "
            f"missing={sorted(actual_installed - set(installed_files))!r}, "
            f"unexpected={sorted(set(installed_files) - actual_installed)!r}"
        )

    configured_contracts = tuple(
        (
            spec.module,
            spec.target,
            spec.python_exports,
            spec.capabilities,
            spec.provided_capsules,
            spec.exclude_linked_static_libraries,
        )
        for spec in extension_set.extensions
    )
    raw_extensions = set_manifest.get("extensions")
    extension_entry_keys = {
        "module",
        "target",
        "python_exports",
        "capabilities",
        "provided_capsules",
        "exclude_linked_static_libraries",
        "artifact_sha256",
        "wheel_sha256",
        "object_closure_sha256",
    }
    if not isinstance(raw_extensions, list) or not all(
        isinstance(item, Mapping)
        and set(item) == extension_entry_keys
        and isinstance(item.get("module"), str)
        and isinstance(item.get("target"), str)
        and isinstance(item.get("python_exports"), list)
        and isinstance(item.get("capabilities"), list)
        and isinstance(item.get("provided_capsules"), list)
        and isinstance(item.get("exclude_linked_static_libraries"), list)
        and all(isinstance(value, str) for value in item["python_exports"])
        and all(isinstance(value, str) for value in item["capabilities"])
        and all(isinstance(value, str) for value in item["provided_capsules"])
        and all(
            isinstance(value, str) for value in item["exclude_linked_static_libraries"]
        )
        and all(
            isinstance(item.get(field), str)
            and len(item[field]) == 64
            and all(character in "0123456789abcdef" for character in item[field])
            for field in (
                "artifact_sha256",
                "wheel_sha256",
                "object_closure_sha256",
            )
        )
        for item in raw_extensions
    ):
        raise SourceExtensionSetValidationError(
            "extension-set manifest extensions must be module objects"
        )
    manifest_contracts = tuple(
        (
            str(item["module"]),
            str(item["target"]),
            tuple(item["python_exports"]),
            tuple(item["capabilities"]),
            tuple(item["provided_capsules"]),
            tuple(item["exclude_linked_static_libraries"]),
        )
        for item in raw_extensions
    )
    if manifest_contracts != configured_contracts:
        raise SourceExtensionSetValidationError(
            "extension-set manifest typed extension contracts differ from "
            f"configured complete set: expected {configured_contracts}, "
            f"got {manifest_contracts}"
        )

    target_triple = set_manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        raise SourceExtensionSetValidationError(
            "extension-set manifest has no target-triple authority"
        )
    artifact_suffix = source_extension_artifact_suffix(target_triple)
    expected_artifacts = {
        publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}{artifact_suffix}",
        ).resolve()
        for spec in extension_set.extensions
    }
    expected_sidecars = {
        artifact.with_name(f"{artifact.name}.extension_manifest.json")
        for artifact in expected_artifacts
    }
    actual_artifacts = {
        path.resolve()
        for suffix in (".molt.a", ".molt.wasm")
        for path in publish_root.glob(f"**/*{suffix}")
        if path.is_file()
    }
    actual_sidecars = {
        path.resolve()
        for suffix in (".molt.a", ".molt.wasm")
        for path in publish_root.glob(f"**/*{suffix}.extension_manifest.json")
        if path.is_file()
    }
    if actual_artifacts != expected_artifacts:
        missing = sorted(str(path) for path in expected_artifacts - actual_artifacts)
        unexpected = sorted(str(path) for path in actual_artifacts - expected_artifacts)
        raise SourceExtensionSetValidationError(
            "published extension artifacts differ from configured complete set; "
            f"missing={missing}, unexpected={unexpected}"
        )
    if actual_sidecars != expected_sidecars:
        missing = sorted(str(path) for path in expected_sidecars - actual_sidecars)
        unexpected = sorted(str(path) for path in actual_sidecars - expected_sidecars)
        raise SourceExtensionSetValidationError(
            "published extension sidecars differ from configured complete set; "
            f"missing={missing}, unexpected={unexpected}"
        )
    entries_by_module = {
        str(item["module"]): item
        for item in raw_extensions
        if isinstance(item, Mapping)
    }
    for spec in extension_set.extensions:
        sidecar_path = publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}{artifact_suffix}.extension_manifest.json",
        ).resolve()
        try:
            sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise SourceExtensionSetValidationError(
                f"failed to read published extension sidecar {sidecar_path}: {exc}"
            ) from exc
        if not isinstance(sidecar, Mapping) or sidecar.get("module") != spec.module:
            raise SourceExtensionSetValidationError(
                f"published extension sidecar has wrong module: {sidecar_path}"
            )
        expected_sidecar_contract = {
            "name": extension_set.package,
            "version": extension_set.package_version,
            "module": spec.module,
            "abi_tier": variant.abi_tier,
            "target_python": variant.target_python.tag,
            "python_tag": f"py{variant.target_python.major}",
            "target_triple": variant.target_triple,
            "artifact_kind": target_plan.artifact_kind,
        }
        sidecar_mismatches = [
            f"{field}: expected {expected!r}, got {sidecar.get(field)!r}"
            for field, expected in expected_sidecar_contract.items()
            if sidecar.get(field) != expected
        ]
        if sidecar_mismatches:
            raise SourceExtensionSetValidationError(
                "extension sidecar differs from set variant contract: "
                + "; ".join(sidecar_mismatches)
            )
        if sidecar.get("deterministic") is not True:
            raise SourceExtensionSetValidationError(
                "extension sidecar requires deterministic=true"
            )
        raw_wheel = sidecar.get("wheel")
        if (
            not isinstance(raw_wheel, str)
            or not raw_wheel
            or Path(raw_wheel).is_absolute()
            or "\\" in raw_wheel
        ):
            raise SourceExtensionSetValidationError(
                f"extension sidecar wheel path is invalid for {spec.module}"
            )
        wheel_path = (sidecar_path.parent / raw_wheel).resolve()
        if (
            not wheel_path.is_relative_to(publish_root.resolve())
            or not wheel_path.is_file()
            or sidecar.get("wheel_sha256") != _sha256_file(wheel_path)
        ):
            raise SourceExtensionSetValidationError(
                f"extension sidecar wheel is not sealed or checksummed for {spec.module}"
            )
        try:
            _validate_compact_source_extension_manifest(sidecar)
            _require_location_neutral(
                sidecar,
                authority=f"published extension sidecar {sidecar_path}",
            )
        except ValueError as exc:
            raise SourceExtensionSetValidationError(str(exc)) from exc
        entry = entries_by_module[spec.module]
        closure = sidecar.get("object_closure")
        closure_sha256 = (
            closure.get("closure_sha256") if isinstance(closure, Mapping) else None
        )
        if not isinstance(closure, Mapping) or closure_sha256 != (
            source_extension_object_closure_digest(
                closure,
                manifest_dir=sidecar_path.parent,
                manifest=sidecar,
            )
        ):
            raise SourceExtensionSetValidationError(
                f"extension sidecar object closure identity is false for {spec.module}"
            )
        checksums = {
            "artifact_sha256": sidecar.get("extension_sha256"),
            "wheel_sha256": sidecar.get("wheel_sha256"),
            "object_closure_sha256": closure_sha256,
        }
        for field_name, sidecar_value in checksums.items():
            if entry.get(field_name) != sidecar_value:
                raise SourceExtensionSetValidationError(
                    f"extension-set manifest {field_name} differs from sidecar for "
                    f"{spec.module}"
                )
        closure_objects = (
            closure.get("objects") if isinstance(closure, Mapping) else None
        )
        if not isinstance(closure_objects, list):
            raise SourceExtensionSetValidationError(
                f"extension sidecar object closure is invalid for {spec.module}"
            )
        for object_index, closure_object in enumerate(closure_objects):
            if not isinstance(closure_object, Mapping):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] is invalid"
                )
            closure_object = cast(Mapping[str, Any], closure_object)
            source = closure_object.get("source")
            try:
                compile_command = _manifest_sequence(
                    sidecar, closure_object, "compile_command"
                )
                symbol_command = _manifest_sequence(
                    sidecar, closure_object, "symbol_command"
                )
            except ValueError as exc:
                raise SourceExtensionSetValidationError(str(exc)) from exc
            compiler_role = (
                "cpp"
                if isinstance(source, str)
                and Path(source).suffix.lower() in {".cc", ".cpp", ".cxx", ".c++"}
                else "c"
            )
            expected_compiler = target_commands[compiler_role]
            expected_nm = target_commands["nm"]
            if not (
                isinstance(compile_command, list)
                and compile_command[: len(expected_compiler)] == expected_compiler
                and isinstance(symbol_command, list)
                and symbol_command == expected_nm
            ):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "did not consume the canonical compiler/nm commands"
                )
            if closure_object.get("unit_sha256") != _object_unit_sha256(
                sidecar, closure_object
            ):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "has false content-addressed unit identity"
                )
        artifact_path = Path(str(sidecar_path).removesuffix(".extension_manifest.json"))
        if _sha256_file(artifact_path) != entry.get("artifact_sha256"):
            raise SourceExtensionSetValidationError(
                f"extension-set manifest artifact checksum differs from bytes for "
                f"{spec.module}"
            )


@dataclass(frozen=True, slots=True)
class ValidatedSourceExtensionSetSeal:
    seal: SourcePackageSeal
    set_manifest: Mapping[str, Any]
    canonical_identity: Mapping[str, Any]

    @property
    def payload_root(self) -> Path:
        return self.seal.payload_root


def validate_source_extension_set_seal(
    root: Path,
    extension_set: SourceExtensionSet,
    *,
    variant: SourceExtensionVariant,
    registry: SourceExtensionRegistry | None = None,
) -> ValidatedSourceExtensionSetSeal:
    """Verify the seal, exact package-set schema, bytes, and registered identity."""

    selected = load_source_extension_registry() if registry is None else registry
    registered = require_registered_source_extension_set(
        extension_set,
        registry=selected,
    )
    seal = verify_source_package_seal(root)
    manifest_path = seal.payload_root / "extension_set_manifest.json"
    try:
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionSetValidationError(
            f"cannot read source-extension set manifest {manifest_path}: {exc}"
        ) from exc
    if not isinstance(payload, Mapping):
        raise SourceExtensionSetValidationError(
            f"source-extension set manifest is not an object: {manifest_path}"
        )
    validate_source_extension_set_publish_root(
        publish_root=seal.payload_root,
        extension_set=registered,
        variant=variant,
        set_manifest=payload,
    )
    inventory = {entry.relative_path: entry.sha256 for entry in seal.files}
    identity = _require_expected_source_extension_set_identity(
        seal.payload_root,
        source_extension_set_expected_identity(
            registered,
            variant=variant,
            registry=selected,
        ),
        inventory_sha256=inventory,
    )
    return ValidatedSourceExtensionSetSeal(
        seal=seal,
        set_manifest=payload,
        canonical_identity=identity,
    )
