from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_pipeline
from molt.cli import link_pipeline

_LINK_PIPELINE_NAMES = (
    "_darwin_link_validation_failure",
    "_link_fingerprint",
    "_link_fingerprint_path",
    "_prepare_native_link",
    "_prepare_native_object_artifact",
    "_retry_native_link_without_hint",
    "_run_native_link_command",
    "_run_native_partial_link_command",
    "_validate_darwin_link_output",
)


def test_cli_link_pipeline_authority_is_single_home() -> None:
    for name in _LINK_PIPELINE_NAMES:
        assert hasattr(link_pipeline, name), name
        assert not hasattr(cli, name), name
        assert not hasattr(build_pipeline, name), name

    cli_source = inspect.getsource(cli)
    build_pipeline_source = inspect.getsource(build_pipeline)
    for name in _LINK_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in build_pipeline_source
