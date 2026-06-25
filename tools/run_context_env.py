#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from pathlib import Path
import sys
from typing import Literal, Sequence, cast


REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.dx import (  # noqa: E402
    CANONICAL_RUN_ENV_KEYS,
    DX_ENV_KEYS,
    RunContext,
    render_env,
)


def emit_shell_exports(env: dict[str, str], keys: Sequence[str]) -> str:
    return render_env(env, keys, "posix")


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
    parser.add_argument(
        "--dx",
        action="store_true",
        help="Emit the full cross-platform Molt DX environment facts.",
    )
    parser.add_argument(
        "--format",
        choices=("dotenv", "posix", "powershell", "cmd", "json"),
        default="posix",
        help="Output format (default: posix).",
    )
    args = parser.parse_args(argv)

    context = RunContext(
        args.root,
        session_prefix=args.session_prefix,
        prefer_external_artifacts=args.prefer_external_artifacts,
    )
    env = (
        context.dx_env(os.environ, create_dirs=False)
        if args.dx
        else context.canonical_env(os.environ, create_dirs=False)
    )
    keys = DX_ENV_KEYS if args.dx else CANONICAL_RUN_ENV_KEYS
    fmt = cast(
        Literal["dotenv", "posix", "powershell", "cmd", "json"],
        args.format,
    )
    print(render_env(env, keys, fmt))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
