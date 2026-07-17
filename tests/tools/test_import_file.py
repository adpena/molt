from __future__ import annotations

import sys
from types import ModuleType

import pytest

from tools.import_file import load_module_from_path


def test_load_module_registers_identity_before_dataclass_execution(tmp_path) -> None:
    source = tmp_path / "loaded.py"
    source.write_text(
        "from dataclasses import dataclass\n"
        "import sys\n"
        "SELF = sys.modules[__name__]\n"
        "@dataclass\n"
        "class Record:\n"
        "    value: int\n",
        encoding="utf-8",
    )
    name = "_molt_test_registered_import"
    try:
        module = load_module_from_path(name, source)
        assert module.SELF is module
        assert module.Record(7).value == 7
        assert sys.modules[name] is module
    finally:
        sys.modules.pop(name, None)


def test_load_module_restores_prior_binding_after_failure(tmp_path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("raise RuntimeError('broken body')\n", encoding="utf-8")
    name = "_molt_test_transactional_import"
    prior = ModuleType(name)
    sys.modules[name] = prior
    try:
        with pytest.raises(RuntimeError, match="broken body"):
            load_module_from_path(name, source)
        assert sys.modules[name] is prior
    finally:
        sys.modules.pop(name, None)


def test_load_module_removes_new_binding_after_failure(tmp_path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("raise RuntimeError('broken body')\n", encoding="utf-8")
    name = "_molt_test_failed_import"
    sys.modules.pop(name, None)

    with pytest.raises(RuntimeError, match="broken body"):
        load_module_from_path(name, source)

    assert name not in sys.modules
