from __future__ import annotations

import os
import sys
from pathlib import Path
import json

from molt_accel.client import MoltClient, MoltClientPool
from molt_accel.decorator import molt_offload


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


def test_molt_offload_decorator() -> None:
    client = MoltClient(worker_cmd=_worker_cmd(), wire="json", env=_worker_env())

    @molt_offload(entry="list_items", codec="json", client=client)
    def handler(request):
        return {"fallback": True}

    request = type("Req", (), {"GET": {"user_id": "7"}})()
    response = handler(request)
    if isinstance(response, dict):
        status = response["status"]
        payload = response["payload"]
    else:
        status = response.status_code
        payload = json.loads(response.content)
    assert status == 200
    assert payload["request"]["user_id"] == 7
    client.close()


def test_molt_offload_decorator_pool() -> None:
    pool = MoltClientPool(
        worker_cmd=_worker_cmd(), wire="json", env=_worker_env(), pool_size=2
    )

    @molt_offload(entry="list_items", codec="json", client=pool)
    def handler(request):
        return {"fallback": True}

    request = type("Req", (), {"GET": {"user_id": "7"}})()
    response = handler(request)
    if isinstance(response, dict):
        status = response["status"]
        payload = response["payload"]
    else:
        status = response.status_code
        payload = json.loads(response.content)
    assert status == 200
    assert payload["request"]["user_id"] == 7
    pool.close()


def test_molt_offload_env_retry_policy(monkeypatch) -> None:
    captured: dict[str, object] = {}

    class FakeClient:
        def call(self, **kwargs):
            captured.update(kwargs)
            return {"ok": True}

    monkeypatch.setenv("MOLT_ACCEL_RETRY_ON_TIMEOUT", "1")
    monkeypatch.setenv("MOLT_ACCEL_RETRY_ON_BUSY", "true")
    monkeypatch.setenv("MOLT_ACCEL_RETRY_BACKOFF_MS", "9")
    monkeypatch.setenv("MOLT_ACCEL_RETRY_BACKOFF_MAX_MS", "21")

    @molt_offload(
        entry="list_items", codec="json", client=FakeClient(), idempotent=True
    )
    def handler(request):
        return {"fallback": True}

    request = type("Req", (), {"GET": {"user_id": "7"}})()
    response = handler(request)
    if isinstance(response, dict):
        status = response["status"]
    else:
        status = response.status_code
    assert status == 200
    assert captured["retry_on_timeout"] is True
    assert captured["retry_on_busy"] is True
    assert captured["retry_backoff_ms"] == 9
    assert captured["retry_backoff_max_ms"] == 21
