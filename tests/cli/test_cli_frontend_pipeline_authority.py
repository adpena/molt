from __future__ import annotations

import inspect
import json
from pathlib import Path
import shutil
from types import SimpleNamespace

import pytest

from molt.compilation_target import admit_target

import molt.cli as cli
from molt.cli import frontend_pipeline
from molt.browser_asset_closure import (
    BROWSER_WASM_ENTRY_ASSETS,
    wasm_loader_asset_payloads,
)

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


def _frontend_args(source_root: Path, *, target: str, split_runtime: bool):
    return dict(
        prepared_build_preamble=SimpleNamespace(),
        prepared_build_roots=SimpleNamespace(molt_root=source_root),
        prepared_build_config=SimpleNamespace(),
        resolved_build_entry=SimpleNamespace(),
        parse_codec="msgpack",
        type_hint_policy="ignore",
        profile="dev",
        json_output=True,
        target=admit_target(target),
        verbose=False,
        out_dir=None,
        trusted=False,
        split_runtime=split_runtime,
        require_linked=False,
        linked=False,
        linked_output=None,
        emit=None,
        output=None,
        emit_ir=None,
        type_facts_path=None,
        tree_shake=True,
        stdlib_profile="auto",
    )


@pytest.fixture
def browser_asset_source(tmp_path):
    """Use the shipped graph and assets, without a compiler or Node process."""
    source = tmp_path / "source"
    wasm = source / "wasm"
    wasm.mkdir(parents=True)
    original = Path(__file__).resolve().parents[2] / "wasm"
    graph_name = "browser_asset_graph.generated.json"
    graph = json.loads((original / graph_name).read_text(encoding="utf-8"))
    for name in (*graph["assets"], graph_name):
        destination = wasm / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original / name, destination)
    return source


class _FrontendPreparationReached(Exception):
    pass


@pytest.mark.parametrize("target", ["wasm", "wasm-freestanding"])
@pytest.mark.parametrize("corrupt", [False, True], ids=["valid", "stale-abi"])
def test_split_browser_assets_are_admitted_before_frontend_or_build_launch(
    browser_asset_source, monkeypatch, capsys, target, corrupt
):
    if corrupt:
        (browser_asset_source / "wasm/wasm_abi_generated.json").write_bytes(
            b"unadmitted ABI generation\n"
        )

    def frontend_launch_trap(**_):
        # Module discovery, runtime Cargo and backend launch all follow this
        # boundary. The valid control proves rejection is the real asset gate.
        raise _FrontendPreparationReached

    monkeypatch.setattr(
        frontend_pipeline, "_prepare_frontend_stage_state", frontend_launch_trap
    )
    args = _frontend_args(browser_asset_source, target=target, split_runtime=True)
    if not corrupt:
        with pytest.raises(_FrontendPreparationReached):
            frontend_pipeline._prepare_frontend_pipeline(**args)
        assert capsys.readouterr().out == ""
        return

    assert frontend_pipeline._prepare_frontend_pipeline(**args) == (None, 2)
    diagnostic = json.loads(capsys.readouterr().out)
    assert diagnostic["command"] == "build"
    assert diagnostic["status"] == "error"
    assert diagnostic["data"]["returncode"] == 2
    assert len(diagnostic["errors"]) == 1
    assert "Invalid split-runtime browser asset closure" in diagnostic["errors"][0]
    assert "hash drift for wasm_abi_generated.json" in diagnostic["errors"][0]


@pytest.mark.parametrize(
    ("target", "split_runtime"),
    [
        ("wasm", False),
        ("wasm-freestanding", False),
        ("native", False),
        ("native", True),
        ("rust", True),
        ("luau", True),
        ("mlir", False),
    ],
)
def test_builds_without_browser_publication_do_not_require_browser_assets(
    tmp_path, monkeypatch, target, split_runtime
):
    def frontend_launch_trap(**_):
        raise _FrontendPreparationReached

    monkeypatch.setattr(
        frontend_pipeline, "_prepare_frontend_stage_state", frontend_launch_trap
    )
    source = tmp_path / "source-without-browser-assets"
    with pytest.raises(_FrontendPreparationReached):
        frontend_pipeline._prepare_frontend_pipeline(
            **_frontend_args(source, target=target, split_runtime=split_runtime)
        )
    assert not source.exists()


def test_early_browser_admission_does_not_cache_later_asset_generations(
    browser_asset_source, monkeypatch
):
    def frontend_launch_trap(**_):
        (browser_asset_source / "wasm/wasm_abi_generated.json").write_bytes(
            b"changed after early admission\n"
        )
        # Late deployment staging retains its real validator. Early admission
        # is not a cached permission to publish a subsequent source generation.
        with pytest.raises(ValueError, match="hash drift for wasm_abi_generated.json"):
            wasm_loader_asset_payloads(
                browser_asset_source / "wasm", BROWSER_WASM_ENTRY_ASSETS
            )
        raise _FrontendPreparationReached

    monkeypatch.setattr(
        frontend_pipeline, "_prepare_frontend_stage_state", frontend_launch_trap
    )
    with pytest.raises(_FrontendPreparationReached):
        frontend_pipeline._prepare_frontend_pipeline(
            **_frontend_args(browser_asset_source, target="wasm", split_runtime=True)
        )
