#!/usr/bin/env python3
"""The perf methodology lock: which code and data decide a cell's numbers.

``config/perf_methodology.toml`` lists the methodology files, names the
mechanics modules they may import, and locks the files' content to a
methodology ``version``. Two boards are comparable only within one version
(``perf_history.identity_class``), so a refactor keeps history comparable and a
change that can move a number or a verdict starts a new comparability class.

When the locked content changes, ``--check`` fails until the author decides:
``--bump`` (the change can move a number or a verdict) or ``--relock`` (it
cannot). The decision lands in the reviewed diff of the authority file.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.exact_json import canonical_json_sha256  # noqa: E402
from molt.toolchain_identity import stable_file_sha256  # noqa: E402

AUTHORITY = REPO_ROOT / "config" / "perf_methodology.toml"
SCHEMA = "molt-perf-methodology-v1"
_SHA256 = re.compile(r"[0-9a-f]{64}")


@dataclass(frozen=True, slots=True)
class Methodology:
    version: int
    locked_sha256: str
    paths: tuple[str, ...]
    mechanics: Mapping[str, str]


def load(path: Path = AUTHORITY) -> Methodology:
    with path.open("rb") as stream:
        document = tomllib.load(stream)
    if set(document) != {
        "schema",
        "version",
        "content_sha256",
        "methodology",
        "mechanics",
    }:
        raise ValueError(f"perf methodology authority keys are not exact: {path}")
    if document["schema"] != SCHEMA:
        raise ValueError(f"perf methodology schema is not {SCHEMA}")
    version = document["version"]
    if type(version) is not int or version < 1:
        raise ValueError("perf methodology version must be a positive integer")
    locked = document["content_sha256"]
    if not isinstance(locked, str) or _SHA256.fullmatch(locked) is None:
        raise ValueError("perf methodology content_sha256 must be lowercase SHA-256")
    paths: list[str] = []
    for row in document["methodology"]:
        if set(row) != {"path", "role"} or not all(
            isinstance(row[key], str) and row[key] for key in ("path", "role")
        ):
            raise ValueError(f"invalid perf methodology row: {row!r}")
        paths.append(row["path"])
    mechanics: dict[str, str] = {}
    for row in document["mechanics"]:
        if set(row) != {"path", "reason"} or not all(
            isinstance(row[key], str) and row[key] for key in ("path", "reason")
        ):
            raise ValueError(f"invalid perf mechanics row: {row!r}")
        mechanics[row["path"]] = row["reason"]
    if len(set(paths)) != len(paths) or len(mechanics) != len(document["mechanics"]):
        raise ValueError("perf methodology paths must be unique")
    if overlap := set(paths) & set(mechanics):
        raise ValueError(f"paths are both methodology and mechanics: {sorted(overlap)}")
    return Methodology(
        version, locked, tuple(sorted(paths)), MappingProxyType(mechanics)
    )


def content_sha256(methodology: Methodology, *, root: Path = REPO_ROOT) -> str:
    """Digest the methodology files' bytes, keyed by repository path."""
    return canonical_json_sha256(
        {
            "schema": SCHEMA,
            "files": {
                path: stable_file_sha256(root / path, label=f"perf methodology {path}")
                for path in methodology.paths
            },
        }
    )


def identity(*, root: Path = REPO_ROOT) -> dict[str, object]:
    """The methodology fields a scoreboard records in its provenance."""
    methodology = load(root / AUTHORITY.relative_to(REPO_ROOT))
    digest = content_sha256(methodology, root=root)
    return {
        "methodology_schema": SCHEMA,
        "methodology_version": methodology.version,
        "methodology_sha256": digest,
        "methodology_locked": digest == methodology.locked_sha256,
    }


def _rewrite_lock(path: Path, *, version: int, digest: str) -> None:
    text = path.read_text(encoding="utf-8")
    for pattern, value in (
        (r"^version = \d+$", f"version = {version}"),
        (r'^content_sha256 = "[0-9a-f]{64}"$', f'content_sha256 = "{digest}"'),
    ):
        text, count = re.subn(pattern, value, text, count=1, flags=re.MULTILINE)
        if count != 1:
            raise ValueError(f"perf methodology authority lacks {pattern!r}")
    path.write_text(text, encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="fail if the lock is stale")
    mode.add_argument(
        "--relock",
        action="store_true",
        help="record new content that cannot move a number or a verdict",
    )
    mode.add_argument(
        "--bump",
        action="store_true",
        help="start a new methodology version for content that can",
    )
    args = parser.parse_args(argv)
    methodology = load()
    digest = content_sha256(methodology)
    if args.check:
        if digest == methodology.locked_sha256:
            print(f"perf methodology v{methodology.version}: locked")
            return 0
        print(
            f"perf methodology v{methodology.version}: content changed "
            f"({methodology.locked_sha256[:12]} -> {digest[:12]}). If the change can "
            "move a measured number or a verdict, run "
            "`python3 tools/perf_methodology.py --bump`; otherwise `--relock`.",
            file=sys.stderr,
        )
        return 1
    version = methodology.version + 1 if args.bump else methodology.version
    _rewrite_lock(AUTHORITY, version=version, digest=digest)
    print(f"perf methodology v{version}: locked at {digest[:12]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
