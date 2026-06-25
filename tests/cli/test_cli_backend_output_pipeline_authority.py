from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import backend_output_pipeline as cli_backend_output_pipeline
from molt.cli import build_pipeline as cli_build_pipeline


_BACKEND_OUTPUT_PIPELINE_NAMES = {
    "_emit_backend_pipeline_outputs",
}


def test_backend_output_pipeline_authority_lives_in_backend_output_module() -> None:
    for name in _BACKEND_OUTPUT_PIPELINE_NAMES:
        owner = getattr(cli_backend_output_pipeline, name)
        assert inspect.getmodule(owner) is cli_backend_output_pipeline
        assert not hasattr(cli_build_pipeline, name)
        assert not hasattr(cli, name)
