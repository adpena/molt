"""Project one non-incremental Cargo target into CI and the cache action."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path


def configure_cargo_cache(environ: Mapping[str, str]) -> Path:
    workspace = Path(environ["GITHUB_WORKSPACE"]).resolve()
    raw_target = environ.get("CARGO_TARGET_DIR") or "target/ci"
    if any(character in raw_target for character in "\r\n\0"):
        raise ValueError("Cargo cache target contains a control character")
    target = Path(raw_target)
    if not target.is_absolute():
        target = workspace / target
    target = target.resolve()
    relative_target = os.path.relpath(target, workspace)
    target.mkdir(parents=True, exist_ok=True)
    with Path(environ["GITHUB_ENV"]).open("a", encoding="utf-8") as github_env:
        github_env.write(f"CARGO_TARGET_DIR={target}\nCARGO_INCREMENTAL=0\n")
    with Path(environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as github_output:
        github_output.write(f"target-dir={relative_target}\n")
    return target


def main() -> int:
    target = configure_cargo_cache(os.environ)
    print(f"Cargo cache target: {target}; incremental compilation disabled")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
