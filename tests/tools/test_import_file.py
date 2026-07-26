from __future__ import annotations

import sys
from types import ModuleType

import pytest

from tools.import_file import (
    load_module_from_path,
    load_sibling_package_module_from_path,
)


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


def test_sibling_package_loader_resolves_relative_import_without_sys_path(
    tmp_path,
) -> None:
    package_root = tmp_path / "authority"
    package_root.mkdir()
    (package_root / "policy.py").write_text("VALUE = 41\n", encoding="utf-8")
    source = package_root / "consumer.py"
    source.write_text(
        "from .policy import VALUE\nRESULT = VALUE + 1\n",
        encoding="utf-8",
    )
    package_name = "_molt_test_sibling_package"
    module_name = f"{package_name}.consumer"
    assert str(tmp_path) not in sys.path
    try:
        module = load_sibling_package_module_from_path(module_name, source)
        assert module.RESULT == 42
        assert sys.modules[module_name] is module
        assert sys.modules[f"{package_name}.policy"].VALUE == 41
    finally:
        sys.modules.pop(f"{package_name}.policy", None)
        sys.modules.pop(module_name, None)
        sys.modules.pop(package_name, None)


def test_sibling_package_loader_restores_parent_after_failure(tmp_path) -> None:
    package_root = tmp_path / "authority"
    package_root.mkdir()
    (package_root / "policy.py").write_text("VALUE = 41\n", encoding="utf-8")
    source = package_root / "broken.py"
    source.write_text(
        "from .policy import VALUE\nraise RuntimeError('broken sibling')\n",
        encoding="utf-8",
    )
    package_name = "_molt_test_sibling_transaction"
    module_name = f"{package_name}.broken"
    prior = ModuleType(package_name)
    sys.modules[package_name] = prior
    try:
        with pytest.raises(RuntimeError, match="broken sibling"):
            load_sibling_package_module_from_path(module_name, source)
        assert sys.modules[package_name] is prior
        assert module_name not in sys.modules
        assert f"{package_name}.policy" not in sys.modules
    finally:
        sys.modules.pop(module_name, None)
        sys.modules.pop(package_name, None)
