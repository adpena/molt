from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_pipeline

_BUILD_PIPELINE_NAMES = (
    "_build_cache_variant",
    "_execute_backend_compile",
    "_generate_snapshot_header",
    "_prepare_backend_cache_setup",
    "_prepare_backend_compile",
    "_prepare_backend_dispatch",
    "_prepare_backend_runtime_context",
    "_prepare_backend_setup",
    "_prepare_non_native_build_result",
    "_replace_directory_tree_from_source",
    "_run_backend_pipeline",
    "_run_build_pipeline",
    "_session_target_dir",
)


def test_cli_build_pipeline_authority_is_single_home() -> None:
    for name in _BUILD_PIPELINE_NAMES:
        assert hasattr(build_pipeline, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _BUILD_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source
