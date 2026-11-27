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
from tools.cargo_reach import Reach, Workspace, reach

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
    assert "-p molt-backend -p molt-backend-native --lib --test " in joined
    # The source's integration targets that a selected crate owns run too;
    # one that only an unselected crate owns would make Cargo refuse the cell.
    owned = libs["molt-backend"].tests | libs["molt-backend-native"].tests
    assert cell.tests and cell.tests <= owned
    assert {"daemon_server", "native_batch_worker_spawn"} <= cell.tests
    assert "wasm_link_facts_cli" not in cell.tests
    # Name filters drop; the plan's known-red exclusions and harness flags stay.
    libtest = argv[argv.index("--") + 1 :]
    assert "--include-ignored" in libtest and "--test-threads=1" in libtest
    assert "read::coff::coff_extended_relocations" in libtest
    assert "--bins" not in argv and "--timings" not in argv
    # molt-wasm-facts and molt-lang-cpython-abi are not selected, so their
    # qualified features narrow the cell.
    assert cell.dropped == {
        "molt-wasm-facts/host-tool",
        "molt-lang-cpython-abi/extension-loader",
    }
    profiles = {
        cell.key.build[cell.key.build.index("--profile") + 1]
        for cell in _cells_for(selection, "molt-backend-native")
        if "--profile" in cell.key.build
    }
    assert profiles == {"dev-fast", "release-fast"}
    assert any("molt-jit: no lib target" in item for item in selection.unverified)


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
    assert cell.argv()[:9] == (
        "cargo",
        "test",
        "--locked",
        "--profile",
        gate.DEFAULT_TRUTH_PROFILE,
        "-p",
        undeclared[0],
        "--lib",
        "--no-fail-fast",
    )
    assert not cell.declared


def test_the_default_truth_profile_keeps_the_test_profiles_assertions() -> None:
    """dev-fast must inherit dev as the built-in test profile does, so the
    default-truth unit checks the same debug assertions (HF-205)."""
    profiles = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))[
        "profile"
    ]
    chosen = profiles[gate.DEFAULT_TRUTH_PROFILE]
    assert chosen.get("inherits") == "dev"
    for setting in ("debug-assertions", "overflow-checks", "opt-level"):
        assert setting not in chosen, setting


def test_the_runtime_gate_runs_once_when_its_own_rule_is_selected(
    plan: dict, libs: dict[str, gate.Library]
) -> None:
    changed = ["runtime/molt-runtime/src/object/ops_dict.rs"]
    assert gate.runtime_rule_selected(plan, changed)
    selection = gate.select(
        gate.plan_sources(plan, host=HOST),
        _reach("molt-runtime"),
        libs,
        gate.runtime_gate(plan),
        experimental=EXPERIMENTAL,
        runner_in_batch=gate.runtime_rule_selected(plan, changed),
    )
    assert selection.runner is None
    assert selection.runner_in_batch
    assert selection.runner_packages == {"molt-runtime"}
    assert not _cells_for(selection, "molt-runtime")


def test_a_change_reaching_the_runtime_only_by_dependency_keeps_its_runner(
    plan: dict, libs: dict[str, gate.Library]
) -> None:
    # The runtime-lib rule globs one molt-runtime-core file; another file of
    # that crate reaches molt-runtime only as a reverse dependency.
    changed = ["runtime/molt-runtime-core/src/lib.rs"]
    assert not gate.runtime_rule_selected(plan, changed)
    selection = gate.select(
        gate.plan_sources(plan, host=HOST),
        _reach("molt-runtime", "molt-runtime-core"),
        libs,
        gate.runtime_gate(plan),
        experimental=EXPERIMENTAL,
        runner_in_batch=gate.runtime_rule_selected(plan, changed),
    )
    assert selection.runner is not None
    assert not selection.runner_in_batch


def test_an_experimental_backend_crate_runs_in_no_presubmit_cell(plan, libs) -> None:
    # An excluded backend's crate has no presubmit source; without the exclusion
    # it would fall to its default-truth unit.
    assert "molt-backend-mlir" in EXPERIMENTAL
    selection = _select(plan, libs, "molt-backend-mlir", "molt-backend-native")
    assert not _cells_for(selection, "molt-backend-mlir")
    assert _cells_for(selection, "molt-backend-native")
    assert any(
        note.startswith("molt-backend-mlir: experimental backend")
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


def test_a_declared_integration_target_runs_only_where_a_selected_crate_owns_it() -> (
    None
):
    libs = {
        "a": gate.Library(present=True, harnessed=True, tests=frozenset({"it"})),
        "b": gate.Library(present=True, harnessed=True, tests=frozenset({"other"})),
    }
    source = gate.parse_source(
        "rule:x",
        ["cargo", "test", "-p", "a", "-p", "b", "--test", "it", "--test", "other"],
        host=HOST,
    )
    assert source is not None and source.tests == ("it", "other")
    (cell,) = gate.select(
        [source], _reach("a"), libs, None, experimental=frozenset()
    ).cells
    assert cell.tests == {"it"}
    assert cell.argv()[-4:-2] == ("--test", "it")


def test_accounting_requires_and_checks_each_integration_summary() -> None:
    manifest = ROOT / "runtime" / "molt-ir" / "Cargo.toml"
    lib = [
        _artifact(manifest, "/t/dev-fast/deps/molt_ir-0123abcd"),
        "     Running unittests src/lib.rs (target/dev-fast/deps/molt_ir-0123abcd)",
        "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
        "finished in 0.10s",
    ]
    failing = [
        "     Running tests/contracts.rs (target/dev-fast/deps/contracts-77)",
        "test result: FAILED. 7 passed; 2 failed; 0 ignored; 0 measured; "
        "0 filtered out; finished in 0.01s",
    ]
    result = gate.account(
        lib + failing, {"molt-ir": "molt-ir"}, ROOT, ["contracts", "missing"]
    )
    # The integration summary never counts as the lib's.
    assert result.counts == {"molt-ir": {"passed": 5, "failed": 0, "ignored": 0}}
    assert result.problems == [
        "integration test contracts: 2 failed",
        "integration test missing: no summary (build or run failed)",
    ]


def _failing_lib_log(manifest: Path, *, nocapture: bool) -> list[str]:
    noise = ["output the test printed\n"] if nocapture else []
    return [
        _artifact(manifest, "/t/dev-fast/deps/molt_tir-0123abcd"),
        "     Running unittests src/lib.rs (target/dev-fast/deps/molt_tir-0123abcd)\n",
        "running 3 tests\n",
        "test passes::one ... ok\n",
        *(
            ["test fails::two ... ", *noise, "FAILED\n"]
            if nocapture
            else ["test fails::two ... FAILED\n"]
        ),
        "test fails::three ... FAILED\n",
        "\n",
        "failures:\n",
        "\n",
        "---- fails::two stdout ----\n",
        "    not_a_name but indented\n",
        "\n",
        "failures:\n",
        "    fails::three\n",
        "    fails::two\n",
        "\n",
        "test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; "
        "0 filtered out; finished in 0.01s\n",
    ]


@pytest.mark.parametrize("nocapture", [False, True])
def test_accounting_names_failed_tests_from_libtests_failure_list(
    nocapture: bool,
) -> None:
    # --nocapture interleaves test output with `test NAME ... FAILED`, so the
    # names come from libtest's last `failures:` block.
    manifest = ROOT / "runtime" / "molt-tir" / "Cargo.toml"
    result = gate.account(
        _failing_lib_log(manifest, nocapture=nocapture), {"molt-tir": "molt-tir"}, ROOT
    )
    assert result.counts == {"molt-tir": {"passed": 1, "failed": 2, "ignored": 0}}
    assert result.failed == {"--lib": ["fails::three", "fails::two"]}
    assert "passes::one" in result.passed


def _cell(libtest: tuple[str, ...] = ()) -> gate.Cell:
    cell = gate.Cell(
        gate.CellKey(("--locked", "--profile", "dev-fast"), ("molt-tir/x",), libtest)
    )
    cell.packages = {"molt-tir": "molt-tir", "molt-ir": "molt-ir"}
    cell.sources = ["rule:tir"]
    cell.skips = {"skipped::one"}
    return cell


def test_a_failed_cell_reruns_only_its_failed_tests_one_at_a_time() -> None:
    cell = _cell(("--include-ignored", "--nocapture", "--test-threads=4"))
    result = gate.CellResult(
        (),
        101,
        {"molt-tir": {"passed": 1, "failed": 1, "ignored": 0}},
        ["cell exited 101", "integration test contracts: 1 failed"],
        {"--lib": ["a::b"], "--test contracts": ["c::d"]},
    )
    (rerun,) = gate.failed_tests([cell], {cell.label: result})
    assert rerun["tests"] == ["a::b", "c::d"]
    # The cell's build (packages, features), only the failed targets, its
    # libtest flags minus --nocapture and thread count, and exact names.
    assert rerun["argv"] == [
        "cargo", "test", "--locked", "--profile", "dev-fast",
        "--features", "molt-tir/x", "-p", "molt-ir", "-p", "molt-tir",
        "--lib", "--test", "contracts",
        "--no-fail-fast", "--message-format=json-render-diagnostics",
        "--", "--include-ignored", "--test-threads=1", "--exact", "a::b", "c::d",
    ]  # fmt: skip


@pytest.mark.parametrize(
    "problems,failed",
    [
        # A build failure is not a failed test.
        (["cell exited 101", "molt-tir: no lib test summary (build or run failed)"], {"--lib": ["a::b"]}),
        # The summary counts a failure the failure list did not name.
        (["cell exited 101"], {}),
    ],
)  # fmt: skip
def test_a_failure_that_is_not_a_named_test_reruns_the_whole_gate(
    problems: list[str], failed: dict[str, list[str]]
) -> None:
    cell = _cell()
    result = gate.CellResult(
        (),
        101,
        {"molt-tir": {"passed": 1, "failed": 1, "ignored": 0}},
        problems,
        failed,
    )
    assert gate.failed_tests([cell], {cell.label: result}) is None


@pytest.mark.parametrize("keep_going", [False, True])
def test_build_failure_stops_queued_cells_by_default(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    keep_going: bool,
) -> None:
    first, second = _cell(), _cell()
    second.sources = ["rule:llvm"]
    selection = gate.Selection(
        _reach("molt-tir"), (first, second), None, frozenset(), ()
    )
    ran = []

    def run(cell: gate.Cell, **_kwargs: object) -> gate.CellResult:
        ran.append(cell.label)
        if cell is first:
            return gate.CellResult((), 101, {}, ["cell exited 101"])
        return gate.CellResult((), 0, {}, [])

    monkeypatch.setattr(gate, "selection_for", lambda _base: selection)
    monkeypatch.setattr(gate, "cell_workers", lambda _count: 1)
    monkeypatch.setattr(gate, "run_cell", run)

    assert gate.main(["--keep-going"] if keep_going else []) == 1

    expected = [first.label, second.label] if keep_going else [first.label]
    assert ran == expected
    result = json.loads(capsys.readouterr().out.splitlines()[-1])
    assert result["status"] == "failed"
    assert len(result["cells"]) == len(expected)
    assert len(result["unverified"]) == int(not keep_going)
    assert "failed_tests" not in result


@pytest.mark.parametrize("passes", [True, False])
def test_rerun_failed_passes_only_when_each_named_test_passes_alone(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    passes: bool,
) -> None:
    log = tmp_path / "gate.stdout.log"
    rerun = {
        "cell": "rule:tir",
        "argv": ["cargo", "test", "--", "--exact", "a::b", "c::d"],
        "toolchains": ["cargo", "rustc"],
        "packages": {"molt-tir": "molt-tir"},
        "tests": ["a::b", "c::d"],
    }
    log.write_text(
        "noise\n" + json.dumps({"schema": gate.SCHEMA, "failed_tests": [rerun]}) + "\n",
        encoding="utf-8",
    )
    ran: list[tuple[str, ...]] = []

    def run_cargo_test(
        argv: tuple[str, ...],
        toolchains: object,
        packages: object,
        tests: object,
        *,
        root: Path,
        plan: object,
        echo: list[str] | None = None,
    ) -> gate.CellResult:
        ran.append(argv)
        return gate.CellResult(
            argv, 0, {}, [], {}, {"a::b", "c::d"} if passes else {"a::b"}
        )

    monkeypatch.setattr(gate, "run_cargo_test", run_cargo_test)
    assert gate.main([gate.RERUN_OPTION, str(log)]) == (0 if passes else 1)
    assert ran == [tuple(rerun["argv"])]
    last = json.loads(capsys.readouterr().out.strip().splitlines()[-1])
    assert last["status"] == ("passed" if passes else "failed")


def test_cells_build_grouped_by_their_projected_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # molt-runtime-platform's build script tracks PATH: each switch between
    # projected environments rebuilds it and its dependents, so cells with
    # one environment build together, in declared order within the group.
    def toolchain_environment(
        plan: object, names: tuple[str, ...], *, environ: dict[str, str]
    ) -> dict[str, str]:
        return {"PATH": "/wasm:/bin"} if "wasm-ld" in names else {}

    class Plan:
        def toolchain_closure(self, names: list[str]) -> tuple[str, ...]:
            return tuple(names)

    monkeypatch.setattr(gate, "toolchain_environment", toolchain_environment)
    cells = []
    for label, toolchains in (
        ("a", {"cargo"}),
        ("b", {"cargo", "wasm-ld"}),
        ("c", {"cargo"}),
        ("d", {"cargo", "wasm-ld"}),
    ):
        cell = gate.Cell(gate.CellKey((label,), (), ()))
        cell.sources = [label]
        cell.toolchains = set(toolchains)
        cells.append(cell)
    ordered = gate.build_order(cells, Plan())  # type: ignore[arg-type]
    assert [cell.label for cell in ordered] == ["a", "c", "b", "d"]
