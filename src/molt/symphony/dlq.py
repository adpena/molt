from __future__ import annotations

import hashlib
import json
from collections import Counter
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


def _utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def dead_letter_fingerprint(*, kind: str, name: str, command: list[str]) -> str:
    payload = json.dumps(
        {"kind": kind, "name": name, "command": command},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()[:16]


@dataclass(slots=True)
class DeadLetterQueue:
    path: Path

    def append(self, row: dict[str, Any]) -> dict[str, Any]:
        payload = dict(row)
        payload.setdefault("recorded_at", _utc_now())
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, ensure_ascii=True, sort_keys=True) + "\n")
        return payload

    def load(self, *, limit: int = 200) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        rows: list[dict[str, Any]] = []
        with self.path.open("r", encoding="utf-8") as handle:
            for raw in handle:
                text = raw.strip()
                if not text:
                    continue
                try:
                    parsed = json.loads(text)
                except Exception:
                    continue
                if isinstance(parsed, dict):
                    rows.append(parsed)
        if limit > 0:
            return rows[-limit:]
        return rows

    def summary(self, *, limit: int = 200) -> dict[str, Any]:
        rows = self.load(limit=limit)
        by_kind = Counter(str(row.get("kind") or "unknown") for row in rows)
        by_name = Counter(str(row.get("name") or "unknown") for row in rows)
        by_fingerprint = Counter(str(row.get("fingerprint") or "unknown") for row in rows)
        latest = rows[-1] if rows else None
        return {
            "path": str(self.path),
            "count": len(rows),
            "by_kind": dict(by_kind),
            "by_name": dict(by_name.most_common(20)),
            "by_fingerprint": dict(by_fingerprint.most_common(20)),
            "latest": latest,
        }
