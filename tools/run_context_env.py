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
        "--uv-project-python",
        default="3.12",
        help=(
            "Python version for the stable DX UV_PROJECT_ENVIRONMENT emitted by "
            "--dx when no explicit UV_PROJECT_ENVIRONMENT is set."
        ),
    )
    parser.add_argument(
        "--uv-project-purpose",
        default="dx",
        help=(
            "Purpose name for the stable DX UV_PROJECT_ENVIRONMENT emitted by "
            "--dx when no explicit UV_PROJECT_ENVIRONMENT is set."
        ),
    )
    parser.add_argument(
        "--session-scoped-uv-project-env",
        action="store_true",
        help=(
            "Keep UV_PROJECT_ENVIRONMENT tied to MOLT_SESSION_ID. The default "
            "--dx behavior uses a stable purpose+Python environment so repeated "
            "bootstrap commands do not rebuild the project venv."
        ),
    )
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
    # ONE authority owns the stable-vs-session uv project env decision
    # (dx.uv_project_env_dir). The CLI only wires its knobs into the env that
    # authority reads, so --dx emits the stable `dx__py3.12` env by default and
    # --session-scoped-uv-project-env opts back into MOLT_SESSION_ID isolation —
    # no separate CLI override lane that could drift from the authority.
    if args.dx:
        if args.session_scoped_uv_project_env:
            os.environ["MOLT_UV_PROJECT_ENV_SESSION_SCOPED"] = "1"
        os.environ.setdefault("MOLT_UV_PROJECT_PURPOSE", args.uv_project_purpose)
        os.environ.setdefault("MOLT_UV_PROJECT_PYTHON", args.uv_project_python)
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
