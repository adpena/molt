"""Exact module-relative package inventory for source-extension sets."""

from __future__ import annotations
from pathlib import Path
from typing import Any, Mapping
from molt.cli.source_extension_set_validation_schema import (
    RecordedSourceExtensionSet,
    SourceExtensionSetValidationError,
)
from molt.cli.source_package_seal import validate_source_package_relative_path


def validate_source_extension_installed_inventory(
    *,
    publish_root: Path,
    extension_set: RecordedSourceExtensionSet,
    set_manifest: Mapping[str, Any],
) -> tuple[str, ...]:
    installed_files = set_manifest.get("installed_package_files")
    if (
        not isinstance(installed_files, list)
        or not all(isinstance(item, str) and item for item in installed_files)
        or installed_files != sorted(set(installed_files))
    ):
        raise SourceExtensionSetValidationError(
            "extension-set installed package inventory is invalid"
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

    return tuple(installed_files)
