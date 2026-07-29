"""Install proof child custody before executing admitted Python payload code."""

import importlib.util
import os
from pathlib import Path
import sys


_hook_dir = Path(__file__).resolve().parent
_authority = _hook_dir.parent / "execution_custody.py"
_spec = importlib.util.spec_from_file_location(
    "_molt_proof_execution_custody", _authority
)
if _spec is None or _spec.loader is None:
    raise RuntimeError("proof execution custody authority cannot be loaded")
_module = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = _module
_spec.loader.exec_module(_module)
install_python_child_custody = _module.install_python_child_custody
sys.path[:] = [
    value
    for value in sys.path
    if os.path.normcase(os.path.abspath(value)) != os.path.normcase(str(_hook_dir))
]

install_python_child_custody()
