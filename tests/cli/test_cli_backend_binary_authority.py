from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import backend_binary
from molt.cli import build_pipeline

_BACKEND_BINARY_NAMES = (
    "_artifact_newer_than_sources",
    "_backend_fingerprint",
    "_backend_fingerprint_path",
    "_ensure_backend_binary",
)


def test_cli_backend_binary_authority_is_single_home() -> None:
    for name in _BACKEND_BINARY_NAMES:
        assert hasattr(backend_binary, name), name
        assert not hasattr(cli, name), name
        assert not hasattr(build_pipeline, name), name

    cli_source = inspect.getsource(cli)
    build_pipeline_source = inspect.getsource(build_pipeline)
    for name in _BACKEND_BINARY_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in build_pipeline_source
