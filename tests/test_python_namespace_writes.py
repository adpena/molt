"""The namespace-write summary: which syntax rebinds a module's globals (HF-226)."""

from __future__ import annotations

import ast

import pytest
from tests.process_guard_common import run_guarded_test_process

from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceSummary,
    own_namespace_literal_delete,
    decode_python_namespace_write,
    python_namespace_write_payload,
    python_namespace_summary,
)
from molt.compiler_analysis.python_source_keys import python_node_source_key


def _summary(source: str, package: str | None = "pkg.sub") -> PythonNamespaceSummary:
    return python_namespace_summary(ast.parse(source), package=package)


# Rows that are not writes of a module namespace by themselves: the graph joins
# member stores with escaped module values (call_anatomy.md 4.5a).
_ESCAPE_ROWS = frozenset(
    {
        "member_store",
        "local_definition_store",
        "computed_member_store",
        "module_escape",
        "own_module_escape",
    }
)


def _writes(source: str, package: str | None = "pkg.sub") -> set[tuple]:
    return {
        (write.module, write.name, write.construct)
        for write in _summary(source, package).foreign_writes
        if write.construct not in {"reflection", "code", *_ESCAPE_ROWS}
    }


def _escape_rows(source: str, package: str | None = "pkg.sub") -> set[tuple]:
    return {
        (write.module, write.name, write.construct)
        for write in _summary(source, package).foreign_writes
        if write.construct in _ESCAPE_ROWS
    }


def _reflections(source: str, package: str | None = "pkg.sub") -> set[str | None]:
    """The modules whose namespace mapping ``source`` reaches by identity."""
    return {
        write.module
        for write in _summary(source, package).foreign_writes
        if write.construct == "reflection"
    }


def test_admitted_namespace_syntax_qualifies_package_target_and_tree(monkeypatch):
    from molt.compiler_analysis import python_namespace_writes as syntax
    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    source = "from . import target\ndef callback():\n    target.flag = 1\n"
    tree = ast.parse(source)
    admission = _PythonAstDigestAdmission(tree)
    collected = []
    original = syntax._NamespaceWriteCollector.collect

    def collect(collector):
        collected.append(collector.tree)
        return original(collector)

    monkeypatch.setattr(syntax._NamespaceWriteCollector, "collect", collect)
    for minor in (12, 13, 14, 15):
        options = dict(target_python=(3, minor), ast_digest_admission=admission)
        first = python_namespace_summary(tree, package="left", **options)
        assert python_namespace_summary(tree, package="left", **options) is first
        second = python_namespace_summary(tree, package="right", **options)
        assert {write.module for write in first.foreign_writes} == {"left.target"}
        assert {write.module for write in second.foreign_writes} == {"right.target"}
    assert len(collected) == 8
    with pytest.raises(ValueError, match="different tree"):
        python_namespace_summary(ast.parse(source), ast_digest_admission=admission)
    assert len(collected) == 8

    # Mutable callers have no admission: their next query must see the edit.
    tree.body[1].body[0].targets[0].attr = "changed"
    fresh = python_namespace_summary(tree, package="left")
    assert {write.name for write in fresh.foreign_writes} == {"changed"}
    generation = _PythonAstDigestAdmission(tree)
    assert (
        python_namespace_summary(tree, package="left", ast_digest_admission=generation)
        == fresh
    )


@pytest.mark.parametrize("admitted", [False, True])
def test_namespace_syntax_uses_cpython_private_bindings(monkeypatch, admitted):
    import sys
    from types import ModuleType

    from molt.compiler_analysis.python_source_keys import _PythonAstDigestAdmission

    target = ModuleType("private_namespace_target")
    monkeypatch.setitem(sys.modules, target.__name__, target)
    source = (
        "import private_namespace_target as target\n"
        "class _Writer:\n"
        "    target.__attribute = 1\n"
        "    def callback(self):\n"
        "        global __own\n"
        "        __own = 2\n"
        "        target.__attribute = 3\n"
        "        setattr(target, '__literal', 4)\n"
    )
    namespace = {}
    exec(compile(source, "<private-namespace-oracle>", "exec"), namespace)
    namespace["_Writer"]().callback()
    written = set(vars(target)) - {
        "__name__",
        "__doc__",
        "__package__",
        "__loader__",
        "__spec__",
    }
    own = set(namespace) - {"__builtins__", "target", "_Writer"}
    tree = ast.parse(source)
    admission = _PythonAstDigestAdmission(tree) if admitted else None
    summary = python_namespace_summary(tree, ast_digest_admission=admission)
    assert {
        write.name
        for write in summary.foreign_writes
        if write.construct not in _ESCAPE_ROWS
    } == written
    assert summary.own_names == own


def test_foreign_writer_owners_match_real_definition_execution(monkeypatch) -> None:
    import sys
    from types import ModuleType

    target = ModuleType("namespace_owner_target")
    monkeypatch.setitem(sys.modules, target.__name__, target)
    source = (
        "import namespace_owner_target as target\n"
        "def function(default=setattr(target, 'function_default', 1)):\n"
        "    target.function_body = 1\n"
        "class Container:\n"
        "    target.class_body = 1\n"
        "    def method(self, default=setattr(target, 'method_default', 1)):\n"
        "        target.method_body = 1\n"
        "factory = lambda default=setattr(target, 'lambda_default', 1): setattr(target, 'lambda_body', 1)\n"
        "stream = (setattr(target, 'generator_body', 1) for _ in (setattr(target, 'generator_iter', 1),))\n"
        "values = [setattr(target, 'eager_comprehension', 1) for _ in (0,)]\n"
        "def outer():\n"
        "    class Nested:\n"
        "        target.nested_class_body = 1\n"
        "    def inner(default=setattr(target, 'inner_default', 1)):\n"
        "        target.inner_body = 1\n"
        "    return inner\n"
    )
    tree = ast.parse(source)
    summary = python_namespace_summary(tree, target_python=(3, 12))
    named = {
        node.name: python_node_source_key(node)
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef)
    }
    factory = next(node for node in ast.walk(tree) if isinstance(node, ast.Lambda))
    generator = next(
        node for node in ast.walk(tree) if isinstance(node, ast.GeneratorExp)
    )
    owners = {
        write.name: write.source_owner
        for write in summary.foreign_writes
        if write.construct not in _ESCAPE_ROWS
    }
    immediate = {
        "function_default",
        "class_body",
        "method_default",
        "lambda_default",
        "generator_iter",
        "eager_comprehension",
    }
    assert {name for name, owner in owners.items() if owner is None} == immediate
    assert owners["function_body"] == named["function"]
    assert owners["method_body"] == named["method"]
    assert owners["lambda_body"] == python_node_source_key(factory)
    assert owners["generator_body"] == python_node_source_key(generator)
    assert owners["nested_class_body"] == owners["inner_default"] == named["outer"]
    assert owners["inner_body"] == named["inner"]
    namespace = {}
    exec(compile(source, "<namespace-owner-oracle>", "exec"), namespace)

    def observed():
        return {name for name in owners if hasattr(target, name)}

    assert observed() == immediate
    namespace["function"]()
    namespace["Container"]().method()
    namespace["factory"]()
    next(namespace["stream"])
    inner = namespace["outer"]()
    assert observed() == set(owners) - {"inner_body"}
    inner()
    assert observed() == set(owners)


def test_foreign_write_dedup_keeps_eager_and_distinct_deferred_owners() -> None:
    tree = ast.parse(
        "import target\ntarget.flag = 1\n"
        "def first():\n    target.flag = 2\n    target.flag = 3\n"
        "def second():\n    target.flag = 4\n"
    )
    writes = python_namespace_summary(tree).foreign_writes
    assert len(writes) == 3
    assert [write.source_owner for write in writes] == [
        None,
        python_node_source_key(tree.body[2]),
        python_node_source_key(tree.body[3]),
    ]
    assert (
        tuple(
            decode_python_namespace_write(python_namespace_write_payload(write))
            for write in writes
        )
        == writes
    )


@pytest.mark.parametrize("future", [False, True])
def test_unscheduled_lazy_annotation_writers_keep_enclosing_owner(future) -> None:
    source = (
        ("from __future__ import annotations\n" if future else "") + "import target\n"
        "def function(value: setattr(target, 'annotation', 1)):\n    pass\n"
        "def outer():\n"
        "    def inner(value: setattr(target, 'nested_annotation', 1)):\n        pass\n"
    )
    tree = ast.parse(source)
    writes = [
        write
        for write in python_namespace_summary(
            tree, target_python=(3, 14)
        ).foreign_writes
        if write.construct not in _ESCAPE_ROWS
    ]
    if future:
        assert not writes
    else:
        outer = next(
            node
            for node in tree.body
            if isinstance(node, ast.FunctionDef) and node.name == "outer"
        )
        assert {write.name: write.source_owner for write in writes} == {
            "annotation": None,
            "nested_annotation": python_node_source_key(outer),
        }


@pytest.mark.parametrize(
    "owner",
    [
        [],
        "owner",
        True,
        [1, 0, 2, 0, "Module"],
        [1, 0, 2, 0, "ClassDef"],
        [1, 0, 2, 0, "Call"],
        [1, 0, 2, 0, "unknown"],
        [2, 0, 1, 0, "FunctionDef"],
    ],
)
def test_namespace_write_codec_rejects_unschedulable_or_malformed_owner(owner) -> None:
    (write,) = _summary("import target\ntarget.flag = 1\n").foreign_writes
    payload = python_namespace_write_payload(write)
    payload[4] = owner
    with pytest.raises(ValueError):
        decode_python_namespace_write(payload)


def test_namespace_write_codec_rejects_a_non_string_construct() -> None:
    with pytest.raises(ValueError, match="construct"):
        decode_python_namespace_write(
            ["sys.modules", None, [], 1, None, None, None, [], [1], []]
        )


@pytest.mark.parametrize(
    ("source", "names"),
    [
        # Deferred code rebinds what it declares global; a module-level
        # ``global`` statement binds nothing new.
        ("X = 1\ndef f():\n    global X, Y\n    X = 2\n", {"X", "Y"}),
        ("global X\nX = 1\n", set()),
        ("class C:\n    def m(self):\n        global Z\n", {"Z"}),
        # Immediate item writes affect current bindings, not future callbacks.
        ('globals().pop("_require_intrinsic", None)\n', set()),
        ('globals()["k"] = 1\n', set()),
        ('del globals()["k"]\n', set()),
        ('def f():\n    globals().setdefault("cache", {})\n', {"cache"}),
        # Reads bind nothing.
        ('x = globals().get("k")\ny = "k" in globals()\nz = globals()["k"]\n', set()),
        ("n = len(globals())\nfor name in globals():\n    pass\n", set()),
        # A function's locals() and vars() are its own frame, not the module.
        ("def f():\n    return locals(), vars()\n", set()),
        # Code run on explicit globals cannot bind this module's names.
        ("exec(code, {})\neval(text, namespace)\n", set()),
        # A function local that only reads the mapping or writes literal keys
        # bounds the reflection to those keys.
        ('def f():\n    g = globals()\n    x = g["k"]\n    y = "k" in g\n', set()),
        (
            'def f():\n    g = globals()\n    g["a"] = g["a"] + 1\n'
            '    g.pop("b", None)\n    del g["c"]\n',
            {"a", "b", "c"},
        ),
    ],
)
def test_own_names_are_the_globals_callbacks_can_rebind(
    source: str, names: set[str]
) -> None:
    assert _summary(source).own_names == frozenset(names)


@pytest.mark.parametrize(
    "operation",
    [
        "globals()['flag'] = 1",
        "globals().update(flag=1)",
        "globals().pop('flag')",
        "del globals()['flag']",
    ],
)
def test_own_mapping_writes_distinguish_initialization_from_later_callback(operation):
    for deferred in (False, True):
        source = "flag = 0\n" + (
            "def callback():\n    " + operation + "\n" if deferred else operation + "\n"
        )
        summary = _summary(source)
        namespace = {}
        exec(compile(source, "<own-writer-timeline>", "exec"), namespace)
        if deferred:
            assert namespace["flag"] == 0
            namespace["callback"]()
            assert summary.own_names == (
                None if "update" in operation else frozenset({"flag"})
            )
        else:
            assert summary.own_names == frozenset()
        if "pop" in operation or operation.startswith("del"):
            assert "flag" not in namespace
        else:
            assert namespace["flag"] == 1


def test_escaping_initial_namespace_still_grants_future_writer_custody():
    source = (
        "flag = 0\nnamespace = globals()\ndef callback():\n    namespace['flag'] = 1\n"
    )
    namespace = {}
    exec(compile(source, "<escaped-own-writer>", "exec"), namespace)
    assert namespace["flag"] == 0
    namespace["callback"]()
    assert namespace["flag"] == 1
    assert _summary(source).own_names is None


@pytest.mark.parametrize(
    ("operation", "condition"),
    [
        ("globals()['TYPE_CHECKING'] = True", "TYPE_CHECKING"),
        ("globals().__setitem__('TYPE_CHECKING', True)", "TYPE_CHECKING"),
        ("globals().update(TYPE_CHECKING=True)", "TYPE_CHECKING"),
        ("globals().update({'TYPE_CHECKING': True})", "TYPE_CHECKING"),
        (
            "def replacements(): return {'TYPE_CHECKING': True}\n"
            "globals().update(replacements())",
            "TYPE_CHECKING",
        ),
        (
            "def replacements(): return {'TYPE_CHECKING': True}\n"
            "globals().update(**replacements())",
            "TYPE_CHECKING",
        ),
        ("globals().__ior__({'TYPE_CHECKING': True})", "TYPE_CHECKING"),
        ("globals().__init__({'TYPE_CHECKING': True})", "TYPE_CHECKING"),
        (
            "del TYPE_CHECKING\nglobals().setdefault('TYPE_CHECKING', True)",
            "TYPE_CHECKING",
        ),
        (
            "TYPE_CHECKING = True\nselected = globals().pop('TYPE_CHECKING')",
            "selected",
        ),
        (
            "TYPE_CHECKING = True\nselected = globals().popitem()[1]",
            "selected",
        ),
        (
            "globals().clear()\nselected = globals().get('TYPE_CHECKING', True)",
            "selected",
        ),
        (
            "globals().__delitem__('TYPE_CHECKING')\n"
            "selected = globals().get('TYPE_CHECKING', True)",
            "selected",
        ),
        (
            "def key(): return 'TYPE_CHECKING'\nglobals()[key()] = True",
            "TYPE_CHECKING",
        ),
        (
            "def key(): return 'bridge'\nglobals()[key()] = globals()\n"
            "globals().get('bridge')['TYPE_CHECKING'] = True",
            "TYPE_CHECKING",
        ),
        (
            "def mutate_box():\n    box[0] = True\n    return None\n"
            "globals().update({'x': (box := [False])}, y=mutate_box())",
            "x[0]",
        ),
        *(
            (
                "globals()['namespace_alias'] = globals()\nselected = "
                + read
                + "\nselected['TYPE_CHECKING'] = True",
                "TYPE_CHECKING",
            )
            for read in (
                "globals()['namespace_alias']",
                "globals().get('namespace_alias')",
                "globals().__getitem__('namespace_alias')",
            )
        ),
    ],
)
def test_immediate_namespace_mutation_keeps_actual_branch_imports(
    tmp_path, operation, condition
):
    import json
    import sys
    from molt.compiler_analysis.python_import_scan import _collect_imports
    from molt.compiler_analysis.python_namespace_graph import (
        PythonGraphNamespaceIndex,
        graph_python_namespace_writes,
    )

    source = (
        "from typing import TYPE_CHECKING\n"
        + operation
        + f"\nif {condition}:\n    import namespace_mutation_leaf\n"
    )
    (tmp_path / "namespace_mutation_leaf.py").write_text(
        "VALUE = 1\n", encoding="utf-8"
    )
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\n"
            "exec(compile(sys.argv[2], '<own-mutation-oracle>', 'exec'), {})\n"
            "print(json.dumps('namespace_mutation_leaf' in sys.modules))\n",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) is True
    tree = ast.parse(source)
    index = PythonGraphNamespaceIndex()
    index.merge(
        graph_python_namespace_writes(
            python_namespace_summary(tree).foreign_writes, lambda _name: None
        )
    )
    imports = _collect_imports(
        tree,
        import_scan_mode="module_init",
        namespace_premise=index.premise(tmp_path / "entry.py", frozenset()),
    )
    assert "namespace_mutation_leaf" in imports


@pytest.mark.parametrize("method", ["setdefault", "pop"])
def test_unknown_namespace_key_preserves_actual_importer_default(method):
    import json
    import sys
    from molt.compiler_analysis.python_import_scan import _collect_imports
    from molt.compiler_analysis.python_namespace_graph import (
        PythonGraphNamespaceIndex,
        graph_python_namespace_writes,
    )
    from pathlib import Path

    source = (
        "def dynamic_key(): return 'late_loader'\n"
        f"loader = globals().{method}(dynamic_key(), __import__)\n"
        "loader('math')\n"
    )
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.modules.pop('math',None)\n"
            "exec(compile(sys.argv[1], '<namespace-importer-default>', 'exec'), {})\n"
            "print(json.dumps('math' in sys.modules))\n",
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(observed.stdout) is True
    tree = ast.parse(source)
    index = PythonGraphNamespaceIndex()
    index.merge(
        graph_python_namespace_writes(
            python_namespace_summary(tree).foreign_writes, lambda _name: None
        )
    )
    assert "math" in _collect_imports(
        tree,
        import_scan_mode="module_init",
        namespace_premise=index.premise(Path("entry.py"), frozenset()),
    )


@pytest.mark.parametrize("collision", [False, True])
def test_namespace_collision_keeps_relative_import_metadata_custody(
    tmp_path, collision
):
    import json
    import sys
    from molt.compiler_analysis.python_import_scan import _collect_imports
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
    from molt.compiler_analysis.python_namespace_graph import (
        PythonGraphNamespaceIndex,
        graph_python_namespace_writes,
    )

    for package in ("pkg", "other"):
        root = tmp_path / package
        root.mkdir()
        (root / "__init__.py").write_text("", encoding="utf-8")
        (root / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")
    source = (
        "class Collision:\n"
        "    def __hash__(self): return hash('trigger')\n"
        "    def __eq__(self, other):\n"
        "        global __package__\n        __package__ = 'other'\n        return False\n"
        # An undeclared loader slot is not a proven absent/inert value. Give
        # the negative cell an actual inert binding; the positive cell instead
        # forces a comparison against an existing custom key after reset.
        + ("globals()[Collision()] = 1\n" if collision else "trigger = False\n")
        + "__package__ = 'pkg'\nglobals().setdefault('trigger', False)\nfrom . import leaf\n"
    )
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-B",
            "-c",
            "import json,sys\nsys.path.insert(0,sys.argv[1])\n"
            "ns={'__name__':'pkg.worker','__package__':'pkg'}\n"
            "exec(compile(sys.argv[2], '<namespace-key-collision>', 'exec'), ns)\n"
            "print(json.dumps([ns['__package__'],ns['leaf'].__name__]))\n",
            str(tmp_path),
            source,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    package = "other" if collision else "pkg"
    assert json.loads(observed.stdout) == [package, package + ".leaf"]
    tree = ast.parse(source)
    index = PythonGraphNamespaceIndex()
    index.merge(
        graph_python_namespace_writes(
            python_namespace_summary(tree).foreign_writes, lambda _name: None
        )
    )
    premise = index.premise(tmp_path / "pkg" / "worker.py", frozenset())
    if collision:
        with pytest.raises(UnresolvedStaticImportError, match="runtime import custody"):
            _collect_imports(
                tree,
                module_name="pkg.worker",
                import_scan_mode="module_init",
                namespace_premise=premise,
            )
    else:
        assert "pkg.leaf" in _collect_imports(
            tree,
            module_name="pkg.worker",
            import_scan_mode="module_init",
            namespace_premise=premise,
        )


@pytest.mark.parametrize("future", [False, True])
def test_lazy_own_namespace_writer_remains_a_callback_without_a_body_job(future):
    tree = ast.parse(
        ("from __future__ import annotations\n" if future else "")
        + "def function(value: globals().pop('flag', None)):\n    pass\n"
    )
    summary = python_namespace_summary(tree, target_python=(3, 14))
    assert summary.own_names == (frozenset() if future else frozenset({"flag"}))


@pytest.mark.parametrize(
    "source",
    [
        "_apply(__name__, globals())\n",
        "namespace = globals()\n",
        "def callback():\n    globals()[name] = value\n",
        "def callback():\n    globals().update(values)\n",
        "def callback():\n    for k, v in pairs:\n        globals()[k] = v\n",
        "names = vars()\n",
        "names = locals()\n",
        "exec(code)\n",
        "def f():\n    eval(text)\n",
        # A local alias bounds nothing once a key is computed, the alias
        # escapes, a nested scope captures it or the local is rebound.
        "def f():\n    g = globals()\n    g[name] = 1\n",
        "def f():\n    g = globals()\n    helper(g)\n",
        "def f():\n    g = globals()\n    return g\n",
        "def f():\n    g = globals()\n    def inner():\n        g['a'] = 1\n",
        "def f():\n    g = globals()\n    g = {}\n    g['a'] = 1\n",
        "def f():\n    g = globals()\n    g.update(a=1)\n",
    ],
)
def test_dynamic_own_reflection_keeps_the_whole_namespace(source: str) -> None:
    summary = _summary(source)
    assert summary.own_names is None
    assert summary.own_reflection_line >= 1


def test_own_reflection_reports_its_first_site() -> None:
    summary = _summary("x = 1\nnames = vars()\nexec(code)\n")
    assert summary.own_reflection_line == 2


def test_star_import_initializes_bindings_without_a_callback_writer() -> None:
    summary = _summary("from exports import *\nhelper()\nseen = value\n")
    assert summary.own_names == frozenset()
    assert summary.own_reflection_line == 0
    assert not summary.foreign_writes


@pytest.mark.parametrize(
    ("receiver", "construct", "name"),
    [
        ("target.value = 1", "attribute", "value"),
        ("setattr(target, 'value', 1)", "setattr", "value"),
        ("vars(target)['value'] = 1", "namespace", "value"),
        ("target.__dict__['value'] = 1", "namespace", "value"),
        ("modules['target'].value = 1", "attribute", "value"),
        ("modules.get('target').value = 1", "attribute", "value"),
    ],
)
def test_star_exported_receivers_keep_foreign_writes(
    monkeypatch, receiver, construct, name
) -> None:
    import sys
    from types import ModuleType

    target = ModuleType("target")
    exports = ModuleType("namespace_write_exports")
    exports.__dict__.update(
        __all__=("target", "modules"), target=target, modules={"target": target}
    )
    monkeypatch.setitem(sys.modules, exports.__name__, exports)
    source = f"from namespace_write_exports import *\n{receiver}\n"
    exec(compile(source, "<star-foreign-write-oracle>", "exec"), {})
    assert target.value == 1
    assert (None, name, construct) in _writes(source)


# Samples of each standard import-hook object's native container kind.
_HOOK_SAMPLES = {
    "sys.modules": {"k": 0},
    "sys.meta_path": [1, 0],
    "sys.path_hooks": [1, 0],
    "sys.path_importer_cache": {"k": 0},
}


@pytest.mark.parametrize(
    "mutation",
    [
        "hooks.append(1)",
        "hooks.extend([1])",
        "hooks.insert(0, 1)",
        "hooks.remove(0)",
        "hooks.sort()",
        "hooks.clear()",
        "hooks.pop()",
        "hooks.pop('k')",
        "hooks.popitem()",
        "hooks.setdefault('n', 1)",
        "hooks.update(n=1)",
        "hooks[0] = 2",
        "hooks['k'] = 2",
        "del hooks[0]",
        "del hooks['k']",
        "hooks += [1]",
        "hooks *= 2",
        "hooks |= {'n': 1}",
        "hooks -= 1",
        "hooks.copy()",
    ],
)
def test_star_exported_hook_mutations_invalidate_the_hooks_they_can_change(
    monkeypatch, mutation
) -> None:
    import copy
    import sys
    from types import ModuleType

    # CPython is the oracle: run the mutation on each standard hook object's
    # container kind and see whether it changes, or cannot apply at all.
    source = f"from namespace_hook_exports import *\n{mutation}\n"
    changed: dict[str, bool] = {}
    supported: dict[str, bool] = {}
    for spelling, sample in _HOOK_SAMPLES.items():
        hooks = copy.copy(sample)
        exports = ModuleType("namespace_hook_exports")
        exports.__dict__.update(__all__=("hooks",), hooks=hooks)
        monkeypatch.setitem(sys.modules, exports.__name__, exports)
        supported[spelling] = True
        try:
            exec(compile(source, "<star-hook-write-oracle>", "exec"), {})
        except AttributeError:
            supported[spelling] = False
        except TypeError as error:
            supported[spelling] = "unsupported operand" not in str(error)
        except (IndexError, KeyError, ValueError):
            pass
        changed[spelling] = hooks != sample

    writes = _writes(source)
    for spelling in _HOOK_SAMPLES:
        module, _, name = spelling.rpartition(".")
        recorded = (
            any(m == "sys.modules" for m, _, _ in writes)
            if spelling == "sys.modules"
            else (module, name, "import_hooks") in writes
        )
        if changed[spelling]:
            assert recorded, f"{mutation} changes {spelling}"
        if not supported[spelling]:
            assert not recorded, f"{mutation} cannot change {spelling}"
    assert ("sys", None, "import_hooks") not in writes


def test_star_may_shadow_an_explicit_receiver_import(monkeypatch) -> None:
    import sys
    from types import ModuleType

    original = ModuleType("namespace_original")
    replacement = ModuleType("namespace_replacement")
    exports = ModuleType("namespace_shadow_exports")
    exports.__dict__.update(__all__=("target",), target=replacement)
    monkeypatch.setitem(sys.modules, original.__name__, original)
    monkeypatch.setitem(sys.modules, exports.__name__, exports)
    source = (
        "import namespace_original as target\n"
        "from namespace_shadow_exports import *\n"
        "target.value = 1\n"
    )
    exec(compile(source, "<star-shadow-write-oracle>", "exec"), {})
    assert replacement.value == 1 and not hasattr(original, "value")
    assert (None, "value", "attribute") in _writes(source)


@pytest.mark.parametrize(
    "body,changed",
    [
        ("def f(target):\n    target.value = 1\nf(Local())\n", False),
        ("(lambda target: setattr(target, 'value', 1))(Local())\n", False),
        (
            "def f():\n    target = Local()\n    target.value = 1\nf()\n",
            False,
        ),
        (
            "def outer(target):\n"
            "    def inner():\n        target.value = 1\n"
            "    inner()\nouter(Local())\n",
            False,
        ),
        (
            "def outer():\n    target = Local()\n"
            "    def inner():\n        nonlocal target\n        target.value = 1\n"
            "    inner()\nouter()\n",
            False,
        ),
        (
            "def outer(target):\n    class C:\n        global target\n"
            "        def inner(self):\n            target.value = 1\n"
            "    C().inner()\nouter(Local())\n",
            False,
        ),
        (
            "def f():\n    try:\n        raise ValueError()\n"
            "    except ValueError as target:\n        target.value = 1\nf()\n",
            False,
        ),
        (
            "def f():\n    match Local():\n        case target:\n"
            "            target.value = 1\nf()\n",
            False,
        ),
        ("[setattr(target, 'value', 1) for target in [Local()]]\n", False),
        ("list(setattr(target, 'value', 1) for target in [Local()])\n", False),
        ("def f():\n    global target\n    target.value = 1\nf()\n", True),
        ("class C:\n    target.value = 1\n", True),
        ("class C:\n    target.value = 1\n    target = Local()\n", True),
        ("def f(target=setattr(target, 'value', 1)):\n    pass\n", True),
        (
            "[None for target in (setattr(target, 'value', 1),)]\n",
            True,
        ),
    ],
)
def test_star_receiver_lookup_keeps_lexical_storage_custody(
    monkeypatch, body: str, changed: bool
) -> None:
    import sys
    from types import ModuleType, SimpleNamespace

    target = ModuleType("namespace_scope_target")
    target.value = 0
    exports = ModuleType("namespace_scope_exports")
    exports.__dict__.update(__all__=("target",), target=target)
    monkeypatch.setitem(sys.modules, exports.__name__, exports)
    source = "from namespace_scope_exports import *\n" + body
    exec(
        compile(source, "<star-lexical-storage-oracle>", "exec"),
        {"Local": SimpleNamespace},
    )
    assert target.value == int(changed)
    writes = _writes(source)
    assert bool(writes) is changed
    if changed:
        assert any(module is None and name == "value" for module, name, _ in writes)


@pytest.mark.parametrize(
    "mutation", ["hooks.append(1)", "hooks[0] = 1", "del hooks[0]", "hooks += [1]"]
)
@pytest.mark.parametrize("argument", ["local", "hooks"])
@pytest.mark.parametrize("closure", [False, True])
def test_star_hook_values_retain_mutation_custody_through_lexical_bindings(
    monkeypatch, mutation: str, argument: str, closure: bool
) -> None:
    import sys
    from types import ModuleType

    hooks = [0]
    exports = ModuleType("namespace_local_hook_exports")
    exports.__dict__.update(__all__=("hooks",), hooks=hooks)
    monkeypatch.setitem(sys.modules, exports.__name__, exports)
    body = (
        f"    def inner():\n        nonlocal hooks\n        {mutation}\n    inner()\n"
        if closure
        else f"    {mutation}\n"
    )
    source = (
        "from namespace_local_hook_exports import *\n"
        f"def mutate(hooks):\n{body}"
        f"local = [0]\nmutate({argument})\n"
    )
    namespace = {}
    exec(compile(source, "<star-local-hook-oracle>", "exec"), namespace)
    assert (namespace["local"] != [0]) is (argument == "local")
    assert (hooks != [0]) is (argument == "hooks")
    # Lexical lookup cannot rule out a carried hook object. Keep the native
    # receiver mutation conservative until its actual value is proved: every
    # list hook it could change is written.
    writes = _writes(source)
    assert ("sys", "meta_path", "import_hooks") in writes
    assert ("sys", "path_hooks", "import_hooks") in writes


def test_explicit_hook_alias_inplace_mutation_is_a_write() -> None:
    assert _writes("from sys import meta_path as hooks\nhooks += [finder]\n") == {
        ("sys", "meta_path", "import_hooks")
    }


def test_named_writes_on_module_references() -> None:
    source = (
        "import logging\n"
        "import a.b\n"
        "import c.d as cd\n"
        "from e import f\n"
        "logging.raiseExceptions = False\n"
        "a.b.value = 1\n"
        "del cd.name\n"
        "f.counter += 1\n"
        "setattr(logging, 'literal', 1)\n"
        "delattr(cd, computed)\n"
        "logging.__setattr__('dunder', 1)\n"
        "logging.__class__ = Patched\n"
    )
    assert _writes(source) == {
        ("logging", "raiseExceptions", "attribute"),
        ("a.b", "value", "attribute"),
        ("c.d", "name", "attribute"),
        ("e.f", "counter", "attribute"),
        ("logging", "literal", "setattr"),
        ("c.d", None, "setattr"),
        ("logging", "dunder", "setattr"),
        ("logging", None, "attribute"),
    }


def test_namespace_mapping_writes_and_hand_offs() -> None:
    source = (
        "import m\n"
        "vars(m)['k'] = 1\n"
        "m.__dict__.pop('popped')\n"
        "m.__dict__.update(values)\n"
        "register(vars(m))\n"
        "seen = m.__dict__.get('k')\n"
        "present = 'k' in vars(m)\n"
    )
    assert _writes(source) == {
        ("m", "k", "namespace"),
        ("m", "popped", "namespace"),
        ("m", None, "namespace"),
    }


def test_sys_modules_entries_are_modules_named_by_their_key() -> None:
    source = (
        "import sys\n"
        "sys.modules[__name__].computed = 1\n"
        "sys.modules['pkg.literal'].named = 2\n"
        "setattr(sys.modules.get(key), 'via_get', 3)\n"
        "sys.modules['not..a.module'].odd = 4\n"
    )
    assert _writes(source) == {
        (None, "computed", "attribute"),
        ("pkg.literal", "named", "attribute"),
        (None, "via_get", "setattr"),
        # A key that names no module reaches some module, not a malformed one.
        (None, "odd", "attribute"),
    }


def test_relative_references_resolve_against_the_package() -> None:
    source = "from . import sibling\nsibling.X = 1\nfrom .. import top\ntop.Y = 2\n"
    assert _writes(source, package="pkg.sub") == {
        ("pkg.sub.sibling", "X", "attribute"),
        ("pkg.top", "Y", "attribute"),
    }
    # Without a package a relative import binds no module reference.
    assert _writes(source, package=None) == set()


def test_import_hook_mutations_and_reads() -> None:
    source = (
        "import sys\n"
        "from sys import meta_path as hooks\n"
        "sys.modules[name] = module\n"
        "sys.path_hooks.insert(0, hook)\n"
        "hooks.append(finder)\n"
        "sys.path_importer_cache.clear()\n"
    )
    assert _writes(source) == {
        ("sys.modules", None, "import_binding"),
        ("sys", "path_hooks", "import_hooks"),
        ("sys", "meta_path", "import_hooks"),
        ("sys", "path_importer_cache", "import_hooks"),
    }
    reads = (
        "import sys\n"
        "loaded = 'x' in sys.modules\n"
        "entry = sys.modules.get('x')\n"
        "first = sys.modules['x']\n"
        "count = len(sys.meta_path)\n"
        "print(sys.path_hooks)\n"
        "for finder in sys.meta_path:\n"
        "    pass\n"
        "where = sys.meta_path.index(finder)\n"
    )
    assert _writes(reads) == set()


@pytest.mark.parametrize(
    "operation",
    [
        "sys.modules['cache.only'] = value",
        "del sys.modules['cache.only']",
        "sys.modules.setdefault('cache.only', value)",
        "sys.modules.pop('cache.only', None)",
        "sys.modules.__setitem__('cache.only', value)",
        "sys.modules.__delitem__('cache.only')",
        "sys.modules.update({'cache.only': value})",
        "sys.modules.update(**{'cache.only': value})",
        "sys.modules.__ior__({'cache.only': value})",
        "sys.modules.__init__({'cache.only': value})",
    ],
)
def test_import_cache_keyed_mutators_preserve_their_actual_binding(operation) -> None:
    from pathlib import Path
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    writes = _summary("import sys\n" + operation + "\n").foreign_writes
    assert len(writes) == 1
    assert (writes[0].module, writes[0].name, writes[0].construct) == (
        "sys.modules",
        "cache.only",
        "import_binding",
    )
    assert (
        decode_python_namespace_write(python_namespace_write_payload(writes[0]))
        == writes[0]
    )
    result = graph_python_namespace_writes(writes, lambda _name: Path("ordinary.py"))
    assert not result.members
    assert not result.by_source
    assert not result.any_module


def test_computed_cache_write_keeps_its_bound_and_redirects_only_admitted_rows() -> (
    None
):
    from molt.compiler_analysis.python_string_facts import PythonStringBound
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    source = "import sys\nsys.modules.setdefault(f'{__name__}.path', value)\n"
    writes = _summary(source).foreign_writes
    assert len(writes) == 1
    assert (writes[0].module, writes[0].name, writes[0].construct) == (
        "sys.modules",
        None,
        "import_binding",
    )
    assert writes[0].key_bound == PythonStringBound("", ".path", 5)
    assert (
        decode_python_namespace_write(python_namespace_write_payload(writes[0]))
        == writes[0]
    )
    result = graph_python_namespace_writes(writes, lambda _name: None)
    assert not result.members
    assert not result.by_source
    assert result.import_redirects.redirect("os.path")
    assert result.import_redirects.redirect("os.path.extra")
    assert not result.import_redirects.redirect("os")


def test_unknown_string_addition_can_redirect_outside_its_literal_prefix() -> None:
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    class Key(str):
        def __radd__(self, left: str) -> str:
            assert left == "encodings."
            return "typing"

    assert "encodings." + Key("ascii") == "typing"
    writes = _summary(
        'import sys\nsys.modules["encodings." + key] = value\n'
    ).foreign_writes
    assert len(writes) == 1
    assert writes[0].name is None and writes[0].key_bound is None
    result = graph_python_namespace_writes(writes, lambda _name: None)
    assert result.import_redirects.redirect("typing")


def test_literal_cache_write_redirects_its_row_and_descendants_only() -> None:
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    writes = _summary("import sys\ndel sys.modules['mymod']\n").foreign_writes
    result = graph_python_namespace_writes(writes, lambda _name: None)
    assert [str(w) for w in result.import_redirects.redirect("mymod.sub")] == [
        "sys.modules['mymod'] write at <module>:2"
    ]
    assert not result.import_redirects.redirect("other")
    assert not result.import_redirects.redirect("mymo")


@pytest.mark.parametrize(
    ("source", "guarded"),
    [
        ("try:\n    import fast\nexcept ImportError:\n    pass\n", ["fast"]),
        ("try:\n    from pkg import mod\nexcept Exception:\n    pass\n", ["pkg.mod"]),
        (
            "import contextlib\nwith contextlib.suppress(ImportError):\n    import fast\n",
            ["fast"],
        ),
        (
            "import importlib\ntry:\n    importlib.import_module(f'enc.{n}')\n"
            "except LookupError:\n    pass\n",
            ["enc."],
        ),
        ("try:\n    __import__(name)\nexcept ImportError:\n    pass\n", [None]),
        # A handler around a nested definition does not guard its body.
        (
            "try:\n    def f():\n        import fast\nexcept ImportError:\n    pass\n",
            [],
        ),
        # else and finally do not swallow.
        (
            "try:\n    pass\nexcept ImportError:\n    pass\nelse:\n    import fast\n",
            [],
        ),
        ("try:\n    import fast\nfinally:\n    pass\n", []),
        ("import fast\n", []),
    ],
)
def test_guarded_imports_name_what_a_handler_can_swallow(
    source: str, guarded: list[str | None]
) -> None:
    rows = [
        write
        for write in _summary(source).foreign_writes
        if write.construct == "guarded_import"
    ]
    assert [
        write.module if write.key_bound is None else write.key_bound.prefix
        for write in rows
    ] == guarded
    for write in rows:
        assert (
            decode_python_namespace_write(python_namespace_write_payload(write))
            == write
        )


_GUARDED_PWD = "    try:\n        import pwd\n    except ImportError:\n        pass\n"


@pytest.mark.parametrize(
    ("source", "initialized"),
    [
        # A function exists only after its def, which runs after the import.
        ("import sys\ndef f():\n" + _GUARDED_PWD, ("sys",)),
        # A module-level guard runs after every statement before it.
        ("import a.b\nif True:\n" + _GUARDED_PWD, ("a", "a.b")),
        ("from a.b import c\nclass C:\n  def m(self):\n" + _GUARDED_PWD, ("a", "a.b")),
        # An import after the guard's statement may not have run.
        ("def f():\n" + _GUARDED_PWD + "import sys\n", ()),
        # A conditional import may not have run.
        ("if flag:\n    import sys\ndef f():\n" + _GUARDED_PWD, ()),
        # A future statement is a compiler directive.
        ("from __future__ import annotations\ndef f():\n" + _GUARDED_PWD, ()),
    ],
)
def test_guarded_import_names_the_modules_completed_before_it(
    source: str, initialized: tuple[str, ...]
) -> None:
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    (row,) = [
        write
        for write in _summary(source).foreign_writes
        if write.construct == "guarded_import"
    ]
    assert (row.module, row.initialized) == ("pwd", initialized)
    assert decode_python_namespace_write(python_namespace_write_payload(row)) == row
    redirects = graph_python_namespace_writes(
        (row,), lambda _name: None
    ).import_redirects
    # The handler swallows a failure of pwd's own body, and of a module pwd
    # imports first, unless that module completed before the guard ran.
    assert redirects.swallowers("pwd", ())
    for dependency in ("sys", "a.b"):
        assert bool(redirects.swallowers(dependency, ("pwd",))) == (
            dependency not in initialized
        )


@pytest.mark.parametrize(
    "operation",
    [
        "sys.modules[key] = value",
        "sys.modules.clear()",
        "sys.modules.update(values)",
        "sys.modules[f'{key}'] = value",
        "publish(sys.modules)",
    ],
)
def test_unknown_import_cache_write_keeps_canonical_acquisition_guard(
    operation,
) -> None:
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_identity_catalog_generated import PythonMember

    writes = _summary("import sys\n" + operation + "\n").foreign_writes
    assert ("sys.modules", None, "import_binding") in {
        (write.module, write.name, write.construct) for write in writes
    }
    result = graph_python_namespace_writes(writes, lambda _name: None)
    assert result.members == int(PythonMember.IMPORT_HOOKS)


@pytest.mark.parametrize(
    "source",
    [
        # Object state is not a namespace.
        "self.value = 1\n",
        "import os\nos.environ['X'] = '1'\n",
        "obj.__dict__['k'] = 1\n",
        "setattr(obj, 'k', 1)\n",
        # A module handed to other code is no named write by itself; it is an
        # escape (`test_escaping_module_values_and_member_stores`).
        "import m\npatch(m)\n",
        # sys.path does not redirect canonical imports.
        "import sys\nsys.path.insert(0, root)\n",
    ],
)
def test_non_namespace_writes_are_not_summarized(source: str) -> None:
    assert _writes(source) == set()


def test_writes_are_reported_in_source_order() -> None:
    source = "import m\nm.b = 1\nm.a = 2\nm.b = 3\n"
    writes = _summary(source).foreign_writes
    assert [(write.name, write.line) for write in writes] == [("b", 2), ("a", 3)]


@pytest.mark.parametrize("frame_writer", [None, "frame.f_locals"])
def test_import_cache_key_survives_alias_only_when_frame_is_sealed(
    frame_writer: str | None,
) -> None:
    from molt.compiler_analysis.python_import_scan import _collect_imports_for_graph
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.compiler_analysis.python_string_facts import PythonStringBound
    from molt.target_python import TargetPythonVersion

    tree = ast.parse(
        "import sys as original_sys\ndef publish(suffix, value):\n"
        "    key = f'plugins.{suffix}'\n"
        "    original_sys.modules[key] = value\n"
    )
    # The receiver's global read can run arbitrary code because this import
    # may be redirected. Its key is still scoped unless that code can edit
    # the publisher's active frame.
    scan = _collect_imports_for_graph(
        tree,
        module_name="publisher",
        target_python=TargetPythonVersion(3, 13, 0),
        namespace_premise=PythonNamespacePremise(
            (), 0, 0, ("sys",), frame_locals_writer=frame_writer
        ),
    )
    assert scan.namespace_summary is not None
    (write,) = scan.namespace_summary.foreign_writes
    assert write.name is None
    assert write.key_bound == (
        PythonStringBound("plugins.", "", 8) if frame_writer is None else None
    )
    assert decode_python_namespace_write(python_namespace_write_payload(write)) == write


def test_module_escape_consumes_only_exact_borrowed_call_operands() -> None:
    from molt.compiler_analysis.python_import_scan import _collect_imports_for_graph
    from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
    from molt.target_python import TargetPythonVersion

    tree = ast.parse(
        """\
import sys
from _intrinsics import require_intrinsic as require
read = require("molt_namespace_entry_get")
read_globals = require("molt_dataclasses_init_globals")
def reader(key, flag):
    borrowed = sys.modules[key]
    if flag:
        return read(borrowed, "first", None)
    return read(borrowed, "last", None)
def fallback(key):
    return read(None, "ignored", sys.modules[key])
def mixed(key, callback, flag):
    callee = read if flag else callback
    return callee(sys.modules[key], "value", None)
def dispatches_getattribute(key):
    return read_globals(sys.modules[key])
"""
    )
    raw = python_namespace_summary(tree)
    candidates = {
        row.line: row for row in raw.foreign_writes if row.construct == "module_escape"
    }
    assert set(candidates) == {6, 11, 14, 16}
    assert len(candidates[6].borrowed_if) == 2
    assert all(
        decode_python_namespace_write(python_namespace_write_payload(row)) == row
        for row in candidates.values()
    )

    for foreign_writes, expected in [
        ((), {11, 14, 16}),
        (("read",), {6, 11, 14, 16}),
    ]:
        result = _collect_imports_for_graph(
            tree,
            module_name="probe",
            target_python=TargetPythonVersion(3, 15, 0),
            namespace_premise=PythonNamespacePremise(
                foreign_writes, 0, 0, (), opaque_activation_sources=()
            ),
        )
        assert result.namespace_summary is not None
        assert {
            row.line
            for row in result.namespace_summary.foreign_writes
            if row.construct == "module_escape"
        } == expected


def test_computed_store_uses_the_attribute_operand_at_its_source_point() -> None:
    from molt.compiler_analysis.python_import_scan import _collect_imports_for_graph
    from molt.compiler_analysis.python_namespace_graph import (
        PythonNamespacePremise,
        graph_python_namespace_writes,
    )
    from molt.compiler_analysis.python_string_facts import PythonStringBound
    from molt.target_python import TargetPythonVersion

    tree = ast.parse(
        """\
def assign(instance, suffix):
    name = f"public_{suffix}"
    setattr(instance, name, 1)
    name = "__class__"
    object.__setattr__(instance, name, 2)
"""
    )
    scan = _collect_imports_for_graph(
        tree,
        module_name="producer",
        target_python=TargetPythonVersion(3, 15, 0),
        namespace_premise=PythonNamespacePremise(
            (), 0, 0, (), opaque_activation_sources=()
        ),
    )
    assert scan.namespace_summary is not None
    first, second = scan.namespace_summary.foreign_writes
    assert first.construct == "computed_member_store"
    assert first.key_bound == PythonStringBound("public_", "", 7)
    assert second.construct == "member_store"
    assert second.name == "__class__"
    graph = graph_python_namespace_writes((first,), lambda _: None)
    assert not graph.computed_member_writes


def test_same_line_namespace_cache_writes_keep_source_site_order():
    from dataclasses import replace
    from molt.compiler_analysis.python_namespace_writes import (
        refine_namespace_writes,
    )

    tree = ast.parse(
        "import sys\nsys.modules[key] = first; sys.modules[key] = second\n"
    )
    first, second = (
        python_node_source_key(statement.targets[0]) for statement in tree.body[1:]
    )
    summary = python_namespace_summary(tree)
    assert {write.source_site for write in summary.foreign_writes} == {first, second}
    refined = refine_namespace_writes(
        summary,
        [
            (first, ("typing",), frozenset(), frozenset()),
            (second, ("typing",), frozenset(), frozenset()),
        ],
    )
    reversed_input = refine_namespace_writes(
        replace(summary, foreign_writes=tuple(reversed(summary.foreign_writes))),
        [
            (second, ("typing",), frozenset(), frozenset()),
            (first, ("typing",), frozenset(), frozenset()),
        ],
    )
    assert [write.source_site for write in refined.foreign_writes] == [first, second]
    assert reversed_input == refined


@pytest.mark.parametrize(
    ("body", "written"),
    [
        # typing.get_type_hints' shape: an alias passed to same-module readers
        # and an eval sink writes nothing.
        (
            "def look(ns, key):\n    return ns[key] if key in ns else None\n"
            "def evaluate(value, ns):\n"
            "    found = look(ns, value)\n"
            "    return found if found is not None else eval(value, ns)\n"
            "def hints(name):\n"
            "    ns = sys.modules[name].__dict__\n"
            "    return evaluate('x', ns)\n",
            False,
        ),
        # A resolved callee that item-writes its parameter writes the mapping.
        (
            "def put(ns):\n    ns['x'] = 1\n"
            "def run(name):\n    ns = sys.modules[name].__dict__\n    put(ns)\n",
            True,
        ),
        # A parameter passed on to a writing parameter is written (fixed point).
        (
            "def put(ns):\n    ns.update(x=1)\n"
            "def relay(ns):\n    put(ns)\n"
            "def run(name):\n    relay(sys.modules[name].__dict__)\n",
            True,
        ),
        # A callee this module does not define may write through its argument.
        ("def run(name):\n    other.write(sys.modules[name].__dict__)\n", True),
        # An alias that escapes any other way (returned) is a write.
        ("def run(name):\n    ns = sys.modules[name].__dict__\n    return ns\n", True),
        # A direct item write on the namespace stays a write.
        ("def run(name):\n    sys.modules[name].__dict__['x'] = 1\n", True),
    ],
)
def test_foreign_namespace_escape_follows_parameter_write_summaries(
    body: str, written: bool
) -> None:
    source = "import sys\n" + body
    namespace_rows = [
        write
        for write in _summary(source).foreign_writes
        if write.construct == "namespace"
    ]
    assert bool(namespace_rows) is written, namespace_rows


@pytest.mark.parametrize(
    "body",
    [
        # 1. A nested function captures the alias and writes through it.
        "def f(ns):\n    def g():\n        ns['sqrt'] = 1\n    g()\n"
        "def run(name):\n    f(sys.modules[name].__dict__)\n",
        # 1. A lambda captures the parameter and writes through it.
        "def f(ns):\n    return (lambda: ns.update(x=1))\n"
        "def run(name):\n    f(sys.modules[name].__dict__)\n",
        # 2. The alias is declared global: it escapes the function.
        "keep = None\n"
        "def run(name):\n    global keep\n    keep = sys.modules[name].__dict__\n",
        # 3. A decorated callee binds the decorator's result, not the body.
        "def deco(fn):\n    return lambda ns: ns.update(x=1)\n"
        "@deco\ndef reader(ns):\n    return ns.get('x')\n"
        "def run(name):\n    reader(sys.modules[name].__dict__)\n",
        # 3. A callee name the module binds twice may name the writer.
        "def reader(ns):\n    return ns.get('x')\n"
        "def writer(ns):\n    ns['x'] = 1\n"
        "if flag:\n    reader = writer\n"
        "def run(name):\n    reader(sys.modules[name].__dict__)\n",
        # 4. A module-level `eval` is not the excluded builtin.
        "def eval(source, ns):\n    ns['x'] = 1\n"
        "def run(name):\n    eval('x', sys.modules[name].__dict__)\n",
    ],
)
def test_namespace_escape_through_an_unproven_reader_stays_a_write(body: str) -> None:
    source = "import sys\n" + body
    rows = [w for w in _summary(source).foreign_writes if w.construct == "namespace"]
    assert rows, "the premise must stay poisoned"


# Read-side facts (static module cells, call_anatomy.md 4.5a): a module whose
# namespace mapping any code reaches by identity keeps its dictionary
# namespace, so every such use must be recorded, read or write. A read of a
# literal dunder key never needs a global, so it is not a reflection.


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("import m\nseen = m.__dict__.get('k')\n", {"m"}),
        ("import m\npresent = 'k' in vars(m)\n", {"m"}),
        ("import m\nfor key in m.__dict__:\n    pass\n", {"m"}),
        ("import m\nregister(vars(m))\n", {"m"}),
        ("import m\nnamespace = getattr(m, '__dict__')\n", {"m"}),
        ("import sys\nsys.modules[name].__dict__.get('k')\n", {None}),
        ("import sys\nns = sys.modules['pkg.literal'].__dict__\n", {"pkg.literal"}),
        ("def f(fn):\n    return fn.__globals__['helper']\n", {None}),
        ("def f(frame):\n    return dict(frame.f_globals)\n", {None}),
        ("def f(frame):\n    return getattr(frame, 'f_globals')\n", {None}),
        # Literal dunder keys stay in every module's dictionary.
        ("import m\nname = m.__dict__['__name__']\n", set()),
        ("def f(frame):\n    return frame.f_globals.get('__name__', 'x')\n", set()),
        ("def f(tb):\n    return '__name__' in tb.tb_frame.f_globals\n", set()),
        # A private name is a global, not a dunder: unittest's marker reads it.
        ("def f(tb):\n    return '__unittest' in tb.tb_frame.f_globals\n", {None}),
        (
            "def f(caller):\n"
            "    ns = getattr(caller, 'f_globals', None)\n"
            "    if isinstance(ns, dict):\n"
            "        return ns.get('__name__')\n",
            set(),
        ),
        # An alias that reads a non-dunder key is a reflection.
        (
            "def f(caller):\n    ns = caller.f_globals\n    return ns.get('helper')\n",
            {None},
        ),
        # An attribute that is not a namespace mapping reaches nothing.
        ("import m\nvalue = m.helper\n", set()),
        ("def f(obj):\n    return obj.__dict__['x']\n", set()),
    ],
)
def test_namespace_mapping_reads_are_reflections(
    source: str, expected: set[str | None]
) -> None:
    assert _reflections(source) == expected


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        # A namespace builtin taken as a value returns its caller's
        # namespace, so it reaches every module's mapping (HF-872).
        ("g = globals\n", {None}),
        ("def f():\n    return vars\n", {None}),
        ("import builtins\nSYMBOL = getattr(builtins, NAME)\n", {None}),
        ("import builtins\nSYMBOL = getattr(builtins, 'globals')\n", {None}),
        ("import builtins\nkeep(builtins.locals)\n", {None}),
        ("from builtins import globals as g\nkeep(g)\n", {None}),
        ("import builtins\ndef f(m):\n    return builtins.vars(m)\n", {None}),
        ("def f(callbacks=[globals]):\n    pass\n", {None}),
        # A binding of the name shadows the builtin.
        ("def f(globals):\n    return globals\n", set()),
        ("def f():\n    vars = {}\n    return vars\n", set()),
        ("globals = {}\ndef f():\n    return globals\n", set()),
        ("def f():\n    globals = 1\n    def g():\n        return globals\n", set()),
        ("f = lambda locals: locals\n", set()),
        # A direct call, and another builtin read by name, are no value.
        ("def f():\n    return globals()['x']\n", set()),
        ("import builtins\nSYMBOL = getattr(builtins, 'len')\n", set()),
    ],
)
def test_namespace_builtin_values_reflect_every_module(
    source: str, expected: set[str | None]
) -> None:
    assert _reflections(source) == expected


@pytest.mark.parametrize(
    "source",
    [
        "import builtins\nnamespace = builtins.globals()\n",
        "from builtins import globals as g\nnamespace = g()\n",
    ],
)
def test_a_namespace_builtin_called_through_builtins_reaches_its_module(
    source: str,
) -> None:
    assert _summary(source).own_namespace_line == 2


def _code_rows(source: str) -> int:
    """How many sites of ``source`` may hand a code object to FunctionType."""
    return sum(
        write.construct == "code" for write in _summary(source, None).foreign_writes
    )


@pytest.mark.parametrize(
    ("source", "rows"),
    [
        # Each spelling that yields a code object, handed on.
        (
            "import types\ndef f(g, ns):\n    return types.FunctionType(g.__code__, ns)\n",
            1,
        ),
        ("def f(frame):\n    return frame.f_code\n", 1),
        ("def f(gen):\n    keep(gen.gi_code)\n", 1),
        ("def f(coro):\n    return getattr(coro, 'cr_code')\n", 1),
        ("def f(agen):\n    x = [agen.ag_code]\n", 1),
        # Code stored into a function is a rebinding too.
        ("def f(a, b):\n    a.__code__ = b.__code__\n", 1),
        # Nested code objects and copies are code objects.
        ("def f(g):\n    return g.__code__.co_consts\n", 1),
        ("def f(g):\n    return g.__code__.replace(co_name='x')\n", 1),
        # An alias with one read that hands the object on.
        ("def f(g):\n    c = g.__code__\n    print(c.co_name)\n    return c\n", 1),
        # Metadata, identity, truth and type reads cannot run the code.
        ("def f(g):\n    return g.__code__.co_name\n", 0),
        ("def f(g):\n    return g.__code__.co_flags & 0x80\n", 0),
        ("def f(g):\n    return g.__code__ is None\n", 0),
        ("def f(g):\n    if getattr(g, '__code__', None):\n        return 1\n", 0),
        ("def f(g):\n    return not g.__code__\n", 0),
        ("def f(g):\n    return getattr(g, '__code__', None).__class__\n", 0),
        ("def f(g):\n    g.__code__\n", 0),
        (
            "def f(frame):\n"
            "    code = getattr(frame, 'f_code', None)\n"
            "    if code is None:\n"
            "        return ''\n"
            "    return getattr(code, 'co_filename', '')\n",
            0,
        ),
        # An alias that several code reads store, read only for metadata.
        (
            "def f(gen):\n"
            "    try:\n"
            "        code = gen.cr_code\n"
            "    except AttributeError:\n"
            "        code = gen.gi_code\n"
            "    return code.co_qualname\n",
            0,
        ),
        # An accessor named for a code attribute forwards it to a reader whose
        # own read of that name is classified where it happens.
        (
            "class W:\n"
            "    @property\n"
            "    def gi_code(self):\n"
            "        return self._wrapped.gi_code\n",
            0,
        ),
        # Not a code attribute.
        ("def f(g):\n    return g.__name__\n", 0),
    ],
)
def test_code_object_reads_are_code_rows(source: str, rows: int) -> None:
    assert _code_rows(source) == rows


@pytest.mark.parametrize(
    ("source", "line"),
    [
        ("x = 1\nnames = globals()\n", 2),
        ("x = 1\nvalue = globals()['x']\n", 2),
        ("x = 1\nlocal = locals()\n", 2),
        ("x = 1\ndef f():\n    return vars()\n", 0),
        ("x = 1\ndef f():\n    return globals().get('x')\n", 3),
        ("x = 1\nexec('y = 2')\n", 2),
        ("x = 1\nprint(x)\n", 0),
        # HF-844: a module-scope statement that pops a literal key with a
        # constant default is a tolerant module global delete.
        ("x = 1\nglobals().pop('x', None)\n", 0),
        ("x = 1\nif x:\n    globals().pop('x', 0)\n", 0),
        # Its value is used, its key or default is computed, or it is not at
        # module scope: the mapping is reached.
        ("x = 1\ny = globals().pop('x', None)\n", 2),
        ("x = 1\nk = 'x'\nglobals().pop(k, None)\n", 3),
        ("x = 1\nd = None\nglobals().pop('x', d)\n", 3),
        ("x = 1\nglobals().pop('x')\n", 2),
        ("x = 1\nglobals().pop('x', None, )\nglobals().get('x')\n", 3),
        ("x = 1\ndef f():\n    globals().pop('x', None)\n", 3),
        ("x = 1\nclass C:\n    globals().pop('x', None)\n", 3),
        ("x = 1\nglobals().__delitem__('x')\n", 2),
        # A possible module binding defeats the proof.
        ("x = 1\nglobals().pop('x', None)\nglobals = dict\n", 2),
        ("from m import *\nglobals().pop('x', None)\n", 2),
        ("def f(globals):\n    pass\nglobals().pop('x', None)\n", 0),
        (
            "for _ in [globals for globals in ()]:\n    pass\nglobals().pop('x', None)\n",
            0,
        ),
        ("[(globals := f) for f in fs]\nglobals().pop('x', None)\n", 2),
        (
            "def f():\n    global globals\n    globals = dict\nf()\nglobals().pop('x', None)\n",
            5,
        ),
    ],
)
def test_own_namespace_reach_is_any_use(source: str, line: int) -> None:
    assert _summary(source, None).own_namespace_line == line


@pytest.mark.parametrize(
    ("source", "name"),
    [
        ("globals().pop('_require_intrinsic', None)\n", "_require_intrinsic"),
        ("globals().pop('x', 'fallback')\n", "x"),
        ("globals().pop(name, None)\n", None),
        ("globals().pop('x', None).y\n", None),
        ("del_ = globals().pop('x', None)\n", None),
        ("globals(1).pop('x', None)\n", None),
        ("vars().pop('x', None)\n", None),
        ("globals().pop('x', default=None)\n", None),
    ],
)
def test_own_namespace_literal_delete_spelling(source: str, name: str | None) -> None:
    deletion = own_namespace_literal_delete(ast.parse(source).body[0])
    assert (deletion.name if deletion is not None else None) == name


@pytest.mark.parametrize(
    ("source", "rows"),
    [
        # A module value handed on escapes; an attribute access keeps it here.
        ("import m\npatch(m)\n", {("m", None, "module_escape")}),
        (
            "import m\nm.f()\nprint(m.x)\nif m: pass\n",
            {("m.x", None, "module_escape")},
        ),
        ("import os\nos.path.join('a')\n", set()),
        (
            "import os\nholder.p = os.path\n",
            {
                ("os.path", None, "module_escape"),
                (None, "p", "member_store"),
            },
        ),
        # Stores on receivers that are not module references.
        ("self.value = 1\n", {(None, "value", "member_store")}),
        ("setattr(obj, 'k', 1)\n", {(None, "k", "member_store")}),
        # A computed name may write any attribute, `__class__` included.
        ("setattr(obj, name, 1)\n", {(None, None, "computed_member_store")}),
        ("obj.__class__ = Other\n", {(None, "__class__", "member_store")}),
        (
            "object.__setattr__(self, '__class__', Other)\n",
            {(None, "__class__", "member_store")},
        ),
        ("self.__setattr__('k', 1)\n", {(None, "k", "member_store")}),
        ("self.__setattr__(name, 1)\n", {(None, None, "computed_member_store")}),
        ("object.__delattr__(self, name)\n", {(None, None, "computed_member_store")}),
        # sys.modules values, by literal, computed and own key.
        (
            "import sys\nclass C:\n    def __init__(self):\n"
            "        self.m = sys.modules['w']\n",
            {("w", None, "module_escape"), (None, "m", "member_store")},
        ),
        (
            "import sys\nclass C:\n    def __init__(self, k):\n"
            "        self.m = sys.modules[k]\n",
            {(None, None, "module_escape"), (None, "m", "member_store")},
        ),
        (
            "import sys\nm = sys.modules.get(__name__)\n",
            {
                (None, None, "own_module_escape"),
            },
        ),
        # The raw summary leaves the call's borrow to the typed transfer.
        (
            "import sys\ndef f(name):\n    m = sys.modules[name]\n"
            "    return getattr(m, 'x')\n",
            {(None, None, "module_escape")},
        ),
        # A store through an alias writes the module: the value escapes.
        (
            "import sys\ndef f():\n    m = sys.modules[__name__]\n    m.f = 1\n",
            {(None, None, "own_module_escape"), (None, "f", "member_store")},
        ),
        (
            "import importlib\nholder.m = importlib.import_module('w')\n",
            {("w", None, "module_escape"), (None, "m", "member_store")},
        ),
        # HF-926: a local that holds only function or class objects is no
        # module (reprlib.recursive_repr, warnings.deprecated).
        (
            "def deco(f):\n    def wrapper():\n        return f()\n"
            "    wrapper.__doc__ = f.__doc__\n"
            "    setattr(wrapper, '__annotate__', None)\n    return wrapper\n",
            {
                (None, "__doc__", "local_definition_store"),
                (None, "__annotate__", "local_definition_store"),
            },
        ),
        (
            "def f():\n    g = lambda: 0\n    g.x = 1\n"
            "    class C:\n        pass\n    C.y = 2\n",
            {
                (None, "x", "local_definition_store"),
                (None, "y", "local_definition_store"),
            },
        ),
        # A parameter, a module global, a decorated or based definition, a
        # second binding, a nonlocal rebinder or a spec attribute stays a
        # possible module write.
        ("def f(mod):\n    mod.x = 1\n", {(None, "x", "member_store")}),
        ("def g():\n    pass\ng.x = 1\n", {(None, "x", "member_store")}),
        (
            "def f():\n    @deco\n    def w():\n        pass\n    w.x = 1\n",
            {(None, "x", "member_store")},
        ),
        (
            "def f():\n    class C(Base):\n        pass\n    C.x = 1\n",
            {(None, "x", "member_store")},
        ),
        (
            "def f(m):\n    def w():\n        pass\n    w = m\n    w.x = 1\n",
            {(None, "x", "member_store")},
        ),
        (
            "def f(m):\n    def w():\n        pass\n"
            "    def h():\n        nonlocal w\n        w = m\n    h()\n    w.x = 1\n",
            {(None, "x", "member_store")},
        ),
        (
            "def f():\n    def w():\n        pass\n    w.__spec__ = None\n",
            {(None, "__spec__", "member_store")},
        ),
    ],
)
def test_escaping_module_values_and_member_stores(source: str, rows: set) -> None:
    """call_anatomy.md 4.5a: a module written through an escaped value is a
    writer in the program, which the graph joins with named member stores."""
    assert _escape_rows(source, package=None) == rows
    for write in _summary(source, package=None).foreign_writes:
        assert (
            decode_python_namespace_write(python_namespace_write_payload(write))
            == write
        )
