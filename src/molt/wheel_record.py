"""Serialize wheel RECORD rows from already computed SHA-256 identities.

The wheel specification requires CSV paths, URL-safe unpadded digests and an
unhashed self row: https://packaging.python.org/en/latest/specifications/binary-distribution-format/
Callers own member ordering and hashing so serialization never rereads payloads.
"""

from __future__ import annotations

import base64
from collections.abc import Iterable
import csv
import io


def wheel_record_bytes(
    entries: Iterable[tuple[str, bytes, int]], *, record_path: str
) -> bytes:
    """Write each supplied member once, followed by RECORD's empty identity."""
    output = io.StringIO(newline="")
    writer = csv.writer(output, lineterminator="\n")
    seen = {record_path}
    for path, digest, size in entries:
        if path in seen:
            raise ValueError(f"duplicate wheel RECORD member: {path!r}")
        seen.add(path)
        encoded = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
        writer.writerow((path, f"sha256={encoded}", size))
    writer.writerow((record_path, "", ""))
    return output.getvalue().encode("utf-8")
