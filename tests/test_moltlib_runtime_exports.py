from __future__ import annotations

import importlib
import pytest

import molt
from molt import intrinsics as _intrinsics

import moltlib
from moltlib import concurrency


def test_moltlib_dir_includes_runtime_modules() -> None:
    entries = dir(moltlib)
    assert "asgi" in entries
    assert "concurrency" in entries
    assert "molt_db" in entries
    assert "net" in entries


def test_moltlib_asgi_surface_exposes_compat_adapter() -> None:
    molt_asgi = importlib.import_module("moltlib.asgi")
    compat_asgi = importlib.import_module("molt.asgi")
    assert hasattr(molt_asgi, "asgi_adapter")
    assert compat_asgi.asgi_adapter is molt_asgi.asgi_adapter


def test_molt_root_package_keeps_moltlib_helpers_out_of_core_namespace() -> None:
    assert "channel" not in dir(molt)
    with pytest.raises(AttributeError, match="moltlib\\.concurrency\\.channel"):
        getattr(molt, "channel")


def test_moltlib_concurrency_reexports_runtime_channel_surface() -> None:
    if not _intrinsics.runtime_active():
        pytest.skip("Molt runtime intrinsics not active")

    compat_concurrency = importlib.import_module("molt.concurrency")

    assert concurrency.channel is not None
    assert compat_concurrency.Channel is concurrency.Channel
    assert compat_concurrency.channel is concurrency.channel
    chan = concurrency.channel(1)
    chan.send(17)
    assert chan.recv() == 17


def test_moltlib_concurrency_token_roundtrip() -> None:
    if not _intrinsics.runtime_active():
        pytest.skip("Molt runtime intrinsics not active")
    token = concurrency.CancellationToken()
    assert token.cancelled() is False
    token.cancel()
    assert token.cancelled() is True


def test_moltlib_db_surface_still_exposes_query_helpers() -> None:
    if not _intrinsics.runtime_active():
        pytest.skip("Molt runtime intrinsics not active")
    molt_db = importlib.import_module("moltlib.molt_db")
    assert hasattr(molt_db, "db_query")
    assert hasattr(molt_db, "db_exec")
    assert hasattr(molt_db, "DbResponse")


def test_moltlib_net_surface_exposes_runtime_types() -> None:
    if not _intrinsics.runtime_active():
        pytest.skip("Molt runtime intrinsics not active")
    molt_net = importlib.import_module("moltlib.net")
    compat_net = importlib.import_module("molt.net")
    assert hasattr(molt_net, "Request")
    assert hasattr(molt_net, "Response")
    assert hasattr(molt_net, "Stream")
    assert hasattr(molt_net, "StreamSender")
    assert hasattr(molt_net, "WebSocket")
    assert molt_net.StreamSender is molt_net.StreamSenderBase
    assert compat_net.stream is molt_net.stream
    assert compat_net.StreamSender is molt_net.StreamSender
