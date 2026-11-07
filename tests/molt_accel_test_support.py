"""The real-worker boundary that the molt_accel client and decorator tests share."""

from __future__ import annotations

import os
import sys
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[1]

# A call to a real worker includes starting its Python process. 500 ms fired
# on a saturated landing host (2026-11-05, receipt 5a52f77deb, HF-545), and the
# decorator's 250 ms default fired the same way in the 2026-11-06 integrator
# batch; owner D1 raises a deadline that fired by 5-10x with its measured cause.
# The production default stays with HF-796.
WORKER_CALL_TIMEOUT_MS = 5_000


def worker_cmd() -> list[str]:
    """The stub worker, run by this interpreter."""
    return [
        sys.executable,
        "-u",
        str(_ROOT / "tests" / "fixtures" / "molt_worker_stub.py"),
    ]


def worker_env() -> dict[str, str]:
    """This process's environment, with the stub's source path and JSON wire."""
    env = os.environ.copy()
    env["PYTHONPATH"] = str(_ROOT / "src")
    env["MOLT_WORKER_WIRE"] = "json"
    return env
