"""Build-tool and metadata custody for recorded source-extension sets."""

from __future__ import annotations
from pathlib import Path
from typing import Any, Mapping, cast
from packaging.requirements import Requirement
from packaging.utils import canonicalize_name
from packaging.version import InvalidVersion, Version
from molt.cli.source_build_environment import source_build_environment_problems
from molt.cli.source_extension_set_validation_schema import (
    RecordedSourceExtensionSet,
    SourceExtensionSetValidationError,
)
from molt.cli.source_extension_toolchain import MOLT_PKGCONF_REQUIREMENT
from molt.file_hashing import _sha256_file


def validate_source_extension_build_custody(
    *,
    publish_root: Path,
    extension_set: RecordedSourceExtensionSet,
    set_manifest: Mapping[str, Any],
) -> None:
    build_environment = set_manifest["build_environment"]
    problems = source_build_environment_problems(build_environment)
    if problems:
        raise SourceExtensionSetValidationError("; ".join(problems))
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
        or not isinstance(meson.get("driver"), Mapping)
        or not isinstance(meson.get("backend"), Mapping)
        or not isinstance(meson.get("config_tools"), list)
        or not isinstance(meson.get("generated_inputs"), list)
    ):
        raise SourceExtensionSetValidationError(
            "extension-set Meson metadata differs from recorded build contract"
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
            "extension-set Meson pkg-config requirement differs from recorded custody"
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
