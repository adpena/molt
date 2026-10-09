#!/usr/bin/env python3
"""Run a prebuilt WASM host from the selected Cargo output directory."""

from __future__ import annotations

import argparse
import os

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

ROOT = bind_repository_imports(__file__)

from molt._host_exit import process_returncode_for_direct_os_exit  # noqa: E402
from molt.cli.wasm_host import resolve_molt_wasm_host_binary  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--cargo-profile", required=True)
    parser.add_argument("host_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    host_args = args.host_args
    if host_args and host_args[0] == "--":
        host_args = host_args[1:]
    if not host_args:
        parser.error("host arguments are required after --")
    host = resolve_molt_wasm_host_binary(ROOT, cargo_profile=args.cargo_profile)
    if host is None:
        parser.exit(
            127,
            f"molt-wasm-host is missing for Cargo profile {args.cargo_profile!r}; "
            "build it in CARGO_TARGET_DIR or set MOLT_WASM_HOST_BIN\n",
        )
    result = _COMMANDS.run([host, *host_args], cwd=ROOT)
    return process_returncode_for_direct_os_exit(
        result.returncode, windows=os.name == "nt"
    )


if __name__ == "__main__":
    raise SystemExit(main())
