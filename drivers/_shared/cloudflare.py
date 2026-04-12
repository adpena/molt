from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any


def strip_jsonc(text: str) -> str:
    out: list[str] = []
    i = 0
    in_string = False
    escaped = False
    while i < len(text):
        ch = text[i]
        if in_string:
            out.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            i += 1
            continue
        if ch == '"':
            in_string = True
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < len(text):
            nxt = text[i + 1]
            if nxt == "/":
                i += 2
                while i < len(text) and text[i] not in "\r\n":
                    i += 1
                continue
            if nxt == "*":
                i += 2
                while i + 1 < len(text) and not (text[i] == "*" and text[i + 1] == "/"):
                    i += 1
                i += 2
                continue
        out.append(ch)
        i += 1
    return re.sub(r",(\s*[}\]])", r"\1", "".join(out))


def load_jsonc(path: Path) -> dict[str, Any]:
    return json.loads(strip_jsonc(path.read_text(encoding="utf-8")))


def extract_r2_bucket_names(config: dict[str, Any]) -> list[str]:
    bucket_names: list[str] = []
    for bucket in config.get("r2_buckets", []):
        if not isinstance(bucket, dict):
            continue
        name = bucket.get("bucket_name")
        if isinstance(name, str) and name:
            bucket_names.append(name)
    return bucket_names
