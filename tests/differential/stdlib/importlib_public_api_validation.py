"""Purpose: differential coverage for importlib public resolver validation."""

import importlib
import importlib.util


def show(label, func):
    try:
        value = func()
    except BaseException as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "OK", repr(value))


show("import-module-name-nonstr", lambda: importlib.import_module(1))
show("import-module-relative-package-none", lambda: importlib.import_module(".x", None))
show("import-module-relative-package-missing", lambda: importlib.import_module(".x"))
show("import-module-relative-package-nonstr", lambda: importlib.import_module(".x", 1))
show("import-module-relative-package-empty", lambda: importlib.import_module(".x", ""))
show("import-module-beyond-top", lambda: importlib.import_module("..x", "pkg"))
show("import-module-empty-name", lambda: importlib.import_module(""))
show("import-module-relative-empty", lambda: importlib.import_module(".", "pkg"))

show("util-name-nonstr", lambda: importlib.util.resolve_name(1, None))
show("util-relative-package-none", lambda: importlib.util.resolve_name(".x", None))
show("util-relative-package-nonstr", lambda: importlib.util.resolve_name(".x", 1))
show("util-relative-package-empty", lambda: importlib.util.resolve_name(".x", ""))
show("util-beyond-top", lambda: importlib.util.resolve_name("..x", "pkg"))
show("util-empty-name", lambda: importlib.util.resolve_name("", None))
show("util-relative-empty", lambda: importlib.util.resolve_name(".", "pkg"))
