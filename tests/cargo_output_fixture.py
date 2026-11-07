"""An uplifted Cargo output as Cargo leaves it: the artifact and its dep-info."""

from __future__ import annotations

import os
from pathlib import Path


def write_cargo_output(output: Path, payload: bytes) -> Path:
    """Write ``output`` with a dep-info naming one source no newer than it.

    `molt.cli.cargo_build_reads.cargo_output_is_current` admits an output only
    with this pair, as Cargo's fingerprint does.
    """
    output.parent.mkdir(parents=True, exist_ok=True)
    source = output.parent / f".{output.stem}-source.rs"
    source.write_text("fn main() {}\n", encoding="utf-8")
    output.write_bytes(payload)
    built = source.stat().st_mtime_ns + 1_000_000_000
    os.utime(output, ns=(built, built))
    escaped = str(source).replace(" ", "\\ ")
    output.with_suffix(".d").write_text(
        f"{str(output).replace(' ', chr(92) + ' ')}: {escaped}\n\n{escaped}:\n",
        encoding="utf-8",
    )
    return output
