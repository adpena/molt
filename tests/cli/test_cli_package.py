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


def test_parser_imports_no_command_implementation() -> None:
    # `molt --version`, `--help` and usage errors build only the parser; the
    # command modules and the CLI record module load when a command runs
    # (HF-228).
    loaded = _molt_modules_loaded_by("import molt.cli.entrypoint_parser")
    assert "molt.cli.entrypoint_parser" in loaded
    assert not loaded & {
        "molt.cli.models",
        "molt.cli.entrypoint_dispatch",
        "molt.cli.build_inputs",
        "molt.cli.setup_readiness",
        "molt.cli.toolchain_validation",
        "molt.process_guard",
    }


_BUILD_STAGES = {
    "molt.frontend",
    "molt.cli.models",
    "molt.cli.build_inputs",
    "molt.cli.frontend_pipeline",
    "molt.cli.module_graph_discovery",
    "molt.cli.build_pipeline",
}


def test_command_timing_imports_no_compiler_records() -> None:
    loaded = _molt_modules_loaded_by("import molt.cli.process_execution")
    assert "molt.cli.process_execution" in loaded
    assert not loaded & _BUILD_STAGES


def test_command_timing_has_one_record_owner() -> None:
    ownership = _fresh_interpreter(
        "from molt.cli import models, process_execution",
        "{'owner': process_execution._TimedResult.__module__,"
        " 'shared_export': hasattr(models, '_TimedResult')}",
    )
    assert ownership == {
        "owner": "molt.cli.process_execution",
        "shared_export": False,
    }


def test_build_reaches_its_build_function_without_other_commands() -> None:
    # `molt build` checks its early-cutoff record before any build stage
    # runs, so the path from `main` to the build function loads no build stage
    # and no other command's implementation (HF-250).
    loaded = _molt_modules_loaded_by(
        "from molt.cli import entrypoint\n"
        "sys.argv = ['molt', 'build', 'probe.py']\n"
        "assert entrypoint.main(build_fn=lambda _request: 0) == 0"
    )
    assert "molt.cli.entrypoint_dispatch" in loaded
    assert not loaded & _BUILD_STAGES
    assert not loaded & {
        "molt.cli.extension_commands",
        "molt.cli.script_commands",
        "molt.cli.quality_commands",
        "molt.cli.wrapper_build",
        "molt.cli.package_distribution",
        "molt.cli.deps",
        "molt.cli.setup_readiness",
        "molt.cli.toolchain_validation",
    }


def test_build_command_imports_build_stages_only_when_it_builds() -> None:
    # An early-cutoff hit returns from `build` before the first stage.
    loaded = _molt_modules_loaded_by("import molt.cli.build_command")
    assert "molt.build_record" in loaded
    assert not loaded & _BUILD_STAGES


def test_frontend_worker_imports_no_graph_discovery_or_backend() -> None:
    # A lowering worker needs the frontend and the module caches, not module
    # graph construction, source extensions or the backend (HF-228).
    loaded = _molt_modules_loaded_by("import molt.cli.frontend_worker")
    assert "molt.cli.frontend_worker" in loaded
    forbidden = {
        "molt.cli.module_graph",
        "molt.cli.build_command",
        "molt.cli.build_pipeline",
        "molt.cli.backend_execution",
        "molt.cli.backend_compile",
        "molt.cli.native_link_plan",
    }
    assert not loaded & forbidden
    assert not {name for name in loaded if name.startswith("molt.cli.source_extension")}


def test_input_report_imports_no_other_molt_module() -> None:
    # Lowering code reports its reads through this module, so it is part of
    # the frontend's semantic fingerprint; it must stay a leaf (HF-250).
    assert _molt_modules_loaded_by("import molt.input_report") - {"molt"} == {
        "molt.input_report"
    }
