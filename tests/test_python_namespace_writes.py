"""The namespace-write summary: which syntax rebinds a module's globals (HF-226)."""

from __future__ import annotations

import ast

import pytest

from molt.compiler_analysis.python_namespace_writes import (
    PythonNamespaceSummary,
    python_namespace_summary,
)


def _summary(source: str, package: str | None = "pkg.sub") -> PythonNamespaceSummary:
    return python_namespace_summary(ast.parse(source), package=package)


def _writes(source: str, package: str | None = "pkg.sub") -> set[tuple]:
    return {
        (write.module, write.name, write.construct)
        for write in _summary(source, package).foreign_writes
    }


@pytest.mark.parametrize(
    ("source", "names"),
    [
        # Deferred code rebinds what it declares global; a module-level
        # ``global`` statement binds nothing new.
        ("X = 1\ndef f():\n    global X, Y\n    X = 2\n", {"X", "Y"}),
        ("global X\nX = 1\n", set()),
        ("class C:\n    def m(self):\n        global Z\n", {"Z"}),
        # Own-namespace item writes with literal keys name what they bind.
        ('globals().pop("_require_intrinsic", None)\n', {"_require_intrinsic"}),
        ('globals()["k"] = 1\n', {"k"}),
        ('del globals()["k"]\n', {"k"}),
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
    "source",
    [
        "_apply(__name__, globals())\n",
        "namespace = globals()\n",
        "globals()[name] = value\n",
        "globals().update(values)\n",
        "for k, v in pairs:\n    globals()[k] = v\n",
        "names = vars()\n",
        "names = locals()\n",
        "exec(code)\n",
        "def f():\n    eval(text)\n",
        "from os import *\n",
    ],
)
def test_dynamic_own_reflection_keeps_the_whole_namespace(source: str) -> None:
    summary = _summary(source)
    assert summary.own_names is None
    assert summary.own_reflection_line >= 1


def test_own_reflection_reports_its_first_site() -> None:
    summary = _summary("x = 1\nnames = vars()\nexec(code)\n")
    assert summary.own_reflection_line == 2


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
        ("sys", "modules", "import_hooks"),
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
