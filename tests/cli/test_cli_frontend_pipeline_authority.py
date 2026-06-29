from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import frontend_pipeline

_FRONTEND_PIPELINE_NAMES = (
    "_dead_module_elimination_extra_roots",
    "_dead_module_elimination_mode",
    "_dead_module_elimination_safelist",
    "_output_base_for_entry",
    "_prepare_frontend_analysis",
    "_prepare_frontend_lowering_config",
    "_prepare_frontend_pipeline",
    "_prepare_frontend_stage_state",
)


def test_cli_frontend_pipeline_authority_is_single_home() -> None:
    for name in _FRONTEND_PIPELINE_NAMES:
        assert hasattr(frontend_pipeline, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _FRONTEND_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source
