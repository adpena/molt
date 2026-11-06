"""Landing test selection derived from the dependency graph (findings family C).

The synthetic repositories pin each edge kind against a hand-written oracle.
The committed-repository cases are the four selections that hand rules missed
on 2026-10-29; each must now hold by construction.
"""

from __future__ import annotations

import os
import textwrap
import tomllib
import json
import sys
import hashlib
from pathlib import Path
from dataclasses import replace

import pytest
from molt.python_environment_identity import PYTHON_IDENTITY_PROBE_TIMEOUT_S

from tests.process_guard_common import run_guarded_test_process
from tools.molt_dev_policy import GateConfig, LandingMode, select_landing_gates
from tools.proof_plan import PytestMemoryUnits
from tools.test_dependency_graph import (
    RepositoryTree,
    build_graph,
    graph_selection,
    landing_envelopes,
    ObservedTestInputs,
    OBSERVED_SCHEMA,
    TestInputRecorder as InputRecorder,
    _observation_path,
    observation_context,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
COMMITTED_GATES = REPO_ROOT / "tools" / "proof_plan.toml"


def test_observed_owner_includes_its_dynamic_plugin():
    from tools.test_dependency_graph import load_graph

    inputs = load_graph(REPO_ROOT).dependencies(
        "tests/cli/test_cli_import_collection.py"
    )
    assert {
        "tests/observed_inputs.py",
        "tools/test_dependency_graph.py",
        "tools/molt_dev_policy.py",
    } <= inputs


def _observed(directory: Path, key: str, **record) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    _observation_path(directory, key).write_text(
        json.dumps({"schema": OBSERVED_SCHEMA, "key": key, **record}), encoding="utf-8"
    )


def _observed_pair(directory: Path, test: str) -> ObservedTestInputs:
    nodes = [test + "::test_left", test + "::test_right"]
    _observed(
        directory,
        test,
        nodes=nodes,
        collection={"complete": True, "files": [], "directories": []},
    )
    for node, path in zip(
        nodes, ("config/data.toml", "config/other.toml"), strict=True
    ):
        _observed(directory, node, complete=True, files=[path], directories=[])
    return ObservedTestInputs(directory)


def test_observed_data_readers_keep_unknown_nodes_and_collection_inputs(tmp_path):
    test = "tests/test_assets.py"
    observed = _observed_pair(tmp_path, test)
    assert observed.readers(test, ["config/data.toml"]) == (test + "::test_left",)
    assert observed.readers(test, ["config/other.toml"]) == (test + "::test_right",)
    _observation_path(tmp_path, test + "::test_right").unlink()
    assert observed.readers(test, ["config/data.toml"]) == (
        test + "::test_left",
        test + "::test_right",
    )
    _observed(
        tmp_path,
        test,
        nodes=[test + "::test_left"],
        collection={"complete": True, "files": ["config/data.toml"], "directories": []},
    )
    assert observed.readers(test, ["config/data.toml"]) == (test,)


@pytest.mark.parametrize(
    "record",
    [
        None,
        {},
        {"complete": False},
        {"complete": True, "files": [3], "directories": []},
    ],
)
def test_observed_incomplete_or_malformed_node_cannot_be_dropped(tmp_path, record):
    test = "tests/test_assets.py"
    observed = _observed_pair(tmp_path, test)
    path = _observation_path(tmp_path, test + "::test_left")
    if record is None:
        path.unlink()
    else:
        _observed(tmp_path, test + "::test_left", **record)
    assert test + "::test_left" in observed.readers(test, ["unrelated.dat"])


def test_observed_directory_reads_cover_added_and_deleted_children(tmp_path):
    test = "tests/test_assets.py"
    observed = _observed_pair(tmp_path, test)
    _observed(
        tmp_path,
        test + "::test_left",
        complete=True,
        files=[],
        directories=["runtime/pkg"],
    )
    assert observed.readers(test, ["runtime/pkg/new.rs"]) == (test + "::test_left",)
    assert observed.readers(test, ["runtime/pkg/sub/deleted.rs"]) == (
        test + "::test_left",
    )
    assert observed.readers(test, ["runtime/other/new.rs"]) == ()


def test_observed_context_binds_host_configuration_environment_and_source(
    graph, monkeypatch
):
    context = observation_context(graph.tree, ())
    monkeypatch.setenv("CODEX_TOOL_CALL_ID", "another-shell-call")
    assert observation_context(graph.tree, ()) == context
    blobs = dict(graph.tree.blobs)
    blobs["runtime/crate/src/a.rs"] = "guest-edited"
    assert observation_context(replace(graph.tree, blobs=blobs), ()) == context
    blobs["docs/agent/V1_HANDOFF_FINDINGS.md"] = "ledger-edited"
    assert observation_context(replace(graph.tree, blobs=blobs), ()) == context
    blobs["config/data.toml"] = "configuration-edited"
    assert observation_context(replace(graph.tree, blobs=blobs), ()) != context
    monkeypatch.setenv("HF633_CONTEXT_INPUT", "different")
    assert observation_context(graph.tree, ()) != context


@pytest.mark.parametrize("complete", [True, False])
def test_observed_refinement_preserves_source_edits_postsubmit_and_node_envelopes(
    tmp_path,
    complete,
):
    root = _repository(
        tmp_path / "repo",
        {
            **_FILES,
            "tests/test_assets.py": "from pathlib import Path\ndef test_left():\n    return Path('config/data.toml').read_text()\ndef test_right():\n    return Path('config/other.toml').read_text()\n",
        },
    )
    graph = build_graph(RepositoryTree.from_worktree(root), cache=None)
    observed = _observed_pair(tmp_path / "reads", "tests/test_assets.py")
    assert (
        "tests/test_assets.py"
        in graph.direct_readers(["config/data.toml"]).data_readers
    )
    selected = graph.direct_readers(
        ["config/data.toml"], observations=observed
    ).data_readers
    assert "tests/test_assets.py::test_left" in selected
    assert "tests/test_assets.py::test_right" not in selected
    # A changed test is its own importer, never refined to observed nodes.
    assert graph.direct_readers(
        ["tests/test_assets.py"], observations=observed
    ).importers == ("tests/test_assets.py",)
    scoped = ObservedTestInputs(tmp_path / "reads", guest_only=True)
    assert (
        "tests/test_assets.py"
        in graph.direct_readers(["config/data.toml"], observations=scoped).data_readers
    )
    _observed(
        tmp_path / "reads",
        "tests/test_assets.py::test_left",
        complete=complete,
        files=["runtime/crate/src/a.rs"],
        directories=[],
    )
    assert (
        "tests/test_assets.py::test_left"
        in graph.direct_readers(
            ["runtime/crate/src/a.rs"], observations=observed
        ).data_readers
    )
    _observed_pair(tmp_path / "reads", "tests/test_assets.py")
    command = {
        "id": "fixture",
        "tiers": ["pre-push"],
        "pytest": ["tests/test_assets.py"],
        "toolchains": ["python"],
        "argv": ["python", "-m", "pytest"],
        "cell": "fixture-cell",
    }
    # Presubmit leaves the observed data reader to postsubmit, by node.
    selection = graph_selection(
        graph, ["config/data.toml"], [command], observations=observed
    )
    assert not selection.run
    assert "tests/test_assets.py::test_left" in selection.postsubmit_only
    assert "tests/test_assets.py::test_right" not in selection.postsubmit_only
    # A recent postsubmit failure among the data readers runs in presubmit, in
    # its node envelope.
    recent = graph_selection(
        graph,
        ["config/data.toml"],
        [command],
        observations=observed,
        recent_failures=["tests/test_assets.py::test_left"],
    )
    assert "tests/test_assets.py" not in recent.selected
    assert "tests/test_assets.py::test_left" in recent.selected
    assert "tests/test_assets.py::test_right" not in recent.selected
    envelope = recent.run["tests/test_assets.py::test_left"]
    assert envelope.targets == ("tests/test_assets.py::test_left",)
    assert envelope.cells == ("fixture-cell",)
    assert envelope.toolchains == ("python",)
    postsubmit = graph_selection(
        graph,
        ["config/data.toml"],
        [command],
        mode=LandingMode.POSTSUBMIT,
        observations=observed,
    )
    assert postsubmit.run["tests/test_assets.py"].targets == ("tests/test_assets.py",)


@pytest.mark.parametrize("workers", [0, 2])
def test_observed_plugin_records_real_reads_and_shared_fixture_consumers(
    tmp_path, monkeypatch, workers
):
    from tools.test_dependency_graph import observation_directory

    root = _repository(
        tmp_path / "repo",
        {
            "pyproject.toml": "[tool.pytest.ini_options]\n",
            "conftest.py": 'pytest_plugins = ["tests.conftest"]\n',
            "runtime/a.rs": "a",
            "runtime/b.rs": "b",
            "tests/test_reads.py": """
            from pathlib import Path
            import pytest
            ROOT = Path(__file__).resolve().parents[1]
            @pytest.fixture(scope="session")
            def shared():
                return (ROOT / "runtime/a.rs").read_text()
            @pytest.mark.parametrize("tag", ["first", "second"])
            def test_shared(shared, tag):
                assert shared == "a"
            def test_other():
                assert (ROOT / "runtime/b.rs").read_text() == "b"
            def test_dynamic(request):
                assert request.getfixturevalue("shared") == "a"
            @pytest.fixture(params=["a", "b"])
            def per_case(request):
                return (ROOT / ("runtime/" + request.param + ".rs")).read_text()
            def test_per_case(per_case):
                assert per_case in {"a", "b"}
        """,
        },
    )
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    context = "a" * 64
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-n",
            str(workers),
            "-c",
            str(REPO_ROOT / "pyproject.toml"),
            "--rootdir",
            str(root),
            "-p",
            "tests.observed_inputs",
            "--molt-observed-context",
            context,
            str(root / "tests/test_reads.py"),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    observed = ObservedTestInputs(
        observation_directory(context),
        current_inputs=RepositoryTree.from_worktree(root).blobs,
    )
    assert set(observed.readers("tests/test_reads.py", ["runtime/a.rs"])) == {
        "tests/test_reads.py::test_shared[first]",
        "tests/test_reads.py::test_shared[second]",
        "tests/test_reads.py::test_per_case[a]",
        "tests/test_reads.py::test_dynamic",
    }
    assert set(observed.readers("tests/test_reads.py", ["runtime/b.rs"])) == {
        "tests/test_reads.py::test_other",
        "tests/test_reads.py::test_per_case[b]",
    }


@pytest.mark.parametrize("phase", ["collection", "setup", "call", "teardown"])
def test_interrupted_observation_retains_static_readers(tmp_path, monkeypatch, phase):
    from tools.test_dependency_graph import observation_directory

    test = "tests/test_interrupt.py"
    root = _repository(
        tmp_path / "repo",
        {
            "pyproject.toml": "[tool.pytest.ini_options]\n",
            "conftest.py": 'pytest_plugins = ["tests.conftest"]\n',
            "runtime/later.rs": "later",
            test: f"""
            from pathlib import Path
            import pytest
            PHASE = {phase!r}
            if PHASE == "collection":
                raise KeyboardInterrupt
            @pytest.fixture
            def capture():
                if PHASE == "setup":
                    raise KeyboardInterrupt
                yield
                if PHASE == "teardown":
                    raise KeyboardInterrupt
            def test_interrupt(capture):
                if PHASE == "call":
                    raise KeyboardInterrupt
                if False:
                    Path("runtime/later.rs").read_text()
            """,
        },
    )
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    context = "c" * 64
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-n",
            "0",
            "-c",
            str(REPO_ROOT / "pyproject.toml"),
            "--rootdir",
            str(root),
            "-p",
            "tests.observed_inputs",
            "--molt-observed-context",
            context,
            str(root / test),
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    )
    assert result.returncode == 2, result.stdout + result.stderr
    directory = observation_directory(context)
    key = "collection:" + test if phase == "collection" else test + "::test_interrupt"
    record = json.loads(_observation_path(directory, key).read_text(encoding="utf-8"))
    assert record["complete"] is False
    assert "runtime/later.rs" not in record["files"]
    graph = build_graph(RepositoryTree.from_worktree(root), cache=None)
    static = graph.direct_readers(["runtime/later.rs"]).data_readers
    assert test in static
    selected = graph.direct_readers(
        ["runtime/later.rs"], observations=ObservedTestInputs(directory)
    ).data_readers
    # A missing report must never turn an unread path into qualified absence.
    expected = test if phase == "collection" else test + "::test_interrupt"
    assert expected in selected
    assert {node.partition("::")[0] for node in selected} == set(static)


def test_observed_cached_file_argument_and_opaque_child_stay_selected(
    tmp_path, monkeypatch
):
    from molt.cli.module_source import _source_content_sha256

    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    source = REPO_ROOT / "runtime/molt-runtime/src/lib.rs"
    expected = hashlib.sha256(source.read_bytes()).hexdigest()
    assert _source_content_sha256(source) == expected
    recorder = InputRecorder(REPO_ROOT, "b" * 64)
    recorder.start("tests/test_reads.py::test_cached")
    try:
        assert _source_content_sha256(source) == expected
        assert (
            _source_content_sha256(REPO_ROOT / "runtime/hf633_not_created.rs") is None
        )
    finally:
        record = recorder.finish()
    assert "runtime/molt-runtime/src/lib.rs" in record["files"]
    assert "runtime/hf633_not_created.rs" in record["files"]
    recorder.start("tests/test_reads.py::test_child")
    try:
        sys.audit("subprocess.Popen", "unobserved-child", [], None, None)
    finally:
        opaque = recorder.finish()
    assert opaque["complete"] is False


def test_observed_thread_ownership_excludes_existing_observers(tmp_path, monkeypatch):
    import threading

    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    go, done = threading.Event(), threading.Event()
    source = REPO_ROOT / "runtime/molt-runtime/src/lib.rs"

    def background():
        go.wait()
        source.read_bytes()
        done.set()

    unrelated = threading.Thread(target=background)
    unrelated.start()
    recorder = InputRecorder(REPO_ROOT, "c" * 64)
    recorder.start("tests/test_reads.py::test_thread")
    try:
        go.set()
        assert done.wait(10)
        assert "runtime/molt-runtime/src/lib.rs" not in recorder.reads
        owned = threading.Thread(target=source.read_bytes)
        owned.start()
        owned.join(10)
        assert not owned.is_alive()
    finally:
        unrelated.join(10)
        record = recorder.finish()
    assert record["complete"] is True
    assert "runtime/molt-runtime/src/lib.rs" in record["files"]


def test_cached_use_report_does_not_activate_or_reopen_inputs(tmp_path, monkeypatch):
    from molt import input_report

    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    recorder = InputRecorder(REPO_ROOT, "d" * 64)
    source = REPO_ROOT / "runtime/not-created-hf633.rs"
    prior = input_report.active()
    recorder.start("tests/test_reads.py::test_metadata")
    try:
        input_report.report_use(source)
        assert input_report.active() == prior
    finally:
        record = recorder.finish()
    assert record["files"] == ["runtime/not-created-hf633.rs"]
    assert record["directories"] == []
    assert record["complete"] is True


def test_observed_dependency_drift_and_mixed_generations_retain_readers(tmp_path):
    test = "tests/test_assets.py"
    _observed_pair(tmp_path, test)
    guest = {"runtime/a.rs": "first", "runtime/b.rs": "second"}
    snapshot = hashlib.sha256(json.dumps(guest, sort_keys=True).encode()).hexdigest()
    _observed(tmp_path, "snapshot:" + snapshot, guest_inputs=guest)
    _observed(
        tmp_path,
        test,
        nodes=[test + "::test_left", test + "::test_right"],
        snapshot=snapshot,
        collection=dict(complete=True, files=[], directories=[], snapshot=snapshot),
    )
    for name, path in (("left", "runtime/a.rs"), ("right", "runtime/b.rs")):
        _observed(
            tmp_path,
            test + "::test_" + name,
            complete=True,
            files=[path],
            directories=[],
            snapshot=snapshot,
        )
    observed = ObservedTestInputs(tmp_path, current_inputs=guest)
    assert observed.readers(test, ["runtime/new.rs"]) == ()
    # A prior edit to a.rs may have introduced a conditional read of new.rs.
    drifted = ObservedTestInputs(
        tmp_path, current_inputs={**guest, "runtime/a.rs": "new"}
    )
    assert drifted.readers(test, ["runtime/new.rs"]) == (test + "::test_left",)
    assert drifted.readers(test, ["runtime/new.rs"], retain_unknown=False) == (
        test + "::test_left",
    )
    _observed(
        tmp_path,
        test + "::test_right",
        complete=True,
        files=[],
        directories=[],
        snapshot="different capture",
    )
    assert observed.readers(test, ["runtime/new.rs"]) == (test + "::test_right",)


def test_partial_postsubmit_preserves_valid_older_node_snapshots(tmp_path):
    test = "tests/test_assets.py"
    old = {"runtime/a.rs": "first", "runtime/b.rs": "second"}
    new = {**old, "runtime/a.rs": "changed"}
    generations = []
    for guest in (old, new):
        snapshot = hashlib.sha256(
            json.dumps(guest, sort_keys=True).encode()
        ).hexdigest()
        _observed(tmp_path, "snapshot:" + snapshot, guest_inputs=guest)
        generations.append(snapshot)
    before, current = generations
    nodes = [test + "::test_left", test + "::test_right"]
    for node, path in zip(nodes, old):
        _observed(
            tmp_path, node, complete=True, files=[path], directories=[], snapshot=before
        )
    collection = dict(complete=True, files=[], directories=[], snapshot=current)
    _observed(tmp_path, test, nodes=nodes, snapshot=current, collection=collection)
    observed = ObservedTestInputs(tmp_path, current_inputs=new)
    # A new collection must not discard an older unchanged nonreader.
    assert observed.readers(test, ["runtime/other.rs"]) == (nodes[0],)
    # A changed module-level guest input can change both nodes' demand.
    collection["files"] = ["runtime/a.rs"]
    _observed(tmp_path, test, nodes=nodes, snapshot=current, collection=collection)
    assert observed.readers(test, ["runtime/other.rs"]) == tuple(nodes)


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
    "tools/uses_reader.py": """
        from tools.data_reader import load

        def render():
            return load()
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
    # pytest reads pyproject's configuration for every run.
    assert _select(graph, "pyproject.toml") == every
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


def test_programs_reached_follow_every_hop_of_execution_and_data(graph):
    programs = ("tools/data_reader.py", "tools/uses_reader.py", "src/pkg/__init__.py")
    # uses_reader runs data_reader.load, which reads config/data.toml two hops
    # away; a test's first-hop inputs would not reach it, a program's do.
    assert graph.programs_reached(["config/data.toml"], programs) == (
        "tools/data_reader.py",
        "tools/uses_reader.py",
    )
    # A scanned directory stands for every file under it. uses_reader binds
    # only data_reader.load, so the scan function's directory is not its input.
    assert graph.programs_reached(["runtime/crate/src/b.rs"], programs) == (
        "tools/data_reader.py",
    )
    # pkg.main imports heavy in its body: the execution closure reaches it.
    assert graph.programs_reached(["src/pkg/heavy.py"], programs) == (
        "src/pkg/__init__.py",
    )
    assert graph.programs_reached(["tools/uses_reader.py"], programs) == (
        "tools/uses_reader.py",
    )
    assert graph.programs_reached(["config/other.toml"], programs) == ()


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
    assert envelopes["tests/test_late.py"].targets == ("tests/test_late.py",)
    assert "tests/test_late.py" not in others


@pytest.fixture(scope="module")
def landing_graph(tmp_path_factory):
    root = _repository(
        tmp_path_factory.mktemp("landing-graph"),
        {
            "src/pkg/__init__.py": "",
            "src/pkg/hub.py": "def value():\n    return 1\n",
            "src/pkg/middle.py": "from pkg.hub import value\n",
            "tests/test_direct.py": "from pkg.hub import value\n",
            "tests/test_transitive.py": "from pkg.middle import value\n",
            "tests/test_changed.py": "def test_changed():\n    pass\n",
            "tests/test_recent.py": "def test_recent():\n    pass\n",
        },
    )
    return build_graph(RepositoryTree.from_worktree(root), cache=None)


def _landing_commands(graph):
    return [
        {
            "id": "ordinary",
            "tiers": ["pre-push", "main"],
            "resource_class": "python-tests",
            "toolchains": ["python"],
            "argv": ["python", "-m", "pytest", "-m", "not slow"],
            "pytest": list(graph.tests),
            "pytest_worker_memory_gb": 1.0,
        },
        {
            "id": "slow",
            "tiers": ["scheduled"],
            "resource_class": "python-tests",
            "toolchains": ["python"],
            "argv": ["python", "-m", "pytest", "-m", "slow"],
            "pytest": list(graph.tests),
            "pytest_worker_memory_gb": 1.0,
        },
    ]


def test_a_foreign_portability_cell_owns_its_tests():
    def command(identifier, cell):
        return {
            "id": identifier,
            "tiers": ["main"],
            "cell": cell,
            "resource_class": "python-tests",
            "toolchains": ["python"],
            "argv": ["python", "-m", "pytest"],
            "pytest": [f"tests/test_{identifier}.py"],
        }

    commands = [command("here", "local-cell"), command("there", "other-os-cell")]
    envelopes, others = landing_envelopes(
        commands, foreign_cells=frozenset({"other-os-cell"})
    )
    assert "tests/test_here.py" in envelopes
    assert "tests/test_there.py" not in envelopes
    assert others["tests/test_there.py"] == ("there",)


def test_foreign_commands_reach_prerequisite_resolution_with_their_exact_scope(
    landing_graph,
):
    command = {
        "id": "foreign",
        "tiers": ["main"],
        "cell": "other-os",
        "resource_class": "python-tests",
        "toolchains": ["hosted-shell"],
        "argv": ["python", "-m", "pytest", "-m", "not slow"],
        "pytest": ["tests/test_direct.py::test_specific"],
    }
    selection = graph_selection(
        landing_graph,
        ["src/pkg/hub.py"],
        [command],
        foreign_cells=frozenset({"other-os"}),
    )
    assert "tests/test_direct.py" not in selection.run
    assert "tests/test_direct.py" not in selection.deferred
    assert len(selection.foreign) == 1
    assert selection.foreign[0].targets == ("tests/test_direct.py::test_specific",)
    assert selection.foreign[0].toolchains == ("hosted-shell",)
    assert selection.foreign[0].cells == ("other-os",)
    assert selection.foreign[0].select == ("-m", "not slow")


def test_presubmit_selects_direct_readers_while_postsubmit_closes_imports(
    landing_graph,
):
    commands = _landing_commands(landing_graph)
    before = graph_selection(
        landing_graph,
        ["src/pkg/hub.py"],
        commands,
        mode=LandingMode.POSTSUBMIT,
    )
    after = graph_selection(
        landing_graph,
        ["src/pkg/hub.py"],
        commands,
        mode=LandingMode.PRESUBMIT,
    )
    assert before.selected == ("tests/test_direct.py", "tests/test_transitive.py")
    assert after.selected == ("tests/test_direct.py",)
    # Direct-input probes run in full; ordinary/slow cells together cover the
    # complete postsubmit scope rather than silently dropping slow probes.
    assert after.run["tests/test_direct.py"].select == ()
    assert before.run["tests/test_direct.py"].select == ()
    assert before.run["tests/test_direct.py"].owners == ("ordinary", "slow")


def test_presubmit_direct_readers_include_imports_inside_test_functions(
    tmp_path,
):
    # A test that imports inside its function reads the module at the first
    # hop: that edge sits on the function's node, not the module's.
    root = _repository(
        tmp_path,
        {
            "src/pkg/__init__.py": "",
            "src/pkg/hub.py": "def value():\n    return 1\n",
            "tests/test_local.py": (
                "def test_local():\n    from pkg.hub import value\n\n"
                "    assert value() == 1\n"
            ),
            "tests/test_other.py": "def test_other():\n    pass\n",
        },
    )
    graph = build_graph(RepositoryTree.from_worktree(root), cache=None)
    readers = graph.direct_readers(["src/pkg/hub.py"])
    assert readers.importers == ("tests/test_local.py",)
    assert readers.data_readers == ()


def test_presubmit_prioritizes_recent_readers_without_adding_unrelated_failures(
    landing_graph,
):
    commands = _landing_commands(landing_graph)
    selected = graph_selection(
        landing_graph,
        ["src/pkg/hub.py", "tests/test_changed.py"],
        commands,
        recent_failures=(
            "tests/test_direct.py",
            "tests/test_recent.py::test_recent",
            "tests/deleted.py",
        ),
    )
    assert selected.selected == (
        "tests/test_direct.py",
        "tests/test_changed.py",
    )
    assert list(selected.run) == list(selected.selected)


def test_graph_pytest_is_failure_first_and_maxfail_one(landing_graph):
    commands = _landing_commands(landing_graph)
    config = GateConfig(
        pytest_memory_units=PytestMemoryUnits(commands, root=landing_graph.tree.root),
        commands=tuple(commands),
        dependency_graph=lambda: landing_graph,
    )
    gates, _ = config.select(
        ["src/pkg/hub.py", "tests/test_changed.py"],
        recent_failures=("tests/test_direct.py", "tests/test_recent.py::test_recent"),
    )
    assert all("--maxfail=1" in gate.command for gate in gates)
    assert [
        argument
        for gate in gates
        for argument in gate.command
        if argument.startswith("tests/")
    ] == ["tests/test_direct.py", "tests/test_changed.py"]


def test_presubmit_graph_cohorts_are_the_lane_try_run(landing_graph):
    commands = _landing_commands(landing_graph)
    config = GateConfig(
        pytest_memory_units=PytestMemoryUnits(commands, root=landing_graph.tree.root),
        commands=tuple(commands),
        dependency_graph=lambda: landing_graph,
    )
    gates, _ = config.select(["src/pkg/hub.py", "tests/test_changed.py"])
    # The changed test and the hub's direct reader run in the lane; the
    # importer of an importer (test_transitive) is postsubmit's.
    assert all(gate.try_run and gate.precheck and not gate.static for gate in gates)
    assert sorted(
        argument
        for gate in gates
        for argument in gate.command
        if argument.startswith("tests/")
    ) == ["tests/test_changed.py", "tests/test_direct.py"]
    post, _ = config.select(
        ["src/pkg/hub.py", "tests/test_changed.py"], mode=LandingMode.POSTSUBMIT
    )
    assert post and not any(gate.precheck for gate in post)


def test_postsubmit_observes_only_the_qualified_command_cohort(
    landing_graph, monkeypatch
):
    # Use the real pytest parser/config while keeping the graph obligation tiny.
    qualified = "tests/cli/test_cli_import_collection.py"
    commands = [
        {"id": "qualified", "pytest": [qualified], "pytest_observed_inputs": True}
    ]
    config = GateConfig.load(COMMITTED_GATES)
    config.commands = tuple(commands)
    config.dependency_graph = lambda: landing_graph
    # The real rule is allowed to absorb the graph's request; recording must
    # still be attached to that retained command, never only to graph batches.
    config.rules = [
        rule for rule in config.rules if rule.name == "backend-environment-contract"
    ]
    gates, _ = config.select(
        ["src/molt/cli/backend_daemon_logs.py"], mode=LandingMode.POSTSUBMIT
    )
    observing = [gate for gate in gates if "--molt-observed-context" in gate.command]
    assert observing
    assert all(
        "--molt-observed-target=" + qualified in gate.command for gate in observing
    )
    assert all("tests.observed_inputs" in gate.command for gate in observing)
    pre, _ = config.select(["src/molt/cli/backend_daemon_logs.py"])
    assert all("--molt-observed-context" not in gate.command for gate in pre)


def test_a_changed_test_runs_slow_cases_and_unrelated_failure_adds_no_work(
    landing_graph,
):
    commands = _landing_commands(landing_graph)[:1]
    config = GateConfig(
        pytest_memory_units=PytestMemoryUnits(commands, root=landing_graph.tree.root),
        commands=tuple(commands),
        dependency_graph=lambda: landing_graph,
    )
    changed = config.graph_selection(["tests/test_changed.py"])
    assert changed is not None
    assert changed.run["tests/test_changed.py"].select == ()
    recent = config.graph_selection([], recent_failures=("tests/test_recent.py",))
    assert recent is not None
    assert recent.selected == ()
    assert recent.run == {}


def test_changed_slow_only_test_uses_its_declared_non_build_owner(landing_graph):
    ordinary, slow = _landing_commands(landing_graph)
    ordinary = {**ordinary, "pytest": ["tests/test_recent.py"]}
    slow = {**slow, "toolchains": ["python", "rustc"]}
    selected = graph_selection(
        landing_graph,
        ["tests/test_changed.py", "tests/test_recent.py"],
        [ordinary, slow],
    )
    assert selected.run["tests/test_changed.py"].toolchains == ("python", "rustc")
    assert selected.run["tests/test_changed.py"].owners == ("slow",)
    assert selected.run["tests/test_changed.py"].select == ()
    assert selected.run["tests/test_recent.py"].toolchains == ("python",)
    build_only = graph_selection(
        landing_graph,
        ["tests/test_changed.py"],
        [{**slow, "resource_class": "compiler-build-resource"}],
    )
    assert not build_only.run
    assert build_only.deferred == {"tests/test_changed.py": ("slow",)}


def test_first_postsubmit_without_last_green_selects_all_source(
    landing_graph, monkeypatch
):
    from types import SimpleNamespace

    from tools import proof_plan, test_dependency_graph
    from tools.driver_errors import DriverError

    policy = """
[[command]]
id = "ordinary"
family = "unit"
cell = "test-cell"
tiers = ["pre-push", "main"]
resource_class = "python-tests"
toolchains = ["python"]
argv = ["python", "-m", "pytest"]
pytest = ["tests/test_direct.py", "tests/test_transitive.py", "tests/test_changed.py", "tests/test_recent.py"]
pytest_worker_memory_gb = 1.0
"""
    calls = []

    class Git:
        def _run(self, arguments):
            calls.append(arguments)
            if arguments == ["show", "head:tools/proof_plan.toml"]:
                return SimpleNamespace(stdout=policy)
            assert arguments == [
                "ls-tree",
                "--full-tree",
                "--name-only",
                "-r",
                "-z",
                "head",
            ]
            return SimpleNamespace(stdout="\0".join(landing_graph.tree.blobs) + "\0")

    monkeypatch.setattr(
        test_dependency_graph, "cached_graph", lambda *_a, **_k: landing_graph
    )
    bound = []

    def bind(command, *, upstream_commit):
        bound.append(upstream_commit)
        return command

    monkeypatch.setattr(proof_plan, "bind_local_gate_command", bind)
    _, _, touched, gates, _ = select_landing_gates(
        Git(),
        landing_graph.tree.root,
        "head",
        None,
        extra=[],
        mode=LandingMode.POSTSUBMIT,
    )
    assert set(touched) == set(landing_graph.tree.blobs)
    assert {
        argument
        for gate in gates
        for argument in gate.command
        if argument.startswith("tests/")
    } == set(landing_graph.tests)
    assert bound and set(bound) == {"head"}
    calls.clear()
    with pytest.raises(DriverError, match="requires an upstream"):
        select_landing_gates(Git(), landing_graph.tree.root, "head", None, extra=[])
    assert calls == []


# --------------------------------------------------------------------------
# The committed repository: the four selections hand rules missed
# --------------------------------------------------------------------------


def _landing_tests(paths, *, mode=LandingMode.PRESUBMIT):
    cfg = GateConfig.load(COMMITTED_GATES)
    gates, _ = cfg.select(paths, mode=mode)
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
    # Never missed: an importer runs in presubmit, a data reader in postsubmit
    # (owner decision 2026-11-06, docs/design/landing_queue.md 9.10).
    selection = GateConfig.load(COMMITTED_GATES).graph_selection([changed])
    left = {target.partition("::")[0] for target in selection.postsubmit_only}
    assert test in _landing_tests([changed]) | left


def test_postsubmit_frontend_edits_select_the_frontend_lowering_cell():
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
    assert readers <= _landing_tests(changed, mode=LandingMode.POSTSUBMIT)


def test_committed_selection_reports_deferred_build_cells():
    cfg = GateConfig.load(COMMITTED_GATES)
    selection = cfg.graph_selection(
        ["src/molt/cli/build_command.py"], mode=LandingMode.POSTSUBMIT
    )
    assert selection is not None
    # A build-owned test is selected and named with its cell, not dropped.
    assert selection.deferred
    assert set(selection.deferred) <= set(selection.selected)
    assert set(selection.run).isdisjoint(selection.deferred)
    assert (
        graph_selection(
            cfg.dependency_graph(),
            ["src/molt/cli/build_command.py"],
            cfg.commands,
            mode=LandingMode.POSTSUBMIT,
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
    # the rules select on their own. A rule's structured pytest cohort may
    # split in presubmit (its direct readers join the try run, HF-701), so
    # cohorts are held to target coverage, and every other gate to its exact
    # command.
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
            if not (gate.pytest and isinstance(gate.command, tuple))
            and gate.command not in commands
        )
        full = _pytest_requests(after)
        dropped.extend(
            (path, selector, target)
            for selector, target in _pytest_requests(before)
            if not _covers(full, selector, target)
        )
    assert dropped == []


def test_presubmit_skips_every_slow_case_and_postsubmit_runs_them(
    tmp_path,
):
    """Presubmit runs only fast cases, even of a test file the change edits;
    postsubmit keeps every slow case (docs/design/landing_queue.md, 9.10)."""
    root = _repository(
        tmp_path,
        {
            "src/pkg/__init__.py": "",
            "src/pkg/hub.py": "def value():\n    return 1\n",
            "src/pkg/table.toml": "value = 1\n",
            "tests/test_imports.py": (
                "from pkg.hub import value\n\n"
                "def test_imports():\n    assert value() == 1\n"
            ),
            "tests/test_reads.py": (
                "from pathlib import Path\n\n"
                "TABLE = Path('src/pkg/table.toml')\n\n"
                "def test_reads():\n    assert TABLE.read_text()\n"
            ),
            "tests/test_changed.py": "def test_changed():\n    pass\n",
        },
    )
    graph = build_graph(RepositoryTree.from_worktree(root), cache=None)
    commands = _landing_commands(graph)
    changed = ["src/pkg/hub.py", "src/pkg/table.toml", "tests/test_changed.py"]
    selection = graph_selection(graph, changed, commands)
    # Presubmit runs importers and changed tests; the data reader is left to
    # postsubmit (owner decision 2026-11-06).
    assert {test: run.skip_slow for test, run in selection.run.items()} == {
        "tests/test_imports.py": True,
        "tests/test_changed.py": True,
    }
    assert selection.postsubmit_only == ("tests/test_reads.py",)
    post = graph_selection(graph, changed, commands, mode=LandingMode.POSTSUBMIT)
    assert post.run and not any(run.skip_slow for run in post.run.values())
    # The cohort argv carries the decision; every reader's cohort skips.
    config = GateConfig(
        pytest_memory_units=PytestMemoryUnits(commands, root=graph.tree.root),
        commands=tuple(commands),
        dependency_graph=lambda: graph,
    )
    gates, _ = config.select(changed)
    skipping = {
        argument
        for gate in gates
        if "--skip-slow" in gate.command
        for argument in gate.command
        if argument.startswith("tests/")
    }
    assert skipping == {"tests/test_imports.py", "tests/test_changed.py"}
    post_gates, _ = config.select(changed, mode=LandingMode.POSTSUBMIT)
    assert not any("--skip-slow" in gate.command for gate in post_gates)


@pytest.mark.parametrize("skip_slow", [False, True])
def test_skip_slow_skips_only_slow_cases_and_never_empties_the_session(
    tmp_path, skip_slow
):
    """tests/conftest.py skips `slow` cases under --skip-slow. A session whose
    cases are all slow still exits 0, where -m "not slow" would exit 5."""
    project = tmp_path / "skip-slow-project"
    project.mkdir()
    (project / "pytest.ini").write_text(
        "[pytest]\naddopts =\nmarkers =\n    slow: slow\n"
        "    flaky(finding_id): known random flake\n",
        encoding="utf-8",
    )
    (project / "conftest.py").write_text(
        "from tests.conftest import pytest_addoption, pytest_collection_modifyitems\n",
        encoding="utf-8",
    )
    executed = project / "executed"
    (project / "test_only_slow.py").write_text(
        "import pytest\nfrom pathlib import Path\n"
        "@pytest.mark.slow\n"
        "def test_slow():\n"
        f"    Path({str(executed)!r}).touch()\n",
        encoding="utf-8",
    )
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-n",
            "0",
            "-p",
            "no:cacheprovider",
            *(["--skip-slow"] if skip_slow else []),
            str(project / "test_only_slow.py"),
        ],
        cwd=project,
        env={
            **os.environ,
            "PYTHONPATH": os.pathsep.join((str(REPO_ROOT), str(REPO_ROOT / "src"))),
        },
        capture_output=True,
        text=True,
        timeout=PYTHON_IDENTITY_PROBE_TIMEOUT_S,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert executed.exists() is not skip_slow
    assert ("1 skipped" if skip_slow else "1 passed") in result.stdout


def test_left_to_postsubmit_names_only_what_no_selected_gate_runs():
    # The receipt never lists a data reader that a selected gate (a curated
    # rule cohort) runs in presubmit, by file or by node.
    from tools.molt_dev_policy import SelectedGate, left_to_postsubmit
    from tools.test_dependency_graph import GraphSelection

    selection = GraphSelection(
        selected=("tests/test_imports.py",),
        run={},
        deferred={},
        postsubmit_only=(
            "tests/test_reads.py",
            "tests/test_rule.py::test_node",
            "tests/test_whole.py::test_inside",
        ),
    )
    gates = [
        SelectedGate(
            command=("python", "-m", "pytest", "tests/test_rule.py::test_node"),
            toolchains=("python",),
            pytest=True,
        ),
        SelectedGate(
            command=("python", "-m", "pytest", "tests/test_whole.py"),
            toolchains=("python",),
            pytest=True,
        ),
        SelectedGate(command="python tools/generators.py check", toolchains=()),
    ]
    assert left_to_postsubmit(selection, gates) == ("tests/test_reads.py",)
    assert left_to_postsubmit(None, gates) == ()


def test_pytest_shards_bound_files_per_gate_and_keep_a_files_nodes_together():
    # A 300-file cohort ran as one gate (8.6 GiB, an indeterminate closure);
    # shards bound each gate and keep every node of a file in one shard.
    from tools.molt_dev_policy import PYTEST_SHARD_FILES, pytest_shards

    files = [
        f"tests/test_{index:03d}.py" for index in range(PYTEST_SHARD_FILES * 2 + 5)
    ]
    targets = [*files[: PYTEST_SHARD_FILES - 1]]
    targets += [
        f"{files[PYTEST_SHARD_FILES - 1]}::test_a",
        f"{files[PYTEST_SHARD_FILES - 1]}::test_b",
    ]
    targets += files[PYTEST_SHARD_FILES:]
    shards = pytest_shards(targets)
    assert [target for shard in shards for target in shard] == targets
    assert all(
        len({target.partition("::")[0] for target in shard}) <= PYTEST_SHARD_FILES
        for shard in shards
    )
    assert len(shards) == 3
    node_file = files[PYTEST_SHARD_FILES - 1]
    assert sum(any(t.startswith(node_file) for t in shard) for shard in shards) == 1
    assert pytest_shards([]) == []
