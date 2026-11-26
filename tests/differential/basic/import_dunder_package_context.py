"""Purpose: differential coverage for __import__ relative package context."""

import builtins
import os
import sys
import warnings

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


class ObservedSpec:
    @property
    def parent(self):
        print("spec parent observed")
        return "another_package"


warnings.simplefilter("ignore", DeprecationWarning)

observed_globals = {
    "__package__": "import_dunder_pkg",
    "__spec__": ObservedSpec(),
}
show(
    "explicit-package-spec",
    lambda: builtins.__import__("helper", observed_globals, None, ("ping",), 1),
)
show(
    "explicit-package-missing-parent",
    lambda: builtins.__import__(
        "helper",
        {"__package__": "import_dunder_pkg", "__spec__": missing_parent_spec},
        None,
        (),
        1,
    ),
)
show(
    "present-none-path",
    lambda: builtins.__import__(
        "helper", {"__name__": "import_dunder_pkg", "__path__": None}, None, ("ping",), 1
    ),
)
warnings.simplefilter("error", DeprecationWarning)
show(
    "spec-mismatch-warning",
    lambda: builtins.__import__("helper", observed_globals, None, ("ping",), 1),
)
warnings.simplefilter("error", ImportWarning)
show(
    "fallback-warning",
    lambda: builtins.__import__("helper", {"__name__": "import_dunder_pkg.mod"}, None, (), 1),
)
