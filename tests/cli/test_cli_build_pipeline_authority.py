from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_pipeline

_BUILD_PIPELINE_NAMES = (
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
