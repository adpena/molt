from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Final

_ASSET_DIR = Path(__file__).with_name("dashboard_assets")
_DEFAULT_DASHBOARD_KERNEL_WASM = Path(
    "/Volumes/APDataStore/Molt/wasm/symphony/dashboard_kernel.wasm"
)


def _read_asset_text(filename: str) -> str:
    return (_ASSET_DIR / filename).read_text(encoding="utf-8")


@dataclass(frozen=True, slots=True)
class DashboardAsset:
    content_type: str
    body: bytes
    etag: str


def _weak_etag(payload: bytes) -> str:
    digest = hashlib.blake2s(payload, digest_size=8).hexdigest()
    return f'W/"{digest}"'


def _build_asset(content_type: str, filename: str) -> DashboardAsset:
    body = _read_asset_text(filename).encode("utf-8")
    return DashboardAsset(
        content_type=content_type,
        body=body,
        etag=_weak_etag(body),
    )


DASHBOARD_HTML: Final[str] = _read_asset_text("dashboard.html")

_ASSET_MAP: Final[dict[str, DashboardAsset]] = {
    "/dashboard.css": _build_asset(
        "text/css; charset=utf-8",
        "dashboard.css",
    ),
    "/dashboard-kernel-bridge.js": _build_asset(
        "application/javascript; charset=utf-8",
        "dashboard_kernel_bridge.js",
    ),
    "/dashboard.js": _build_asset(
        "application/javascript; charset=utf-8",
        "dashboard.js",
    ),
}


def fetch_dashboard_asset(path: str) -> DashboardAsset | None:
    return _ASSET_MAP.get(path)


_dashboard_wasm_cache: tuple[tuple[str, int, int], DashboardAsset] | None = None


def fetch_dashboard_kernel_wasm_asset() -> DashboardAsset | None:
    global _dashboard_wasm_cache
    raw_path = str(
        os.environ.get("MOLT_SYMPHONY_DASHBOARD_KERNEL_WASM_PATH") or ""
    ).strip()
    wasm_path = (
        Path(raw_path).expanduser()
        if raw_path
        else _DEFAULT_DASHBOARD_KERNEL_WASM.expanduser()
    )
    try:
        stat = wasm_path.stat()
    except OSError:
        return None
    sig = (str(wasm_path.resolve()), int(stat.st_mtime_ns), int(stat.st_size))
    cached = _dashboard_wasm_cache
    if cached is not None and cached[0] == sig:
        return cached[1]
    try:
        body = wasm_path.read_bytes()
    except OSError:
        return None
    asset = DashboardAsset(
        content_type="application/wasm",
        body=body,
        etag=_weak_etag(body),
    )
    _dashboard_wasm_cache = (sig, asset)
    return asset
