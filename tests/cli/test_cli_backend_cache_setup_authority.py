from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import backend_cache_setup
from molt.cli import build_pipeline

_BACKEND_CACHE_SETUP_NAMES = (
    "_build_cache_variant",
    "_prepare_backend_cache_setup",
)


def test_cli_backend_cache_setup_authority_is_single_home() -> None:
    for name in _BACKEND_CACHE_SETUP_NAMES:
        assert hasattr(backend_cache_setup, name), name
        assert not hasattr(cli, name), name
        assert not hasattr(build_pipeline, name), name

    cli_source = inspect.getsource(cli)
    build_pipeline_source = inspect.getsource(build_pipeline)
    for name in _BACKEND_CACHE_SETUP_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in build_pipeline_source
