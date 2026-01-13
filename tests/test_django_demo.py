from __future__ import annotations

import os
import sys
from pathlib import Path

import pytest

try:
    import django  # type: ignore
except ImportError:  # pragma: no cover - optional dep
    django = None  # type: ignore

HAS_DJANGO = django is not None
pytestmark = pytest.mark.skipif(not HAS_DJANGO, reason="django not installed")


def _setup_django() -> None:
    root = Path(__file__).resolve().parents[1] / "demo" / "django_app"
    sys.path.insert(0, str(root))
    os.environ.setdefault("DJANGO_SETTINGS_MODULE", "demoapp.settings")
    assert django is not None
    django.setup()


def _worker_cmd() -> str:
    root = Path(__file__).resolve().parents[1]
    worker = root / "tests" / "fixtures" / "molt_worker_stub.py"
    return f"{sys.executable} -u {worker}"


def _setup_worker_env() -> None:
    os.environ["MOLT_WORKER_CMD"] = _worker_cmd()
    os.environ["MOLT_WIRE"] = "json"
    os.environ["MOLT_STUB_LIST_ITEMS_CODEC_OUT"] = "json"
    # Ensure molt_accel is importable by the stub worker.
    root = Path(__file__).resolve().parents[1]
    sys.path.insert(0, str(root))
    sys.path.insert(0, str(root / "src"))
    os.environ["PYTHONPATH"] = f"{root}:{root / 'src'}"


def test_demo_baseline_and_offload_parity() -> None:
    _setup_django()
    _setup_worker_env()
    from django.test import Client

    client = Client()
    resp_base = client.get("/baseline/?user_id=7&limit=5")
    resp_offload = client.get("/offload/?user_id=7&limit=5")

    assert resp_base.status_code == 200
    assert resp_offload.status_code == 200
    base_json = resp_base.json()
    offload_json = resp_offload.json()
    assert "counts" in offload_json and "items" in offload_json
    assert set(base_json["counts"].keys()) == set(offload_json["counts"].keys())


def test_compute_offload_parity() -> None:
    _setup_django()
    _setup_worker_env()
    from django.test import Client

    client = Client()
    resp_base = client.get("/compute/?values=1,2,3&scale=2&offset=1")
    resp_offload = client.get("/compute_offload/?values=1,2,3&scale=2&offset=1")

    assert resp_base.status_code == 200
    assert resp_offload.status_code == 200
    assert resp_base.json() == resp_offload.json()


def test_demo_baseline_sqlite_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _setup_django()
    from demoapp.db_seed import seed_db
    from django.test import Client

    db_path = tmp_path / "demo.sqlite"
    seed_db(db_path, users=1, items_per_user=4)
    monkeypatch.setenv("MOLT_DEMO_DB_PATH", str(db_path))

    client = Client()
    resp = client.get("/baseline/?user_id=1&limit=3")

    assert resp.status_code == 200
    payload = resp.json()
    assert len(payload["items"]) == 3
    assert payload["counts"] == {"open": 2, "closed": 1}
