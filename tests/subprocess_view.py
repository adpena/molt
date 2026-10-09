"""Replace ``subprocess`` members for one module under test, not the process.

``monkeypatch.setattr(module.subprocess, "run", fake)`` rebinds the single
process-wide ``subprocess.run``: every other caller during the test sees the
fake too, among them hosted checkout custody's git probe, the memory guard and
pytest plugins. A test that fakes a tool for one module patches that module's
own view instead.
"""

from __future__ import annotations

import importlib
import subprocess
from types import ModuleType
from typing import Any

import pytest


def patch_module_subprocess(
    monkeypatch: pytest.MonkeyPatch,
    module: ModuleType | str,
    **replacements: Any,
) -> ModuleType:
    """Bind ``module.subprocess`` to a copy of ``subprocess`` with replacements.

    ``module`` must reach ``subprocess`` through its own module global, as
    ``import subprocess`` provides. A replacement may add a name the host's
    ``subprocess`` lacks (a Windows-only creation flag on POSIX, for one).
    """

    target = importlib.import_module(module) if isinstance(module, str) else module
    current = getattr(target, "subprocess", None)
    if current is not subprocess and not (
        isinstance(current, ModuleType) and current.__name__.endswith(".subprocess")
    ):
        raise AttributeError(
            f"{target.__name__} does not reach subprocess through a module global"
        )
    view = ModuleType(f"{target.__name__}.subprocess")
    base = current if current is not None else subprocess
    view.__dict__.update(
        (name, value) for name, value in vars(base).items() if name != "__name__"
    )
    view.__dict__.update(replacements)
    monkeypatch.setattr(target, "subprocess", view)
    return view
