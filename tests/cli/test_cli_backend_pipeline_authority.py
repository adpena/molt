from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import backend_pipeline as cli_backend_pipeline
from molt.cli import build_pipeline as cli_build_pipeline


_BACKEND_PIPELINE_NAMES = {
    "_run_backend_pipeline",
}


def test_backend_pipeline_authority_lives_in_backend_pipeline_module() -> None:
    for name in _BACKEND_PIPELINE_NAMES:
        owner = getattr(cli_backend_pipeline, name)
        assert inspect.getmodule(owner) is cli_backend_pipeline
        assert not hasattr(cli_build_pipeline, name)
        assert not hasattr(cli, name)
