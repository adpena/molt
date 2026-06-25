from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import build_pipeline as cli_build_pipeline
from molt.cli import non_native_output as cli_non_native_output


_NON_NATIVE_OUTPUT_NAMES = {
    "_generate_snapshot_header",
    "_prepare_non_native_build_result",
    "_replace_directory_tree_from_source",
}


def test_non_native_output_authority_lives_in_non_native_output_module() -> None:
    for name in _NON_NATIVE_OUTPUT_NAMES:
        owner = getattr(cli_non_native_output, name)
        assert inspect.getmodule(owner) is cli_non_native_output
        assert not hasattr(cli_build_pipeline, name)
        assert not hasattr(cli, name)
