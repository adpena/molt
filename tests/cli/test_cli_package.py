"""``molt.cli`` binds only ``main``, so importing it loads nothing else.

Every ``molt.cli.<submodule>`` import first runs the package ``__init__``. When
that file imported the build pipeline, each frontend worker and each ``molt``
command paid for 263 modules (HF-227). Code imports a name from the submodule
that defines it; the package has no second spelling for it.
"""

from __future__ import annotations

import json
import sys

from tests.process_guard_common import run_guarded_test_process


def _fresh_interpreter(statement: str, report: str) -> object:
    # This pytest process has already imported most of the package, which also
    # binds each loaded submodule as a package attribute. Development site
    # hooks may preload Molt modules, so probes compare against a baseline.
    probe = (
        "import json, sys\n"
        "before = set(sys.modules)\n"
        f"{statement}\n"
        f"print(json.dumps({report}))\n"
    )
    result = run_guarded_test_process([sys.executable, "-c", probe], timeout=120)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def _molt_modules_loaded_by(statement: str) -> set[str]:
    loaded = _fresh_interpreter(statement, "sorted(set(sys.modules) - before)")
    assert isinstance(loaded, list)
    return {name for name in loaded if name.startswith("molt")}


def test_importing_the_package_loads_no_other_molt_module() -> None:
    assert _molt_modules_loaded_by("import molt.cli") - {"molt"} == {"molt.cli"}


def test_the_package_binds_only_main() -> None:
    namespace = _fresh_interpreter(
        "import molt.cli",
        "{'names': sorted(name for name in vars(molt.cli) if not name.startswith('__')),"
        " 'forwards': '__getattr__' in vars(molt.cli)}",
    )
    # ``annotations`` is the ``from __future__`` feature binding. Without a
    # module ``__getattr__`` (PEP 562), any other package attribute is an
    # AttributeError until its submodule is imported.
    assert namespace == {"names": ["annotations", "main"], "forwards": False}


def test_entrypoint_imports_the_build_pipeline_only_when_a_command_builds() -> None:
    loaded = _molt_modules_loaded_by("import molt.cli.entrypoint")
    assert "molt.cli.entrypoint" in loaded
    assert not loaded & {"molt.cli.build_command", "molt.cli.build_pipeline"}
