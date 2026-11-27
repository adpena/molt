#!/usr/bin/env python3
"""The perf methodology lock: which code and data decide a cell's numbers.

``config/perf_methodology.toml`` lists the methodology files, names the
mechanics modules they may import, and locks the files' content to a
methodology ``version``. A board records the version it was measured under,
so a refactor keeps boards comparable and a change that can move a number or a
verdict starts a new comparability class. The instruction signal
(``tools/perf_instructions.py``) keys its records on its own bytes instead.

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

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"


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
    return parse(path.read_text(encoding="utf-8"), label=str(path))


def parse(text: str, *, label: str) -> Methodology:
    document = tomllib.loads(text)
    if set(document) != {
        "schema",
        "version",
        "content_sha256",
        "methodology",
        "mechanics",
    }:
        raise ValueError(f"perf methodology authority keys are not exact: {label}")
    if document["schema"] != SCHEMA:
        raise ValueError(f"perf methodology schema is not {SCHEMA}")
    version = document["version"]
    if type(version) is not int or version < 1:
        raise ValueError("perf methodology version must be a positive integer")
    locked = document["content_sha256"]
    if not isinstance(locked, str) or _SHA256.fullmatch(locked) is None:
        raise ValueError("perf methodology content_sha256 must be lowercase SHA-256")
    if not isinstance(document["methodology"], list) or not isinstance(
        document["mechanics"], list
    ):
        raise ValueError("perf methodology and mechanics must contain rows")
    paths: list[str] = []
    for row in document["methodology"]:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "role"}
            or not all(
                isinstance(row[key], str) and row[key] for key in ("path", "role")
            )
        ):
            raise ValueError(f"invalid perf methodology row: {row!r}")
        paths.append(row["path"])
    mechanics: dict[str, str] = {}
    for row in document["mechanics"]:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "reason"}
            or not all(
                isinstance(row[key], str) and row[key] for key in ("path", "reason")
            )
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


def merged_version(
    baseline: Methodology, current: Methodology, incoming: Methodology
) -> int:
    """Preserve explicit compatibility decisions on both sides of a merge."""
    version = max(current.version, incoming.version)
    if (
        min(current.version, incoming.version) > baseline.version
        and current.locked_sha256 != incoming.locked_sha256
    ):
        # Both sides declared an incompatible change starting at the same
        # baseline. Neither side's version denotes the combined contract.
        version += 1
    return version


def render_lock(text: str, *, version: int, digest: str) -> str:
    """Change only the lock fields, preserving the authored scope and comments."""
    for pattern, value in (
        (r"^version = \d+$", f"version = {version}"),
        (r'^content_sha256 = "[0-9a-f]{64}"$', f'content_sha256 = "{digest}"'),
    ):
        text, count = re.subn(pattern, value, text, count=1, flags=re.MULTILINE)
        if count != 1:
            raise ValueError(f"perf methodology authority lacks {pattern!r}")
    return text


def publish_lock(path: Path, text: str, *, version: int, digest: str) -> None:
    from molt.file_publication import atomic_write_bytes

    atomic_write_bytes(
        path,
        render_lock(text, version=version, digest=digest).encode("utf-8"),
        purpose="methodology-lock",
        create_parent=False,
    )


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
            "`uv run python tools/perf_methodology.py --bump`; otherwise `--relock`.",
            file=sys.stderr,
        )
        return 1
    version = methodology.version + 1 if args.bump else methodology.version
    text = AUTHORITY.read_text(encoding="utf-8")
    publish_lock(AUTHORITY, text, version=version, digest=digest)
    print(f"perf methodology v{version}: locked at {digest[:12]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
