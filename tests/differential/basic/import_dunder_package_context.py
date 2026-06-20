"""Purpose: differential coverage for __import__ relative package context."""

import builtins
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import import_dunder_pkg.helper as seeded_helper


class Spec:
    pass


def show(label, func):
    try:
        value = func()
    except BaseException as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, value.__name__, value is seeded_helper)


spec = Spec()
spec.parent = "import_dunder_pkg"
bad_spec = Spec()
bad_spec.parent = 1
missing_parent_spec = Spec()

show("globals-none", lambda: builtins.__import__("helper", None, None, (), 1))
show("globals-nondict", lambda: builtins.__import__("helper", 1, None, (), 1))
show(
    "package-nonstr",
    lambda: builtins.__import__("helper", {"__package__": 1}, None, (), 1),
)
show("missing-name", lambda: builtins.__import__("helper", {}, None, (), 1))
show(
    "spec-parent",
    lambda: builtins.__import__(
        "helper", {"__package__": None, "__spec__": spec}, None, ("ping",), 1
    ),
)
show(
    "spec-parent-nonstr",
    lambda: builtins.__import__(
        "helper", {"__package__": None, "__spec__": bad_spec}, None, (), 1
    ),
)
show(
    "spec-parent-missing",
    lambda: builtins.__import__(
        "helper", {"__package__": None, "__spec__": missing_parent_spec}, None, (), 1
    ),
)
show(
    "name-fallback",
    lambda: builtins.__import__(
        "helper", {"__name__": "import_dunder_pkg.mod"}, None, ("ping",), 1
    ),
)
show(
    "path-name",
    lambda: builtins.__import__(
        "helper", {"__name__": "import_dunder_pkg", "__path__": []}, None, ("ping",), 1
    ),
)
show(
    "package-empty",
    lambda: builtins.__import__("helper", {"__package__": ""}, None, (), 1),
)
