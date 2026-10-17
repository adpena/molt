"""The namespace-write summary: which syntax rebinds a module's globals (HF-226)."""

from __future__ import annotations

import ast

import pytest
from tests.process_guard_common import run_guarded_test_process

from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceSummary,
    decode_python_namespace_write,
    python_namespace_write_payload,
    python_namespace_summary,
)
from molt.compiler_analysis.python_source_keys import python_node_source_key


def _summary(source: str, package: str | None = "pkg.sub") -> PythonNamespaceSummary:
    return python_namespace_summary(ast.parse(source), package=package)


def _writes(source: str, package: str | None = "pkg.sub") -> set[tuple]:
    return {
        (write.module, write.name, write.construct)
        for write in _summary(source, package).foreign_writes
    }


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
    owners = {write.name: write.source_owner for write in summary.foreign_writes}
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
    writes = python_namespace_summary(tree, target_python=(3, 14)).foreign_writes
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
    with pytest.raises(ValueError):
        decode_python_namespace_write(["target", "flag", "attribute", 1, owner])


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


def test_computed_disjoint_cache_write_is_retained_without_name_identity_claim() -> (
    None
):
    from molt.compiler_analysis.python_namespace_graph import (
        graph_python_namespace_writes,
    )

    source = "import sys\nsys.modules.setdefault(f'{__name__}.path', value)\n"
    writes = _summary(source).foreign_writes
    assert len(writes) == 1
    assert (writes[0].module, writes[0].name, writes[0].construct) == (
        "sys.modules",
        None,
        "import_binding_disjoint",
    )
    assert (
        decode_python_namespace_write(python_namespace_write_payload(writes[0]))
        == writes[0]
    )
    result = graph_python_namespace_writes(writes, lambda _name: None)
    assert not result.members
    assert not result.by_source


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
        # A module handed to a generic writer is monkeypatching, outside the
        # spec 0216 contract; only named syntax counts.
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
