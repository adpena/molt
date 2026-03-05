from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Final

_ASSET_DIR = Path(__file__).with_name("dashboard_assets")


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
    "/dashboard.js": _build_asset(
        "application/javascript; charset=utf-8",
        "dashboard.js",
    ),
}


def fetch_dashboard_asset(path: str) -> DashboardAsset | None:
    return _ASSET_MAP.get(path)
