from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import typecheck

_TYPECHECK_NAMES = (
    "_collect_py_files",
    "_collect_type_facts_for_build",
    "_run_ty_check",
    "check",
)


def test_cli_typecheck_authority_is_single_home() -> None:
    for name in _TYPECHECK_NAMES:
        assert hasattr(typecheck, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for name in _TYPECHECK_NAMES:
        assert f"def {name}(" not in cli_source
