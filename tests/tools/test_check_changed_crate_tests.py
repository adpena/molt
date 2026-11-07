"""Whole-crate lib-test cells for the Cargo packages a change reaches (HF-701).

The cells are derived from the committed proof plan and the real workspace,
so these tests read both; synthetic sources pin the edge cases.
"""

from __future__ import annotations

import json
import tomllib
from pathlib import Path

import pytest

from tools import check_changed_crate_tests as gate
from tools.proof_plan import excluded_backend_crates, excluded_backends
from tools.check_changed_clippy import Reach, Workspace, reach

ROOT = Path(__file__).resolve().parents[2]
HOST = "aarch64-apple-darwin"
EXPERIMENTAL = excluded_backend_crates(excluded_backends(ROOT))


@pytest.fixture(scope="module")
def plan() -> dict:
    return tomllib.loads(
        (ROOT / "tools" / "proof_plan.toml").read_text(encoding="utf-8")
    )


@pytest.fixture(scope="module")
def libs() -> dict[str, gate.Library]:
    return gate.libraries(ROOT)


def _reach(*packages: str) -> Reach:
    return Reach(frozenset(packages), False, False, frozenset())


def _select(plan, libs, *packages: str) -> gate.Selection:
    return gate.select(
        gate.plan_sources(plan, host=HOST),
        _reach(*packages),
        libs,
        gate.runtime_gate(plan),
        experimental=EXPERIMENTAL,
    )


def _cells_for(selection: gate.Selection, package: str) -> list[gate.Cell]:
    return [cell for cell in selection.cells if package in cell.packages]


def test_a_native_backend_source_reaches_its_reverse_dependencies() -> None:
    change = reach(
        Workspace.load(ROOT),
        ["runtime/molt-backend-native/src/simple_backend/tests/trampolines.rs"],
    )
    assert {"molt-backend-native", "molt-backend", "molt-jit"} <= change.packages
    assert not change.whole_workspace


def test_hf205_cell_is_compiler_authorities_dev_fast_with_its_config(
    plan, libs
) -> None:
    """HF-205 failed only in dev-fast; the cell must keep the source's build."""
    selection = _select(plan, libs, "molt-backend-native", "molt-backend", "molt-jit")
    (cell,) = [
        cell
        for cell in selection.cells
        if "command:rust.test.compiler-authorities" in cell.sources
    ]
    argv = cell.argv()
    assert argv[:2] == ("cargo", "test")
    joined = " ".join(argv)
    assert "--profile dev-fast" in joined
    assert "--config profile.dev-fast.package.cranelift-codegen.opt-level=0" in joined
    assert "--no-default-features" in argv
    assert "-p molt-backend -p molt-backend-native --lib --no-fail-fast" in joined
    # Name filters drop; the plan's known-red exclusions and harness flags stay.
    libtest = argv[argv.index("--") + 1 :]
    assert "--include-ignored" in libtest and "--test-threads=1" in libtest
    assert "read::coff::coff_extended_relocations" in libtest
    assert "--bins" not in argv and "--test" not in argv and "--timings" not in argv
    # molt-wasm-facts is not selected, so its qualified feature narrows the cell.
    assert cell.dropped == {"molt-wasm-facts/host-tool"}
    profiles = {
        cell.key.build[cell.key.build.index("--profile") + 1]
        for cell in _cells_for(selection, "molt-backend-native")
        if "--profile" in cell.key.build
    }
    assert profiles == {"dev-fast", "release-fast"}
    assert any("molt-jit: no lib target" in item for item in selection.unverified)


def test_wasm_backend_never_runs_its_empty_default_features(plan, libs) -> None:
    selection = _select(plan, libs, "molt-backend-wasm")
    cells = _cells_for(selection, "molt-backend-wasm")
    assert cells
    for cell in cells:
        assert "--all-features" in cell.key.build or (
            "molt-backend-wasm/test-util" in cell.key.features
        ), cell.argv()
    assert "molt-backend-wasm" in cells[0].declared


def test_runtime_runs_through_its_runtime_gate_and_core_gets_lib_cells(
    plan, libs
) -> None:
    selection = _select(plan, libs, "molt-runtime", "molt-runtime-core")
    assert selection.runner is not None
    assert "tools/run_runtime_test_gate.py" in selection.runner
    assert selection.runner_packages == {"molt-runtime"}
    assert not _cells_for(selection, "molt-runtime")
    assert _cells_for(selection, "molt-runtime-core")


def test_a_crate_no_source_names_runs_its_default_truth_unit(plan, libs) -> None:
    named = {
        name for source in gate.plan_sources(plan, host=HOST) for name in source.names
    }
    undeclared = sorted(
        name
        for name, lib in libs.items()
        if lib.present
        and lib.harnessed
        and name not in named
        and name != "molt-runtime"
        and name not in EXPERIMENTAL
    )
    assert undeclared, "every member has a declared source; pick another witness"
    selection = _select(plan, libs, undeclared[0])
    (cell,) = selection.cells
    assert cell.sources == ["default-truth"]
    assert cell.argv()[:7] == (
        "cargo",
        "test",
        "--locked",
        "-p",
        undeclared[0],
        "--lib",
        "--no-fail-fast",
    )
    assert not cell.declared


def test_an_experimental_backend_crate_runs_in_no_presubmit_cell(plan, libs) -> None:
    # HF-620 removed the Luau packages from every presubmit source; without the
    # exclusion the crate fell to its default-truth unit, whose Lune oracles
    # need a toolchain the plan no longer provisions.
    assert "molt-backend-luau" in EXPERIMENTAL
    selection = _select(plan, libs, "molt-backend-luau", "molt-backend-native")
    assert not _cells_for(selection, "molt-backend-luau")
    assert _cells_for(selection, "molt-backend-native")
    assert any(
        note.startswith("molt-backend-luau: experimental backend")
        for note in selection.unverified
    )


def test_sources_parse_features_targets_and_filters() -> None:
    source = gate.parse_source(
        "rule:x",
        [
            "cargo",
            "test",
            "--locked",
            "--profile",
            "dev-fast",
            "-p",
            "a",
            "-p",
            "b@1.0.0",
            "--features",
            "a/x,b/y",
            "--test",
            "it",
            "--timings",
            "--no-fail-fast",
            "--",
            "family::",
            "--skip",
            "slow",
            "--test-threads=1",
            "--exact",
        ],
        host=HOST,
    )
    assert source is not None
    assert source.names == {"a": "a", "b": "b@1.0.0"}
    assert source.build == ("--locked", "--profile", "dev-fast")
    assert source.features == ("a/x", "b/y")
    assert source.libtest == ("--test-threads=1",)
    assert source.skips == ("slow",)
    assert source.unrunnable is None
    foreign = gate.parse_source(
        "command:y",
        ["cargo", "test", "-p", "a", "--target", "x86_64-unknown-linux-gnu", "--lib"],
        host=HOST,
    )
    assert foreign is not None and "not this host" in (foreign.unrunnable or "")
    assert (
        gate.parse_source("rule:z", ["cargo", "test", "-p", "a", "--no-run"], host=HOST)
        is None
    )
    assert (
        gate.parse_source("rule:z", ["cargo", "clippy", "-p", "a"], host=HOST) is None
    )


def test_features_filter_per_source_then_cells_merge_by_key() -> None:
    lib = gate.Library(present=True, harnessed=True)
    libs = {name: lib for name in ("a", "b", "c")}
    sources = [
        gate.parse_source(
            "rule:one", ["cargo", "test", "-p", "a", "--features", "x"], host=HOST
        ),
        gate.parse_source(
            "rule:two",
            ["cargo", "test", "-p", "a", "-p", "c", "--features", "a/x,c/z", "--lib"],
            host=HOST,
        ),
        gate.parse_source(
            "rule:three",
            ["cargo", "test", "-p", "b", "--target", "x86_64-unknown-linux-gnu"],
            host=HOST,
        ),
    ]
    selection = gate.select(
        sources, _reach("a", "b"), libs, None, experimental=frozenset()
    )
    # rule:one's bare feature is a/x; rule:two loses c/z (c is not reached).
    (cell,) = selection.cells
    assert cell.sources == ["rule:one", "rule:two"]
    assert cell.key.features == ("a/x",)
    assert cell.packages == {"a": "a"}
    assert cell.dropped == {"c/z"}
    assert selection.unverified == (
        f"b [rule:three]: --target x86_64-unknown-linux-gnu is not this host ({HOST})",
    )


def _artifact(manifest: Path, executable: str) -> str:
    return json.dumps(
        {
            "reason": "compiler-artifact",
            "manifest_path": str(manifest),
            "target": {"name": "molt_ir", "kind": ["lib"]},
            "profile": {"test": True},
            "executable": executable,
        }
    )


def test_accounting_binds_each_summary_to_its_package() -> None:
    manifest = ROOT / "runtime" / "molt-ir" / "Cargo.toml"
    log = [
        _artifact(manifest, "/t/dev-fast/deps/molt_ir-0123abcd"),
        "   Compiling molt-ir v0.1.0",
        "     Running unittests src/lib.rs (target/dev-fast/deps/molt_ir-0123abcd)",
        "running 6 tests",
        "test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; "
        "finished in 0.10s",
    ]
    result = gate.account(log, {"molt-ir": "molt-ir"}, ROOT)
    assert result.counts == {"molt-ir": {"passed": 5, "failed": 0, "ignored": 1}}
    assert result.problems == []


def test_accounting_fails_a_missing_summary_and_an_unmapped_binary() -> None:
    log = [
        "     Running unittests src/lib.rs (target/dev-fast/deps/stranger-99)",
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
        "finished in 0.01s",
    ]
    result = gate.account(log, {"molt-ir": "molt-ir"}, ROOT)
    assert result.counts == {}
    assert result.problems == [
        "test binary stranger-99 maps to no selected package",
        "molt-ir: no lib test summary (build or run failed)",
    ]


def test_libs_without_tests_name_members_with_a_lib(libs) -> None:
    for name in gate.LIBS_WITHOUT_TESTS:
        assert name in libs and libs[name].present, name


def test_cells_carry_their_sources_toolchains(plan, libs) -> None:
    selection = _select(plan, libs, "molt-backend-native")
    (llvm,) = [cell for cell in selection.cells if "rule:backend-llvm" in cell.sources]
    assert {"llvm-sdk", "clang", "llvm-config"} <= llvm.toolchains
