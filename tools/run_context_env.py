#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.dx import CANONICAL_RUN_ENV_KEYS, RunContext  # noqa: E402


def _shell_double_quote(value: str) -> str:
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("$", "\\$")
        .replace("`", "\\`")
    )
    return f'"{escaped}"'


def emit_shell_exports(env: dict[str, str], keys: Sequence[str]) -> str:
    return "\n".join(f"export {key}={_shell_double_quote(env[key])}" for key in keys)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Emit canonical Molt RunContext environment exports."
    )
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--session-prefix", default="run")
    parser.add_argument(
        "--prefer-external-artifacts",
        action="store_true",
        help="Prefer a healthy external artifact root when MOLT_EXT_ROOT is unset.",
    )
    args = parser.parse_args(argv)

    env = RunContext(
        args.root,
        session_prefix=args.session_prefix,
        prefer_external_artifacts=args.prefer_external_artifacts,
    ).canonical_env(os.environ, create_dirs=False)
    print(emit_shell_exports(env, CANONICAL_RUN_ENV_KEYS))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
