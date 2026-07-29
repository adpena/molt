"""Execute the target-interpreter identity probe without command-line code injection."""

from __future__ import annotations

import ast
import sys
from pathlib import Path

_AUTHORITY = Path(__file__).with_name("command_envelope.py")
_MODULE = ast.parse(_AUTHORITY.read_text(encoding="utf-8"), filename=str(_AUTHORITY))
for _statement in _MODULE.body:
    if isinstance(_statement, ast.Assign) and any(
        isinstance(target, ast.Name) and target.id == "_PROBE_SCRIPT"
        for target in _statement.targets
    ):
        _PROBE_SCRIPT = ast.literal_eval(_statement.value)
        if not isinstance(_PROBE_SCRIPT, str):
            raise TypeError("_PROBE_SCRIPT authority is not a string literal")
        break
else:
    raise RuntimeError("command_envelope.py has no _PROBE_SCRIPT authority")

if len(sys.argv) < 2:
    raise SystemExit("usage: python_identity_probe.py SOURCE_ROOT [HASH_WORKERS]")
sys.path[0] = str(Path(sys.argv[1]).resolve(strict=True))
exec(compile(_PROBE_SCRIPT, str(_AUTHORITY), "exec"))
