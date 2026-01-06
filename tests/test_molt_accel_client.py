from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

from molt_accel.client import MoltClient
from molt_accel.errors import MoltInvalidInput


def _worker_cmd() -> list[str]:
    root = Path(__file__).resolve().parents[1]
    worker = root / "tests" / "fixtures" / "molt_worker_stub.py"
    return [sys.executable, "-u", str(worker)]


def _worker_env() -> dict[str, str]:
    root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_WIRE"] = "json"
    return env


def test_client_echo_roundtrip() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    payload = {"ok": True, "count": 3}
    result = client.call(entry="echo", payload=payload, codec="json", timeout_ms=500)
    assert result == payload
    client.close()


def test_client_unknown_entry() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())
    with pytest.raises(MoltInvalidInput):
        client.call(entry="missing", payload={}, codec="json", timeout_ms=500)
    client.close()
