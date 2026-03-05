from __future__ import annotations

import json
from collections import Counter
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


def _utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


@dataclass(slots=True)
class TasteMemoryStore:
    events_path: Path
    distillations_dir: Path

    def record(self, row: dict[str, Any]) -> dict[str, Any]:
        payload = dict(row)
        payload.setdefault("recorded_at", _utc_now())
        self.events_path.parent.mkdir(parents=True, exist_ok=True)
        with self.events_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, ensure_ascii=True, sort_keys=True) + "\n")
        return payload

    def load(self, *, limit: int = 200) -> list[dict[str, Any]]:
        if not self.events_path.exists():
            return []
        rows: list[dict[str, Any]] = []
        with self.events_path.open("r", encoding="utf-8") as handle:
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

    def distill_recent(self, *, limit: int = 200) -> dict[str, Any]:
        rows = self.load(limit=limit)
        status_counts = Counter(str(row.get("cycle_status") or "unknown") for row in rows)
        failing_codes = Counter()
        successful_actions = Counter()
        preferred_tools = Counter()
        for row in rows:
            for code in row.get("failure_codes") or []:
                failing_codes[str(code)] += 1
            for action in row.get("successful_actions") or []:
                successful_actions[str(action)] += 1
            for tool in row.get("tools_used") or []:
                preferred_tools[str(tool)] += 1
        distillation = {
            "generated_at": _utc_now(),
            "samples": len(rows),
            "status_counts": dict(status_counts),
            "recurring_failure_codes": dict(failing_codes.most_common(10)),
            "successful_actions": dict(successful_actions.most_common(10)),
            "preferred_tools": dict(preferred_tools.most_common(10)),
        }
        self.distillations_dir.mkdir(parents=True, exist_ok=True)
        stamp = distillation["generated_at"].replace(":", "").replace("-", "")
        out = self.distillations_dir / f"distillation_{stamp}.json"
        out.write_text(json.dumps(distillation, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        distillation["path"] = str(out)
        return distillation
