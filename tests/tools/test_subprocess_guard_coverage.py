from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import sys
import warnings
import weakref

import pytest

from tests.process_guard_common import (
    current_thread_only,
    install_thread_module_view,
    run_guarded_test_process,
)
from tools import check_subprocess_guard_coverage
from tools import scan_records


REPO_ROOT = Path(__file__).resolve().parents[2]
AUDIT_TOOL = REPO_ROOT / "tools" / "check_subprocess_guard_coverage.py"


@pytest.mark.parametrize("newline", ["\n", "\r", "\r\n"])
@pytest.mark.parametrize("trailing_newline", [False, True])
def test_raw_call_segments_match_cpython(newline: str, trailing_newline: bool) -> None:
    from collections import Counter

    source = newline.join(
        [
            "import subprocess as sp",
            "label = 'λ🧪\v\f\x85\u2028'; sp.run(['echo', label])",
            "sp.run(",
            "    sp.check_output(['echo', '嵐']),",
            "    shell=True,",
            ")",
            "sp.run('pkill -f molt-backend', shell=True)",
        ]
    ) + (newline if trailing_newline else "")
    expected = Counter(
        (
            node.lineno,
            node.func.attr,
            " ".join(ast.get_source_segment(source, node).strip().split()),
        )
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
    )
    calls = check_subprocess_guard_coverage.scan_source(
        source.encode(), "tools/physical_lines.py"
    )
    assert (
        Counter(
            (call.line, call.method, call.source)
            for call in calls
            if call.method in check_subprocess_guard_coverage.SUBPROCESS_METHODS
        )
        == expected
    )
    # These three policy findings describe one actual call expression.
    assert [(call.method, call.source) for call in calls if call.line == 7] == [
        (method, "sp.run('pkill -f molt-backend', shell=True)")
        for method in ("run", "shell.exec", "shell.kill")
    ]


@pytest.mark.parametrize("position", ["end_lineno", "end_col_offset", "col_offset"])
def test_missing_call_positions_keep_each_method_fallback(position: str) -> None:
    source = "import subprocess as sp\nsp.run('pkill -f molt-backend', shell=True)"
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            if position == "col_offset":
                delattr(node, position)
            else:
                setattr(node, position, None)
    visitor = check_subprocess_guard_coverage._SubprocessVisitor(
        path="tools/missing_positions.py", source_text=source
    )
    try:
        visitor.visit(tree)
        assert [(call.method, call.source) for call in visitor.calls] == [
            ("run", "run(...)"),
            ("shell.exec", "shell.exec(...)"),
            ("shell.kill", "shell.kill(...)"),
        ]
    finally:
        visitor._visit_handlers.clear()


def test_dispatch_reuse_preserves_raw_scope_and_diagnostic_order():
    source = "\n".join(
        [
            "import subprocess as sp",
            "from subprocess import run as launch",
            "USE_SHELL = True",
            "class Host:",
            "    def member(self, launch):",
            "        launch([])",
            "        sp.run(sp.call([]), shell=USE_SHELL)",
            "async def outer(pos, /, *args, launch=None, **kwargs):",
            "    launch([])",
            "    operation = sp.run",
            "    alias: object = operation",
            "    alias('pkill -f molt-backend', shell=True)",
            "    operation += 1",
            "    operation([])",
            "    a, b = sp.run, sp.run",
            "    a([])",
            "    return sp.check_output([])",
            "",
            "@sp.run([])",
            "def decorated(arg=sp.call([])) -> sp.check_output([]):",
            "    sp.check_call([])",
            "",
        ]
    )
    calls = check_subprocess_guard_coverage.scan_source(
        source.encode(), "tools/dispatch.py"
    )
    assert [(call.line, call.qualname, call.method, call.source) for call in calls] == [
        (7, "Host.member", "run", "sp.run(sp.call([]), shell=USE_SHELL)"),
        (7, "Host.member", "shell.exec", "sp.run(sp.call([]), shell=USE_SHELL)"),
        (7, "Host.member", "call", "sp.call([])"),
        (12, "outer", "run", "alias('pkill -f molt-backend', shell=True)"),
        (12, "outer", "shell.exec", "alias('pkill -f molt-backend', shell=True)"),
        (12, "outer", "shell.kill", "alias('pkill -f molt-backend', shell=True)"),
        (17, "outer", "check_output", "sp.check_output([])"),
        # Keep the existing generic FunctionDef field order, including its
        # current qualification of defaults, decorators and return annotations.
        (20, "decorated", "call", "sp.call([])"),
        (21, "decorated", "check_call", "sp.check_call([])"),
        (19, "decorated", "run", "sp.run([])"),
        (20, "decorated", "check_output", "sp.check_output([])"),
    ]
    assert {call.path for call in calls} == {"tools/dispatch.py"}


def test_dispatch_resolves_each_node_type_once_for_each_visitor(monkeypatch):
    module = check_subprocess_guard_coverage
    observed = []

    class ObservingVisitor(module._SubprocessVisitor):
        def _resolve_visit_handler(self, node_type):
            observed.append((self.path, node_type.__name__))
            return super()._resolve_visit_handler(node_type)

    monkeypatch.setattr(
        module,
        "_SubprocessVisitor",
        current_thread_only(ObservingVisitor, module._SubprocessVisitor),
    )
    source = b"import subprocess\nsubprocess.run([1])\nsubprocess.run([2])\n"
    expected_types = {
        "Module",
        "Import",
        "alias",
        "Expr",
        "Call",
        "Attribute",
        "Name",
        "Load",
        "List",
        "Constant",
    }
    for path in ("tools/first.py", "tools/second.py"):
        calls = module.scan_source(source, path)
        assert [(call.path, call.line, call.method) for call in calls] == [
            (path, 2, "run"),
            (path, 3, "run"),
        ]
        names = [name for selected, name in observed if selected == path]
        assert set(names) == expected_types
        assert len(names) == len(expected_types)


def test_dispatch_preserves_inherited_handlers_and_generic_node_traversal(monkeypatch):
    module = check_subprocess_guard_coverage
    constants = []

    class ConstantVisitor(module._SubprocessVisitor):
        def visit_Constant(self, node):
            constants.append(node.value)
            self.generic_visit(node)

    class InheritingVisitor(ConstantVisitor):
        pass

    monkeypatch.setattr(
        module,
        "_SubprocessVisitor",
        current_thread_only(InheritingVisitor, module._SubprocessVisitor),
    )
    calls = module.scan_source(
        b"import subprocess\nmatch 1:\n    case _:\n        subprocess.run([2, 3])\n",
        "tools/match.py",
    )
    assert constants == [1, 2, 3]
    assert [(call.line, call.method) for call in calls] == [(4, "run")]


@pytest.mark.parametrize("fail", [False, True])
def test_dispatch_releases_bound_handlers_on_success_and_error(monkeypatch, fail):
    module = check_subprocess_guard_coverage
    references = []
    retained_handler_maps = []

    class VisitorFailure(Exception):
        pass

    class TrackingVisitor(module._SubprocessVisitor):
        def __init__(self, **kwargs):
            super().__init__(**kwargs)
            references.append(weakref.ref(self))
            # Keeping the map itself alive makes an uncleared bound method a
            # real root: cyclic GC cannot hide a missing finally cleanup.
            retained_handler_maps.append(self._visit_handlers)

        def visit_Call(self, node):
            if fail:
                raise VisitorFailure("handler stopped")
            super().visit_Call(node)

    monkeypatch.setattr(
        module,
        "_SubprocessVisitor",
        current_thread_only(TrackingVisitor, module._SubprocessVisitor),
    )

    def scan():
        try:
            result = module.scan_source(
                b"import subprocess\nsubprocess.run([])\n", "tools/lifetime.py"
            )
        except VisitorFailure as error:
            assert fail
            assert str(error) == "handler stopped"
        else:
            assert not fail
            assert [(call.line, call.method) for call in result] == [(2, "run")]
        # Leave this frame before observing release; an active exception
        # traceback may legitimately retain the visitor until it is handled.

    scan()
    assert retained_handler_maps == [{}]
    assert len(references) == 1
    assert references[0]() is None


@pytest.mark.parametrize("prefix", [b"", b"\xef\xbb\xbf"])
def test_shared_observation_keeps_raw_alias_bom_and_newline_semantics(prefix):
    data = prefix + (
        b"import subprocess as child\r\n"
        b"def launch():\r\n"
        b"    child.run(['echo', 'fixture'])\r\n"
    )
    observation = check_subprocess_guard_coverage.PythonSourceObservation(
        data, "tools/fixture.py"
    )
    calls = check_subprocess_guard_coverage.scan_observation(
        observation, "tools/fixture.py"
    )
    assert [(call.path, call.line, call.qualname, call.method) for call in calls] == [
        ("tools/fixture.py", 3, "launch", "run")
    ]
    assert calls[0].source == "child.run(['echo', 'fixture'])"
    if prefix:
        # The direct-test UTF-8 parser previously rejected a BOM even though
        # the raw scanner explicitly accepts UTF-8-sig. Sharing cannot erase it.
        with pytest.raises(SyntaxError):
            observation.tree()
    else:
        assert isinstance(observation.tree(), ast.Module)


def test_observation_reuses_only_its_own_bytes_and_reports_parse_errors():
    first = check_subprocess_guard_coverage.PythonSourceObservation(
        b"import subprocess\nsubprocess.run(['echo'])\n", "tools/item.py"
    )
    assert (
        len(check_subprocess_guard_coverage.scan_observation(first, "tools/item.py"))
        == 1
    )
    changed = check_subprocess_guard_coverage.PythonSourceObservation(
        b"pass\n", "tools/item.py"
    )
    assert (
        check_subprocess_guard_coverage.scan_observation(changed, "tools/item.py") == ()
    )
    broken = check_subprocess_guard_coverage.PythonSourceObservation(
        b"def broken(:\n", "tools/item.py"
    )
    with pytest.raises(SyntaxError):
        check_subprocess_guard_coverage.scan_observation(broken, "tools/item.py")
    with pytest.raises(SyntaxError):
        broken.tree()


@pytest.mark.parametrize("action", ["error", "default", "ignore"])
@pytest.mark.parametrize("raw_first", [False, True])
def test_warning_bearing_observation_preserves_parser_policy_and_order(
    action, raw_first
):
    # Invalid escapes warn during AST parsing on supported Python 3.12+.
    # Keep the literal source byte independent of the observation/parser code.
    source = b"import subprocess\n" + rb"value = '\q'" + b"\nsubprocess.run(['echo'])\n"
    filename = "tests/warning_policy.py"
    observation = check_subprocess_guard_coverage.PythonSourceObservation(
        source, filename
    )

    def raw():
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter(action, SyntaxWarning)
            calls = check_subprocess_guard_coverage.scan_observation(
                observation, filename
            )
        assert caught == []
        assert [(call.path, call.line, call.method) for call in calls] == [
            (filename, 3, "run")
        ]

    def direct():
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter(action, SyntaxWarning)
            if action == "error":
                with pytest.raises(SyntaxError, match="invalid escape sequence") as exc:
                    observation.tree()
                assert exc.type is SyntaxError
                assert exc.value.filename == filename
                assert exc.value.lineno == 2
            else:
                assert isinstance(observation.tree(), ast.Module)
        if action == "default":
            assert len(caught) == 1
            assert caught[0].category is SyntaxWarning
            assert "invalid escape sequence" in str(caught[0].message)
            assert caught[0].filename == filename
            assert caught[0].lineno == 2
        else:
            assert caught == []

    if raw_first:
        raw()
        direct()
    else:
        direct()
        raw()


def test_warning_bearing_observation_rechecks_policy_and_filename():
    observation = check_subprocess_guard_coverage.PythonSourceObservation(
        rb"value = '\q'" + b"\n", "tests/original.py"
    )
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("ignore", SyntaxWarning)
        assert isinstance(observation.tree(), ast.Module)
        warnings.simplefilter("error", SyntaxWarning)
        with pytest.raises(SyntaxError, match="invalid escape sequence") as exc:
            observation.tree()
        assert exc.value.filename == "tests/original.py"
        assert exc.value.lineno == 1
        with pytest.raises(SyntaxError, match="invalid escape sequence") as renamed:
            observation.tree(filename="tests/renamed.py")
        assert renamed.value.filename == "tests/renamed.py"
        warnings.simplefilter("default", SyntaxWarning)
        assert isinstance(observation.tree(), ast.Module)
    assert len(caught) == 1
    assert caught[0].category is SyntaxWarning
    assert caught[0].filename == "tests/original.py"
    assert caught[0].lineno == 1


def test_warning_free_observation_shares_parse_across_policy_changes(monkeypatch):
    observation = check_subprocess_guard_coverage.PythonSourceObservation(
        b"value = 1\n", "tests/ordinary.py"
    )
    parses = []
    parse = ast.parse

    def observe(source, filename="<unknown>", *args, **kwargs):
        parses.append(filename)
        return parse(source, filename, *args, **kwargs)

    install_thread_module_view(
        monkeypatch, "ast", ast, check_subprocess_guard_coverage, parse=observe
    )
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("ignore", SyntaxWarning)
        original = observation.tree()
        warnings.simplefilter("error", SyntaxWarning)
        assert observation.tree() is original
        assert (
            check_subprocess_guard_coverage.scan_observation(
                observation, "tests/ordinary.py"
            )
            == ()
        )
        warnings.simplefilter("default", SyntaxWarning)
        assert observation.tree() is original
    assert caught == []
    assert parses == ["tests/ordinary.py"]


def test_default_inventory_includes_host_process_boundaries() -> None:
    module = check_subprocess_guard_coverage
    assert REPO_ROOT / "src" / "molt" / "repl.py" in module.DEFAULT_TARGETS
    assert (
        REPO_ROOT / "src" / "molt" / "toolchain_identity.py" in module.DEFAULT_TARGETS
    )
    assert REPO_ROOT / "src" / "molt_accel" in module.DEFAULT_TARGETS
    assert REPO_ROOT / "packaging" in module.DEFAULT_TARGETS


def test_copied_default_targets_replay_the_computed_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = check_subprocess_guard_coverage
    root = tmp_path / "checkout"
    for path in module.default_targets(root):
        if path.suffix == ".py":
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("pass\n", encoding="utf-8")
        else:
            path.mkdir(parents=True, exist_ok=True)
    source = root / "tools" / "allowed.py"
    source.write_text(
        "import subprocess as sp\ndef execute():\n    sp.run([])\n", encoding="utf-8"
    )
    (root / "packaging" / "child.py").write_text(
        "import subprocess\nsubprocess.call([])\n", encoding="utf-8"
    )
    (root / "tools" / "command.sh").write_text(
        "pkill -f fixture-only\n", encoding="utf-8"
    )
    monkeypatch.setattr(
        module,
        "ALLOWLIST",
        (
            module.AllowedRawSubprocessUse(
                "tools/allowed.py", "execute", "run", "fixture Python call"
            ),
            module.AllowedRawSubprocessUse(
                "packaging/child.py", "<module>", "call", "fixture packaging call"
            ),
            module.AllowedRawSubprocessUse(
                "tools/command.sh", "<text>", "shell.kill", "fixture text only"
            ),
        ),
    )

    def records() -> scan_records.ScanRecords:
        return scan_records.ScanRecords(
            "check_subprocess_guard_coverage", owner=AUDIT_TOOL, cache_root=tmp_path
        )

    cold = records()
    default_audit = module.audit_paths(root=root, records=cold)
    assert default_audit.ok
    assert [(call.path, call.method) for call in default_audit.raw_calls] == [
        ("tools/allowed.py", "run"),
        ("packaging/child.py", "call"),
        ("tools/command.sh", "shell.kill"),
    ]
    assert cold.misses == default_audit.scanned_files - 1
    assert cold.publish() is True
    warm = records()
    copied_audit = module.audit_paths(
        tuple(module.default_targets(root)), root=root, records=warm
    )
    assert (warm.hits, warm.misses) == (cold.misses, 0)
    assert copied_audit == default_audit

    # A changed file must be re-examined even when every other result replays.
    source.write_text(source.read_text() + "    sp.call([])\n", encoding="utf-8")
    changed = records()
    rejected = module.audit_paths(root=root, records=changed)
    assert not rejected.ok
    assert [(call.path, call.method) for call in rejected.unexpected] == [
        ("tools/allowed.py", "call")
    ]
    assert (changed.hits, changed.misses) == (cold.misses - 1, 1)


@pytest.fixture
def cli_root(tmp_path: Path) -> Path:
    # The actual script keeps its real allowance policy and audits this isolated
    # checkout through --root. Fixture subprocess expressions are only parsed.
    (tmp_path / "tools").mkdir()
    return tmp_path


def _run_audit_cli(root: Path, *args: str):
    return run_guarded_test_process(
        [sys.executable, "-I", "-B", str(AUDIT_TOOL), "--root", str(root), *args],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=60,
    )


@pytest.mark.parametrize("json_output", [False, True])
def test_cli_selected_file_uses_only_its_allowance(
    cli_root: Path, json_output: bool
) -> None:
    source = cli_root / "tools" / "secret_guard.py"
    source.write_text(
        "import subprocess\ndef _run(cmd):\n    return subprocess.run(cmd)\n",
        encoding="utf-8",
    )
    args = ["tools/secret_guard.py", "--json"] if json_output else [str(source)]

    result = _run_audit_cli(cli_root, *args)

    assert result.returncode == 0, result.stdout + result.stderr
    if json_output:
        report = json.loads(result.stdout)
        assert report["ok"] is True
        assert report["scanned_files"] == 1
        assert report["raw_call_count"] == 1
        assert report["allowlist_entries"] == 1
        assert report["stale_allowlist"] == []
    else:
        assert result.stdout == (
            "OK: subprocess guard coverage audit passed "
            "(scanned_files=1, raw_calls=1, allowlist_entries=1)\n"
        )


def test_cli_selected_directory_scans_python_and_text_once(cli_root: Path) -> None:
    directory = cli_root / "packaging"
    directory.mkdir()
    (directory / "bootstrap.py").write_text(
        "import subprocess\n"
        "def _prepare_environment():\n"
        "    subprocess.run([])\n"
        "    subprocess.run([])\n"
        "def main():\n"
        "    subprocess.call([])\n",
        encoding="utf-8",
    )
    (directory / "clean.sh").write_text("echo 'ready'\n", encoding="utf-8")

    result = _run_audit_cli(
        cli_root, "packaging", "packaging/bootstrap.py", "packaging/clean.sh", "--json"
    )

    assert result.returncode == 0, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["scanned_files"] == 2
    assert report["raw_call_count"] == 3
    assert report["allowlist_entries"] == 2


@pytest.mark.parametrize("directory_scope", [False, True])
@pytest.mark.parametrize("file_exists", [False, True])
def test_cli_selected_stale_allowances_survive_removed_calls_and_files(
    cli_root: Path, directory_scope: bool, file_exists: bool
) -> None:
    relative = "packaging/bootstrap.py" if directory_scope else "tools/secret_guard.py"
    source = cli_root / relative
    source.parent.mkdir(exist_ok=True)
    if file_exists:
        source.write_text("def clean():\n    return 1\n", encoding="utf-8")
    selection = "packaging" if directory_scope else relative

    result = _run_audit_cli(cli_root, selection, "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    expected_count = 2 if directory_scope else 1
    assert report["allowlist_entries"] == expected_count
    assert report["scanned_files"] == int(file_exists)
    assert len(report["stale_allowlist"]) == expected_count
    assert {entry["path"] for entry in report["stale_allowlist"]} == {relative}
    assert report["unexpected"] == []
    assert report["expanded_allowlist"] == []


@pytest.mark.parametrize("expanded", [False, True])
def test_cli_selected_file_keeps_unexpected_and_expanded_failures(
    cli_root: Path, expanded: bool
) -> None:
    extra = (
        "    subprocess.run([])\n"
        if expanded
        else "def unapproved():\n    subprocess.run([])\n"
    )
    (cli_root / "tools" / "secret_guard.py").write_text(
        "import subprocess\ndef _run(cmd):\n    subprocess.run(cmd)\n" + extra,
        encoding="utf-8",
    )

    result = _run_audit_cli(cli_root, "tools/secret_guard.py", "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["allowlist_entries"] == 1
    assert report["stale_allowlist"] == []
    if expanded:
        assert report["unexpected"] == []
        assert len(report["expanded_allowlist"]) == 1
        assert report["expanded_allowlist"][0]["actual_count"] == 2
    else:
        assert report["expanded_allowlist"] == []
        assert [call["qualname"] for call in report["unexpected"]] == ["unapproved"]


def test_cli_selected_text_file_uses_existing_shell_audit(cli_root: Path) -> None:
    (cli_root / "cleanup.sh").write_text("pkill -f worker\n", encoding="utf-8")

    result = _run_audit_cli(cli_root, "cleanup.sh", "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["scanned_files"] == 1
    assert report["allowlist_entries"] == 0
    assert [call["method"] for call in report["unexpected"]] == ["shell.kill"]


def test_cli_replays_records_and_still_fails_a_planted_call(
    cli_root: Path, tmp_path_factory: pytest.TempPathFactory
) -> None:
    cache = tmp_path_factory.mktemp("scan-records")
    env = {**os.environ, "MOLT_CACHE": str(cache)}
    source = cli_root / "tools" / "secret_guard.py"
    source.write_text(
        "import subprocess\ndef _run(cmd):\n    return subprocess.run(cmd)\n",
        encoding="utf-8",
    )

    def audit() -> dict[str, object]:
        result = run_guarded_test_process(
            [sys.executable, "-I", "-B", str(AUDIT_TOOL), "--root", str(cli_root)]
            + ["tools/secret_guard.py", "--json"],
            cwd=cli_root,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=60,
        )
        return json.loads(result.stdout)

    assert audit()["ok"] is True
    published = cache / "repository-scan-records" / "check_subprocess_guard_coverage"
    assert len(list(published.iterdir())) == 1
    assert audit()["ok"] is True  # replayed

    source.write_text(
        source.read_text(encoding="utf-8")
        + "def unapproved():\n    subprocess.run([])\n",
        encoding="utf-8",
    )
    for _ in range(2):  # computed, then replayed
        report = audit()
        assert report["ok"] is False
        assert [call["qualname"] for call in report["unexpected"]] == ["unapproved"]


def test_cli_default_scope_retains_global_stale_checks(cli_root: Path) -> None:
    result = _run_audit_cli(cli_root, "--json")

    assert result.returncode == 1, result.stdout + result.stderr
    report = json.loads(result.stdout)
    assert report["allowlist_entries"] == len(report["stale_allowlist"])
    assert "tools/bench.py" in {entry["path"] for entry in report["stale_allowlist"]}
    assert "packaging/bootstrap.py" in {
        entry["path"] for entry in report["stale_allowlist"]
    }


def test_explicit_custom_allowlist_is_not_filtered_by_selected_files(
    tmp_path: Path,
) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "selected.py"
    source.write_text("pass\n", encoding="utf-8")
    entry = module.AllowedRawSubprocessUse(
        "outside.py", "missing", "run", "explicit caller-owned inventory"
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=(entry,))

    assert not audit.ok
    assert audit.stale_allowlist == (entry,)
    assert audit.allowlist_entries == 1
    assert module._audit_to_dict(audit)["allowlist_entries"] == 1


def test_explicit_custom_allowlist_formats_its_actual_denominator(
    tmp_path: Path,
) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "selected.py"
    source.write_text("import subprocess\nsubprocess.run([])\n", encoding="utf-8")
    entry = module.AllowedRawSubprocessUse(
        "selected.py", "<module>", "run", "custom bounded probe"
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=(entry,))

    assert audit.ok
    assert module._format_text(audit) == (
        "OK: subprocess guard coverage audit passed "
        "(scanned_files=1, raw_calls=1, allowlist_entries=1)\n"
    )


def test_unclassified_raw_subprocess_call_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "bad.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch():\n"
        "    return subprocess.run(['python3', '-c', 'pass'])\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "bad.py"
    assert audit.unexpected[0].qualname == "launch"
    assert audit.unexpected[0].method == "run"


@pytest.mark.parametrize(
    ("expression", "expected"),
    [
        ('getattr(os, "killpg", None)', "os.killpg"),
        ('getattr(os, "kill")', "os.kill"),
        ('getattr(subprocess, "run")', "run"),
        ('getattr(os, "system")', "os.system"),
        ('getattr(proc, "terminate")', "process.terminate"),
        ("subprocess.Popen", "Popen"),
        ("proc.kill", "process.kill"),
    ],
)
def test_raw_callable_aliases_remain_audited(
    tmp_path: Path, expression: str, expected: str
) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "aliases.py"
    source.write_text(
        "import os, subprocess\n"
        "def invoke(proc):\n"
        f"    operation = {expression}\n"
        "    alias: object = operation\n"
        "    if alias is not None:\n"
        "        alias(123, 0)\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    # Merely retrieving a capability is not a call; invoking its alias is.
    assert [(call.qualname, call.method) for call in audit.raw_calls] == [
        ("invoke", expected),
        *([("invoke", "shell.exec")] if expected == "os.system" else []),
    ]


def test_callable_alias_scope_shadowing_and_immediate_getattr(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "shadowing.py"
    source.write_text(
        "import subprocess as sp\n"
        "launch = sp.run\n"
        "def shadow(launch):\n"
        "    launch([])\n"
        "def reassigned():\n"
        "    launch = lambda _: None\n"
        "    launch([])\n"
        "def real():\n"
        "    launch(['echo safe'], shell=True)\n"
        "    getattr(sp, 'run')(['true'])\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    assert [(call.qualname, call.method) for call in audit.raw_calls] == [
        ("real", "run"),
        ("real", "shell.exec"),
        ("real", "run"),
    ]


def test_direct_import_alias_and_constant_capability_name(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "import_alias.py"
    source.write_text(
        "import os as host\n"
        "from os import killpg as signal_group\n"
        "alias = signal_group\n"
        "name = 'killpg'\n"
        "def invoke():\n"
        "    alias(123, 0)\n"
        "    getattr(host, name, None)(123, 0)\n",
        encoding="utf-8",
    )
    audit = module.audit_paths([source], root=tmp_path, allowlist=())
    assert [call.method for call in audit.raw_calls] == ["os.killpg", "os.killpg"]


def test_unclassified_os_kill_call_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "bad_kill.py"
    source.write_text(
        "import os\n\ndef terminate(pid):\n    os.kill(pid, 9)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "bad_kill.py"
    assert audit.unexpected[0].qualname == "terminate"
    assert audit.unexpected[0].method == "os.kill"


def test_unclassified_process_object_signal_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "bad_process_signal.py"
    source.write_text(
        "def close(proc):\n    proc.terminate()\n    proc.kill()\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "process.terminate",
        "process.kill",
    ]


def test_non_executable_process_vocabulary_is_not_a_violation(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "documentation.py"
    source.write_text(
        '"""Never invoke taskkill or pkill -f molt-backend."""\n'
        "FORBIDDEN = ('taskkill', 'pkill -f molt-backend')\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert audit.ok
    assert audit.raw_calls == ()


def test_os_system_and_popen_execution_sinks_fail(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "bad_os_shell.py"
    source.write_text(
        "import os\n"
        "from os import popen as open_pipe\n\n"
        "def launch():\n"
        "    os.system('echo unsafe')\n"
        "    open_pipe('echo unsafe')\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "os.system",
        "shell.exec",
        "os.popen",
        "shell.exec",
    ]


def test_shell_true_constant_flow_and_kill_sink_fail(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "bad_shell.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch():\n"
        "    command = 'pkill -f molt-backend'\n"
        "    use_shell = True\n"
        "    return subprocess.run(command, shell=use_shell)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert not audit.ok
    assert [item.method for item in audit.unexpected] == [
        "run",
        "shell.exec",
        "shell.kill",
    ]


def test_inner_parameter_shadows_outer_shell_constant(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "shadowed_shell.py"
    source.write_text(
        "import subprocess\n\n"
        "USE_SHELL = True\n\n"
        "def launch(USE_SHELL):\n"
        "    return subprocess.run(['echo', 'safe'], shell=USE_SHELL)\n",
        encoding="utf-8",
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=())

    assert [item.method for item in audit.unexpected] == ["run"]


def test_unclassified_makefile_pkill_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "Makefile.pgo"
    source.write_text(
        'train:\n\tpkill -9 -f "molt-backend"\n',
        encoding="utf-8",
    )

    audit = module.audit_paths(
        [],
        root=tmp_path,
        allowlist=(),
        text_paths=[source],
    )

    assert not audit.ok
    assert len(audit.unexpected) == 1
    assert audit.unexpected[0].path == "Makefile.pgo"
    assert audit.unexpected[0].qualname == "<text>"
    assert audit.unexpected[0].method == "shell.kill"


def test_stale_allowlist_entry_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "clean.py"
    source.write_text("def ok():\n    return 1\n", encoding="utf-8")
    allowlist = (
        module.AllowedRawSubprocessUse(
            "clean.py",
            "missing",
            "run",
            "stale entry should fail",
        ),
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=allowlist)

    assert not audit.ok
    assert audit.stale_allowlist == allowlist


def test_expanded_allowlist_entry_fails(tmp_path: Path) -> None:
    module = check_subprocess_guard_coverage
    source = tmp_path / "expanded.py"
    source.write_text(
        "import subprocess\n\n"
        "def launch_twice():\n"
        "    subprocess.run(['true'])\n"
        "    subprocess.run(['true'])\n",
        encoding="utf-8",
    )
    allowlist = (
        module.AllowedRawSubprocessUse(
            "expanded.py",
            "launch_twice",
            "run",
            "one call is expected",
        ),
    )

    audit = module.audit_paths([source], root=tmp_path, allowlist=allowlist)

    assert not audit.ok
    assert len(audit.expanded_allowlist) == 1
    assert audit.expanded_allowlist[0].actual_count == 2
