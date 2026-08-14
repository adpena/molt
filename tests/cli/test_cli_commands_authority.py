from __future__ import annotations

import ast
from pathlib import Path

import molt.cli as cli
from molt.cli import (
    extension_commands,
    process_execution,
    quality_commands,
    script_commands,
)


_COMMAND_AUTHORITIES = {
    process_execution: (
        "_format_duration",
        "_run_command",
        "_run_command_timed",
    ),
    script_commands: (
        "_deploy",
        "_run_script_cross",
        "compare",
        "diff",
        "parity_run",
        "run_script",
    ),
    quality_commands: (
        "_internal_batch_build_server",
        "_normalize_internal_batch_stdlib_profile",
        "bench",
        "lint",
        "profile",
        "test",
    ),
    extension_commands: (
        "_extension_export_config_errors",
        "_extension_export_package",
        "_extension_manifest_public_exports",
        "_extension_source_text_by_path",
        "_source_extension_compile_command_for_source",
        "_source_plan_abi_include_order",
        "_source_plan_include_paths_for_abi",
        "_source_plan_skipped_generated_sources_warning",
        "_sysroot_arg_value",
        "extension_build",
        "extension_metadata",
    ),
}


def test_cli_commands_are_owned_by_focused_modules() -> None:
    for module, names in _COMMAND_AUTHORITIES.items():
        module_path = Path(module.__file__).resolve()
        tree = ast.parse(
            module_path.read_text(encoding="utf-8"), filename=str(module_path)
        )
        top_level_definitions = {
            node.name
            for node in tree.body
            if isinstance(node, (ast.AsyncFunctionDef, ast.ClassDef, ast.FunctionDef))
        }
        assert top_level_definitions == set(names), module.__name__
        for name in names:
            assert hasattr(module, name), f"{module.__name__}.{name}"
            assert getattr(module, name).__module__ == module.__name__, name
            assert not hasattr(cli, name), name


def test_legacy_commands_authority_is_deleted() -> None:
    assert not hasattr(cli, "_commands")
    assert not (Path(cli.__file__).resolve().parent / "commands.py").exists()
