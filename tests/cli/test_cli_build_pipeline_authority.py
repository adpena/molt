from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_pipeline

_BUILD_PIPELINE_NAMES = (
    "_artifact_newer_than_sources",
    "_backend_fingerprint",
    "_backend_fingerprint_path",
    "_build_cache_variant",
    "_darwin_link_validation_failure",
    "_ensure_backend_binary",
    "_execute_backend_compile",
    "_generate_snapshot_header",
    "_link_fingerprint",
    "_link_fingerprint_path",
    "_prepare_backend_cache_setup",
    "_prepare_backend_compile",
    "_prepare_backend_dispatch",
    "_prepare_backend_runtime_context",
    "_prepare_backend_setup",
    "_prepare_native_link",
    "_prepare_native_object_artifact",
    "_prepare_non_native_build_result",
    "_replace_directory_tree_from_source",
    "_retry_native_link_without_hint",
    "_run_backend_pipeline",
    "_run_build_pipeline",
    "_run_native_link_command",
    "_run_native_partial_link_command",
    "_session_target_dir",
    "_validate_darwin_link_output",
)


def test_cli_build_pipeline_authority_is_single_home() -> None:
    for name in _BUILD_PIPELINE_NAMES:
        assert hasattr(build_pipeline, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _BUILD_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source
