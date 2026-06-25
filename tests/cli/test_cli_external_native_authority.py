from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import external_native
from molt.cli import wrapper_build

_EXTERNAL_NATIVE_NAMES = (
    "_external_native_artifact_output_custody_error",
    "_external_native_support_source_paths",
    "_external_package_dir",
    "_external_package_source_root",
    "_parse_external_static_packages",
    "_resolve_external_package_native_artifact_plan",
    "_resolve_import_admission_policy",
    "_stage_external_package_native_artifacts_for_build",
    "_validate_external_package_native_artifact",
)

_EXTERNAL_NATIVE_DEFINITIONS = (
    "def _external_native_artifact_output_custody_error(",
    "def _external_native_support_source_paths(",
    "def _external_package_dir(",
    "def _external_package_source_root(",
    "def _parse_external_static_packages(",
    "def _resolve_external_package_native_artifact_plan(",
    "def _resolve_import_admission_policy(",
    "def _stage_external_package_native_artifacts_for_build(",
    "def _validate_external_package_native_artifact(",
)


def test_cli_external_native_authority_is_single_home() -> None:
    for name in _EXTERNAL_NATIVE_NAMES:
        assert getattr(cli, name) is getattr(external_native, name)

    cli_source = inspect.getsource(cli)
    for marker in _EXTERNAL_NATIVE_DEFINITIONS:
        assert marker not in cli_source


def test_wrapper_build_uses_external_native_authority_directly() -> None:
    source = inspect.getsource(wrapper_build)
    assert "cli._parse_external_static_packages" not in source
    assert "cli._resolve_external_package_native_artifact_plan" not in source
