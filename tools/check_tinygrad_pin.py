#!/usr/bin/env python3
"""Assert the in-tree off-the-shelf tinygrad equals the pinned upstream revision.

The ML-contract source of truth is a SINGLE pinned upstream tinygrad revision
(``docs/spec/tinygrad_pin.md``). Every derived parity fact
(``runtime/molt-gpu/op_contract.toml`` and the planned API contract / diff
oracle) is a function of that pin. If someone re-vendors a newer tinygrad into
``bench/friends/repos/tinygrad_off_the_shelf/`` without re-running the
generators, every derived fact silently goes stale — the exact "fidelity
theater" failure mode doc 67 §1.2.1 found already live.

This gate makes that drift RED instead of silent: it reads the off-the-shelf
``pyproject.toml`` (STRUCTURALLY, via ``tomllib`` — never a regex over the text)
and fails if its ``[project] version`` is not exactly ``PINNED_TINYGRAD_VERSION``.

``PINNED_TINYGRAD_VERSION`` is the machine-readable half of the pin doc; the two
MUST agree, and this script also verifies the doc's pinned-version line matches
(so the human-readable pin and the gate constant cannot drift from each other).

Usage::

    python3 tools/check_tinygrad_pin.py            # assert; exit 1 on drift
    python3 tools/check_tinygrad_pin.py --check     # alias of the default (CI mode)

Mirrors ``tools/gen_op_kinds.py --check`` discipline: fail-loud, no silent
fallback, exit 1 on any divergence.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:  # pragma: no cover - fallback for <3.11
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]

# The pinned upstream tinygrad revision. This is the authoritative machine-readable
# pin; docs/spec/tinygrad_pin.md is its human-readable companion (kept in sync by the
# doc cross-check below). To bump: follow the bump protocol in the pin doc.
PINNED_TINYGRAD_VERSION = "0.13.0"

# The in-tree read-only oracle (the vendored upstream source).
OFF_THE_SHELF_PYPROJECT = (
    ROOT / "bench/friends/repos/tinygrad_off_the_shelf/pyproject.toml"
)
PIN_DOC = ROOT / "docs/spec/tinygrad_pin.md"


class TinygradPinError(RuntimeError):
    """A pin divergence (the gate's RED state)."""


def read_off_the_shelf_version() -> str:
    """Return the off-the-shelf tinygrad ``[project] version`` (structural parse)."""
    if not OFF_THE_SHELF_PYPROJECT.exists():
        raise TinygradPinError(
            f"off-the-shelf tinygrad pyproject missing: {OFF_THE_SHELF_PYPROJECT}\n"
            "  the pinned upstream oracle "
            "(bench/friends/repos/tinygrad_off_the_shelf/) is absent"
        )
    data = tomllib.loads(OFF_THE_SHELF_PYPROJECT.read_text(encoding="utf-8"))
    project = data.get("project")
    if not isinstance(project, dict):
        raise TinygradPinError(
            f"{OFF_THE_SHELF_PYPROJECT}: missing or malformed [project] table"
        )
    name = project.get("name")
    if name != "tinygrad":
        raise TinygradPinError(
            f"{OFF_THE_SHELF_PYPROJECT}: [project] name is {name!r}, expected 'tinygrad' "
            "(is this the right vendored package?)"
        )
    version = project.get("version")
    if not isinstance(version, str) or not version:
        raise TinygradPinError(
            f"{OFF_THE_SHELF_PYPROJECT}: [project] version is missing or not a string"
        )
    return version


def read_pin_doc_version() -> str:
    """Return the version string recorded in the pin doc's pinned-version row.

    The pin doc states the version in a markdown table row of the form
    ``| **pinned version** | **`0.13.0`** |``. We extract it structurally (anchored
    on the ``pinned version`` label cell) so the doc and the gate constant cannot
    silently diverge.
    """
    if not PIN_DOC.exists():
        raise TinygradPinError(f"pin doc missing: {PIN_DOC}")
    text = PIN_DOC.read_text(encoding="utf-8")
    # Match the table row whose first cell labels the pinned version; capture the
    # backtick-quoted version in the value cell. Tolerant of bold/whitespace.
    match = re.search(
        r"^\|\s*\**\s*pinned version\s*\**\s*\|\s*\**\s*`([^`]+)`",
        text,
        flags=re.IGNORECASE | re.MULTILINE,
    )
    if match is None:
        raise TinygradPinError(
            f"{PIN_DOC}: could not find the '**pinned version**' table row "
            "(expected a row like `| **pinned version** | **`0.13.0`** |`)"
        )
    return match.group(1).strip()


def check() -> None:
    """Run every pin assertion; raise ``TinygradPinError`` on the first failure."""
    # (1) The pin doc's stated version must equal the authoritative constant.
    doc_version = read_pin_doc_version()
    if doc_version != PINNED_TINYGRAD_VERSION:
        raise TinygradPinError(
            "pin doc / gate constant disagree:\n"
            f"  docs/spec/tinygrad_pin.md pins  {doc_version!r}\n"
            f"  PINNED_TINYGRAD_VERSION is      {PINNED_TINYGRAD_VERSION!r}\n"
            "  update both together (the doc and tools/check_tinygrad_pin.py)"
        )

    # (2) The vendored upstream source must equal the pin.
    actual = read_off_the_shelf_version()
    if actual != PINNED_TINYGRAD_VERSION:
        raise TinygradPinError(
            "vendored tinygrad has drifted from the pin:\n"
            f"  bench/friends/repos/tinygrad_off_the_shelf is tinygrad {actual!r}\n"
            f"  the pin (docs/spec/tinygrad_pin.md) is        {PINNED_TINYGRAD_VERSION!r}\n"
            "  a silent dependency bump invalidates every derived parity fact;\n"
            "  follow the bump protocol in docs/spec/tinygrad_pin.md (re-vendor +\n"
            "  regenerate all gpu_*_contract facts + re-run the diff oracle as ONE\n"
            "  change), or restore the pinned revision."
        )


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="CI mode (default behavior): exit 1 on any pin divergence",
    )
    ap.parse_args(argv)

    try:
        check()
    except TinygradPinError as exc:
        print(f"tinygrad pin check FAILED:\n{exc}", file=sys.stderr)
        return 1
    print(
        f"tinygrad pin OK: off-the-shelf == pinned upstream {PINNED_TINYGRAD_VERSION}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
