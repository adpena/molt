"""Landing test selection derived from the dependency graph (findings family C).

The synthetic repositories pin each edge kind against a hand-written oracle.
The committed-repository cases are the four selections that hand rules missed
on 2026-10-29; each must now hold by construction.
"""

from __future__ import annotations

import textwrap
import tomllib
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools.molt_dev_policy import GateConfig
from tools.test_dependency_graph import (
    RepositoryTree,
    build_graph,
    graph_selection,
    landing_envelopes,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
COMMITTED_GATES = REPO_ROOT / "tools" / "proof_plan.toml"


def _git(root: Path, *arguments: str) -> str:
    return run_guarded_test_process(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout


def _repository(root: Path, files: dict[str, str]) -> Path:
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text), encoding="utf-8")
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(
        root,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "-m",
        "fixture",
    )
    return root


_FILES = {
    "pyproject.toml": """
        [tool.pytest.ini_options]
        addopts = ["-p", "pkg.plugin"]
    """,
    "src/pkg/__init__.py": """
        def main():
            from pkg import heavy
            return heavy.run()
    """,
    "src/pkg/plugin.py": "PLUGIN = True\n",
    "src/pkg/core.py": "def value():\n    return 1\n",
    "src/pkg/heavy.py": "def run():\n    return 2\n",
    "src/pkg/lazy.py": """
        def cheap():
            return 0

        def costly():
            from pkg import heavy
            return heavy.run()
    """,
    "config/data.toml": "x = 1\n",
    "config/other.toml": "y = 1\n",
    "runtime/crate/src/a.rs": "fn a() {}\n",
    "runtime/crate/src/b.rs": "fn b() {}\n",
    "tools/data_reader.py": """
        from pathlib import Path

        ROOT = Path(__file__).resolve().parents[1]
        DATA = ROOT / "config" / "data.toml"
        SOURCES = ROOT / "runtime/crate/src"

        def load():
            return DATA.read_text()

        def scan():
            return sorted(SOURCES.rglob("*.rs"))
    """,
    "tools/gen_table.py": "OUTPUT = 'src/pkg/table_generated.py'\n",
    "tools/gen_host.py": "OUTPUT = 'config/host_generated.toml'\n",
    "src/pkg/table_generated.py": "TABLE = 1\n",
    "config/host_generated.toml": "z = 1\n",
    "tools/generator_manifest.toml": """
        [[generator]]
        tool = "tools/gen_table.py"
        outputs = ["src/pkg/table_generated.py"]
        check_mode = true
        sync_test = "tests/test_generator_sync.py"

        [[generator]]
        tool = "tools/gen_host.py"
        outputs = ["config/host_generated.toml"]
        check_mode = true
        ci_checkable = false
    """,
    "tests/conftest.py": """
        import pytest

        @pytest.fixture(autouse=True)
        def _isolated():
            from pkg import core
            yield core.value()

        @pytest.fixture
        def heavy_fixture():
            from pkg import heavy
            return heavy.run()
    """,
    "tests/test_core.py": "from pkg import core\n\ndef test_core():\n    assert core.value()\n",
    "tests/test_lazy.py": "from pkg.lazy import cheap\n\ndef test_cheap():\n    assert not cheap()\n",
    "tests/test_main.py": "import pkg\n\ndef test_main():\n    assert pkg.main()\n",
    "tests/test_reader.py": "from tools.data_reader import load\n\ndef test_load():\n    assert load()\n",
    "tests/test_scan.py": "from tools.data_reader import scan\n\ndef test_scan():\n    assert scan()\n",
    "tests/test_heavy.py": "def test_heavy(heavy_fixture):\n    assert heavy_fixture\n",
    "tests/test_literal.py": """
        from pathlib import Path

        def test_reads():
            assert Path("config/other.toml").exists()
            assert "tests/test_core.py"
    """,
    "tests/test_generator_sync.py": "def test_sync():\n    pass\n",
    "tests/test_host_reader.py": """
        def test_reads():
            assert open("config/host_generated.toml").read()
    """,
}


@pytest.fixture(scope="module")
def graph(tmp_path_factory):
    root = _repository(tmp_path_factory.mktemp("repo"), _FILES)
    return build_graph(RepositoryTree.from_worktree(root), cache=None)


def _select(graph, *paths):
    return set(graph.select(paths))


HARNESS_ONLY = {
    "tests/test_literal.py",
    "tests/test_generator_sync.py",
    "tests/test_host_reader.py",
}


def test_import_edges_bind_only_the_functions_a_test_imports(graph):
    # test_main calls pkg.main, whose body imports heavy; the package init
    # alone (executed for ``from pkg import core``) does not. test_lazy imports
    # only lazy.cheap, which never reaches heavy.
    assert _select(graph, "src/pkg/heavy.py") == {
        "tests/test_main.py",
        "tests/test_heavy.py",
    }


def test_harness_setup_binds_every_test_and_requested_fixtures_bind_their_users(
    graph,
):
    every = set(graph.tests)
    # The autouse fixture imports pkg.core; the -p plugin and the conftest
    # file itself run for every test below it.
    assert _select(graph, "src/pkg/core.py") == every
    assert _select(graph, "src/pkg/plugin.py") == every
    assert _select(graph, "tests/conftest.py") == every
    assert "tests/test_heavy.py" in _select(graph, "src/pkg/heavy.py")


def test_path_constants_bind_their_readers_at_use_sites(graph):
    assert _select(graph, "config/data.toml") == {"tests/test_reader.py"}
    # A directory a tool scans stands for every file under it.
    assert _select(graph, "runtime/crate/src/b.rs") == {"tests/test_scan.py"}
    assert _select(graph, "config/other.toml") == {"tests/test_literal.py"}


def test_a_test_file_edit_selects_only_that_test(graph):
    # test_literal names test_core's path as a string, which reads no bytes.
    assert _select(graph, "tests/test_core.py") == {"tests/test_core.py"}
    assert _select(graph, "tests/test_literal.py") == {"tests/test_literal.py"}


def test_generator_provenance(graph):
    # A generator's sync test executes it. A checked generator's output is
    # re-rendered by the always freshness gate, so the output's own edit
    # selects its readers; an unchecked (host-input) output is selected by
    # its generator's edit as well.
    assert _select(graph, "tools/gen_table.py") == {"tests/test_generator_sync.py"}
    assert _select(graph, "tools/gen_host.py") == {"tests/test_host_reader.py"}
    assert _select(graph, "config/host_generated.toml") == {"tests/test_host_reader.py"}


def test_untouched_inputs_select_nothing(graph):
    assert _select(graph, "README.md") == set()
    assert HARNESS_ONLY.isdisjoint(_select(graph, "src/pkg/heavy.py"))


def test_commit_tree_and_cache_agree_with_the_worktree(tmp_path):
    root = _repository(tmp_path / "repo", _FILES)
    cache = tmp_path / "facts.json"
    worktree = build_graph(RepositoryTree.from_worktree(root), cache=cache)
    assert worktree.stats["parsed_blobs"] == worktree.stats["python_files"]
    head = _git(root, "rev-parse", "HEAD").strip()
    committed = build_graph(RepositoryTree.from_commit(root, head), cache=cache)
    # Facts are keyed by Git blob id, so the commit's tree parses nothing new.
    assert committed.stats["parsed_blobs"] == 0
    for path in ("src/pkg/heavy.py", "config/data.toml", "runtime/crate/src/a.rs"):
        assert committed.select([path]) == worktree.select([path])
    # An uncommitted edit is part of the worktree, not of the commit.
    (root / "tests/test_core.py").write_text(
        "from tools.data_reader import load\n\ndef test_core():\n    assert load()\n",
        encoding="utf-8",
    )
    edited = build_graph(RepositoryTree.from_worktree(root), cache=cache)
    assert edited.stats["parsed_blobs"] == 1
    assert "tests/test_core.py" in edited.select(["config/data.toml"])
    assert "tests/test_core.py" not in committed.select(["config/data.toml"])


def test_landing_envelopes_come_from_pre_push_cells_without_builds():
    commands = [
        {
            "id": "unit",
            "tiers": ["pre-push", "pr"],
            "resource_class": "python-tests",
            "toolchains": ["python", "uv"],
            "pytest": ["tests/unit/", "tests/test_nodes.py::test_a"],
        },
        {
            "id": "build",
            "tiers": ["pre-push", "pr"],
            "resource_class": "compiler-build-resource",
            "toolchains": ["python", "cargo"],
            "pytest": ["tests/test_built.py", "tests/test_nodes.py::test_b"],
        },
        {
            "id": "main-only",
            "tiers": ["main"],
            "resource_class": "python-tests",
            "toolchains": ["python"],
            "pytest": ["tests/test_late.py"],
        },
    ]
    envelopes, others = landing_envelopes(commands)
    assert envelopes["tests/unit/"].targets == ("tests/unit/",)
    # A node-owned file runs only its pre-push nodes.
    assert envelopes["tests/test_nodes.py"].targets == ("tests/test_nodes.py::test_a",)
    assert "tests/test_built.py" not in envelopes
    assert others["tests/test_built.py"] == ("build",)
    assert others["tests/test_late.py"] == ("main-only",)


# --------------------------------------------------------------------------
# The committed repository: the four selections hand rules missed
# --------------------------------------------------------------------------


def _landing_tests(paths):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _ = cfg.select(paths)
    return {
        argument.partition("::")[0]
        for gate in gates
        if isinstance(gate.command, tuple) and "pytest" in gate.command
        for argument in gate.command
        if argument.startswith("tests/")
    }


@pytest.mark.parametrize(
    "changed,test",
    [
        # HF-197 step 4a: the test reads dead_functions.rs by literal path.
        (
            "runtime/molt-tir/src/passes/dead_functions.rs",
            "tests/cli/test_required_features_reachability.py",
        ),
        # HF-555: the test reads TIR pass sources through tools/op_kinds/paths.py.
        ("runtime/molt-passes/src/tir/passes/scev.rs", "tests/test_gen_op_kinds.py"),
        # The regrtest shim's own test imports it.
        ("tools/molt_regrtest_shim.py", "tests/test_molt_regrtest_shim.py"),
    ],
)
def test_missed_consumers_are_selected_by_construction(changed, test):
    assert test in _landing_tests([changed])


def test_frontend_edits_select_the_frontend_lowering_cell():
    # HF-557's frontend edits selected none of this cell. Independent oracle:
    # every cell file whose source imports molt.frontend must run.
    changed = [
        "src/molt/frontend/_protocol.py",
        "src/molt/frontend/lowering/import_lowering.py",
        "src/molt/frontend/visitors/call_dispatch_named.py",
        "src/molt/frontend/visitors/statement_scope.py",
    ]
    plan = tomllib.loads(COMMITTED_GATES.read_text(encoding="utf-8"))
    (cell,) = [
        command
        for command in plan["command"]
        if command["id"] == "python.unit.frontend-lowering.macos"
    ]
    readers = {
        target
        for target in cell["pytest"]
        if "molt.frontend" in (REPO_ROOT / target).read_text(encoding="utf-8")
    }
    assert len(readers) >= 20
    assert readers <= _landing_tests(changed)


def test_committed_selection_reports_deferred_build_cells():
    cfg = GateConfig.load(COMMITTED_GATES)
    selection = cfg.graph_selection(["src/molt/cli/build_command.py"])
    assert selection is not None
    # A build-owned test is selected and named with its cell, not dropped.
    assert selection.deferred
    assert set(selection.deferred) <= set(selection.selected)
    assert set(selection.run).isdisjoint(selection.deferred)
    assert (
        graph_selection(
            cfg.dependency_graph(), ["src/molt/cli/build_command.py"], cfg.commands
        ).selected
        == selection.selected
    )


def _pytest_requests(gates) -> set[tuple[tuple[str, ...], str]]:
    """(pytest selector, target) for every target the selected cohorts run."""
    requests = set()
    for gate in gates:
        if not (gate.pytest and isinstance(gate.command, tuple)):
            continue
        argv = gate.command
        start = next(
            index + 1
            for index, argument in enumerate(argv)
            if argument.startswith("--molt-worker-memory-gb")
        )
        first = next(
            index
            for index, argument in enumerate(argv)
            if index >= start and argument.startswith("tests/")
        )
        selector = tuple(argv[start:first])
        requests.update((selector, target) for target in argv[first:])
    return requests


def _covers(full, selector, target) -> bool:
    path = target.partition("::")[0]
    return any(
        (have == target and scope in {(), selector})
        or (have == path and (scope == () or (scope == selector and path == target)))
        for scope, have in full
    )


def test_the_graph_never_drops_what_a_rule_selects():
    # For one tracked path per rule glob, the full landing selection keeps
    # every gate and every pytest target (at the same or a wider scope) that
    # the rules select on their own.
    from dataclasses import replace
    from fnmatch import fnmatchcase

    from tools.dirty_tree_policy import is_ignored

    cfg = GateConfig.load(COMMITTED_GATES)
    rules_only = replace(cfg, dependency_graph=None)
    tracked = sorted(cfg.dependency_graph().tree.blobs)
    paths: set[str] = set()
    for rule in cfg.rules:
        for glob in rule.globs:
            if not any(character in glob for character in "*?["):
                if glob in cfg.dependency_graph().tree.blobs:
                    paths.add(glob)
                continue
            match = next(
                (
                    path
                    for path in tracked
                    if fnmatchcase(path, glob) and is_ignored(path, (glob,))
                ),
                None,
            )
            if match is not None:
                paths.add(match)
    paths.discard("tools/proof_plan.toml")
    assert len(paths) > 300
    dropped = []
    for path in sorted(paths):
        before, _ = rules_only.select([path])
        after, _ = cfg.select([path])
        commands = {gate.command for gate in after}
        dropped.extend(
            (path, gate.command)
            for gate in before
            if not gate.pytest and gate.command not in commands
        )
        full = _pytest_requests(after)
        dropped.extend(
            (path, selector, target)
            for selector, target in _pytest_requests(before)
            if not _covers(full, selector, target)
        )
    assert dropped == []
