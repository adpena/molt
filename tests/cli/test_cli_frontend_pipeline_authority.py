from __future__ import annotations

import inspect
import json
from pathlib import Path
import shutil
import sys
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest

from molt.compilation_target import admit_target

import molt.cli as cli
from molt.cli import entrypoint
from molt.cli import frontend_pipeline
from molt.cli import backend_ir, backend_pipeline
from molt.cli.function_records import encode_function_body
from molt.cli.output import CliFailure
from molt.browser_asset_closure import (
    BROWSER_WASM_ENTRY_ASSETS,
    wasm_loader_asset_payloads,
)

_FRONTEND_PIPELINE_NAMES = (
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


def test_dev_closure_omits_unread_bodies_with_or_without_diagnostics(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    entry = tmp_path / "entry.py"
    entry.write_text(
        "import provider\n"
        "from provider import selected\n"
        "provider.attribute\n"
        "getattr(provider, 'reflected')\n"
        "value = 42\n"
        "def _unused(argument=1, *, keyword=2):\n"
        "    global value\n"
        "    value = argument\n"
        "    return keyword\n"
        "def _other_unused(): return None\n"
        "def _callback(): return 3\n"
        "callbacks = [_callback]\n"
        "def closure(argument):\n"
        "    def capture(): return argument\n"
        "    return capture\n"
        "def resumable(): yield 7\n"
        "class Iterable:\n"
        "    def __iter__(self): return iter((8,))\n"
        "def live(): return value\n"
        "print(live(), selected(), callbacks[0](), closure(5)(), "
        "next(resumable()), next(iter(Iterable())))\n",
        encoding="utf-8",
    )
    (tmp_path / "provider.py").write_text(
        "def selected(): return 2\n"
        "def attribute(): return 1\n"
        "def reflected(): return 2\n",
        encoding="utf-8",
    )
    materialize = backend_ir.materialize_tir_closure
    results: list[backend_ir._PreparedBackendIR] = []

    def capture(
        *, prepared_backend_ir: backend_ir._PreparedBackendIR, **kwargs: object
    ) -> int:
        results.append(prepared_backend_ir)
        return 0

    def select(
        frontend: frontend_pipeline._PreparedFrontendPipelineBundle,
        **kwargs: object,
    ) -> tuple[backend_ir._PreparedBackendIR | None, CliFailure | None]:
        with ThreadPoolExecutor(max_workers=2) as executor:
            twins = [executor.submit(materialize, frontend, **kwargs) for _ in range(2)]
            left, right = [future.result() for future in twins]
        assert left is right
        return left

    monkeypatch.setattr(backend_pipeline, "_run_backend_pipeline", capture)
    monkeypatch.setattr(backend_ir, "materialize_tir_closure", select)
    for index, options in enumerate(
        (
            ("--release",),
            ("--build-profile", "dev"),
            ("--build-profile", "dev", "--diagnostics"),
        )
    ):
        monkeypatch.setattr(
            sys,
            "argv",
            [
                "molt",
                "build",
                *options,
                "--python-version",
                "3.12",
                "--stdlib-profile",
                "micro",
                "--emit",
                "obj",
                "--output",
                str(tmp_path / f"output-{index}.o"),
                str(entry),
            ],
        )
        assert entrypoint.main() == 0
    eager, demanded, observed = results
    assert observed.plan_seal == demanded.plan_seal
    assert observed.declarations == demanded.declarations
    assert observed.ir == demanded.ir
    assert eager.declarations is not None
    assert demanded.declarations is not None
    omitted = {"py.entry._other_unused", "py.entry._unused"}
    assert not eager.declarations.declaration_only
    assert demanded.declarations.declaration_only == omitted
    assert demanded.declarations.callables == eager.declarations.callables
    assert eager.ir["code_slots"] == demanded.ir["code_slots"]
    assert eager.plan_seal != demanded.plan_seal
    assert omitted.isdisjoint(
        seed.function["name"] for seed in demanded.function_digest_seeds
    )
    eager_functions = {function["name"]: function for function in eager.ir["functions"]}
    demanded_functions = {
        function["name"]: function for function in demanded.ir["functions"]
    }
    assert eager_functions.keys() - demanded_functions.keys() == omitted
    for symbol in demanded.declarations.callables.keys() - omitted:
        assert encode_function_body(demanded_functions[symbol]) == encode_function_body(
            eager_functions[symbol]
        ), symbol
    assert sum(
        op.get("s_value") == "molt_function_new_unreachable"
        for function in demanded_functions.values()
        for op in function["ops"]
    ) == len(omitted)


def test_demanded_leaf_edit_reuses_unchanged_module_until_demand_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import frontend_worker

    project = tmp_path / "project"
    project.mkdir()
    entry = project / "entry.py"
    provider = project / "provider.py"
    provider.write_text("def value(): return 40\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "build-state"))
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "molt-cache"))
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "0")
    monkeypatch.chdir(project)
    prepared: list[backend_ir._PreparedBackendIR] = []
    entry_hits: list[bool] = []
    load_lowered = frontend_worker._read_persisted_module_lowering
    materialize = backend_ir.materialize_tir_closure

    def observe(
        module_path: Path, *, module_name: str, **kwargs: object
    ) -> dict[str, object] | None:
        result = load_lowered(module_path, module_name=module_name, **kwargs)
        if module_name == "entry":
            entry_hits.append(result is not None)
        return result

    def capture(
        *, prepared_backend_ir: backend_ir._PreparedBackendIR, **kwargs: object
    ) -> int:
        prepared.append(prepared_backend_ir)
        return 0

    def compile_entry(
        expression: str, *, mode: backend_ir.BodyMaterializationMode
    ) -> backend_ir._PreparedBackendIR:
        def lower(
            frontend: frontend_pipeline._PreparedFrontendPipelineBundle,
            **kwargs: object,
        ) -> tuple[backend_ir._PreparedBackendIR | None, CliFailure | None]:
            return materialize(frontend, **{**kwargs, "mode": mode})

        text = (
            "from provider import value\ndef _formerly_unused(): return 2\nprint("
            + expression
            + ")\n"
        )
        if not entry.exists() or entry.read_text(encoding="utf-8") != text:
            entry.write_text(text, encoding="utf-8")
        monkeypatch.setattr(backend_ir, "materialize_tir_closure", lower)
        monkeypatch.setattr(
            sys,
            "argv",
            [
                "molt",
                "build",
                "--python-version",
                "3.12",
                "--stdlib-profile",
                "micro",
                "--emit",
                "obj",
                "--output",
                str(tmp_path / "output.o"),
                str(entry),
            ],
        )
        before = len(prepared)
        assert entrypoint.main() == 0
        assert len(prepared) == before + 1
        return prepared[-1]

    monkeypatch.setattr(frontend_worker, "_read_persisted_module_lowering", observe)
    monkeypatch.setattr(backend_pipeline, "_run_backend_pipeline", capture)
    omitted = "py.entry._formerly_unused"
    first = compile_entry("value() + 1", mode="demanded")
    provider.write_text("def value(): return 41\n", encoding="utf-8")
    edit = compile_entry("value() + 1", mode="demanded")
    assert first.declarations is not None and edit.declarations is not None
    assert omitted in first.declarations.declaration_only
    assert first.declarations.declaration_only == edit.declarations.declaration_only
    assert first.plan_seal != edit.plan_seal
    assert entry_hits == [False, True]

    eager_edit = compile_entry("value() + 1", mode="eager")
    changed = compile_entry("value() + _formerly_unused()", mode="demanded")
    assert changed.declarations is not None
    assert omitted not in changed.declarations.declaration_only
    assert entry_hits[-1] is False
    eager_changed = compile_entry("value() + _formerly_unused()", mode="eager")
    for actual, oracle in ((edit, eager_edit), (changed, eager_changed)):
        assert actual.declarations is not None
        expected = {body["name"]: body for body in oracle.ir["functions"]}
        actual_bodies = {body["name"]: body for body in actual.ir["functions"]}
        assert (
            expected.keys() - actual_bodies.keys()
            == actual.declarations.declaration_only
        )
        for name in (
            actual.declarations.callables.keys() - actual.declarations.declaration_only
        ):
            assert encode_function_body(actual_bodies[name]) == encode_function_body(
                expected[name]
            ), name

    monkeypatch.setenv("MOLT_DISABLE_FRONTEND_LOWERING_CACHE", "1")
    fresh = compile_entry("value() + 1", mode="demanded")
    assert fresh.declarations == edit.declarations
    assert [encode_function_body(body) for body in fresh.ir["functions"]] == [
        encode_function_body(body) for body in edit.ir["functions"]
    ]


def _frontend_args(source_root: Path, *, target: str, split_runtime: bool):
    return dict(
        prepared_build_preamble=SimpleNamespace(),
        prepared_build_roots=SimpleNamespace(molt_root=source_root),
        prepared_build_config=SimpleNamespace(),
        resolved_build_entry=SimpleNamespace(),
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
        stdlib_profile="auto",
        include_modules=(),
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
