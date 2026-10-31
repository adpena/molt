from __future__ import annotations

from tests.artifact_targets import artifact_target as _target

import inspect
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import backend_pipeline as cli_backend_pipeline
from molt.cli import build_pipeline as cli_build_pipeline
from molt.cli.build_output_layout import _BuildOutputLayout


_BACKEND_PIPELINE_NAMES = {
    "_run_backend_pipeline",
}


def test_backend_pipeline_authority_lives_in_backend_pipeline_module() -> None:
    for name in _BACKEND_PIPELINE_NAMES:
        owner = getattr(cli_backend_pipeline, name)
        assert inspect.getmodule(owner) is cli_backend_pipeline
        assert not hasattr(cli_build_pipeline, name)
        assert not hasattr(cli, name)


@pytest.mark.parametrize("kind", ["native-object", "rust", "luau", "wasm"])
def test_backend_requested_artifact_is_private_until_publication(tmp_path: Path, kind):
    output = tmp_path / "program.out"
    output.write_bytes(b"previous artifact")
    layout = _BuildOutputLayout(
        is_wasm=kind == "wasm",
        is_wasm_freestanding=False,
        is_rust_transpile=kind == "rust",
        is_luau_transpile=kind == "luau",
        is_mlir_emit=False,
        split_runtime=False,
        linked=False,
        target_triple=None,
        emit_mode="obj" if kind == "native-object" else "bin",
        output_artifact=output,
        output_binary=None,
        linked_output_path=None,
        emit_ir_path=None,
        facts_target=_target(None),
    )
    with cli_backend_pipeline._staged_backend_output(layout, project_root=tmp_path) as (
        staged,
        destination,
    ):
        assert destination == output
        assert staged.output_artifact != output
        staged.output_artifact.write_bytes(b"new backend output")
        assert output.read_bytes() == b"previous artifact"
        candidate = staged.output_artifact
    assert output.read_bytes() == b"previous artifact"
    assert not candidate.exists()
