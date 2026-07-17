from __future__ import annotations

import hashlib
import re
from collections.abc import Sequence
from pathlib import Path

from molt.cli import atomic_io
from molt.cli.runtime_paths import _build_state_root


def wasm_link_args_from_rustflags(flags: str) -> list[str]:
    """Extract ordered linker arguments from a Rust flags string."""
    tokens = flags.split()
    link_args: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token == "-C" and index + 1 < len(tokens):
            value = tokens[index + 1]
            if value.startswith("link-arg="):
                link_args.append(value.removeprefix("link-arg="))
                index += 2
                continue
        if token.startswith("-Clink-arg="):
            link_args.append(token.removeprefix("-Clink-arg="))
        index += 1
    return link_args


def write_wasm_link_args_response_file(
    response_root: Path,
    *,
    label: str,
    link_args: Sequence[str],
) -> Path:
    """Publish one content-addressed, byte-stable linker response file."""
    digest = hashlib.sha256("\0".join(link_args).encode("utf-8")).hexdigest()
    safe_label = re.sub(r"[^A-Za-z0-9_.-]+", "_", label).strip("._-") or "runtime"
    response_path = response_root / f"{safe_label}.{digest}.rsp"
    payload = ("\n".join(link_args) + "\n").encode("utf-8")
    try:
        current = response_path.read_bytes()
    except OSError:
        current = None
    if current != payload:
        atomic_io._atomic_write_bytes(response_path, payload)
    return response_path.resolve(strict=False)


def wasm_link_args_response_file(
    project_root: Path,
    *,
    label: str,
    link_flags: str,
) -> Path | None:
    """Materialize ordered link flags under the canonical build-state root."""
    link_args = wasm_link_args_from_rustflags(link_flags)
    if not link_args:
        return None
    return write_wasm_link_args_response_file(
        _build_state_root(project_root) / "wasm_link_args",
        label=label,
        link_args=link_args,
    )


def wasm_link_args_response_rustflags(
    project_root: Path,
    *,
    label: str,
    link_flags: str,
) -> str:
    """Return the bounded Rust flags form used by Cargo build environments."""
    response_path = wasm_link_args_response_file(
        project_root,
        label=label,
        link_flags=link_flags,
    )
    return "" if response_path is None else f"-C link-arg=@{response_path}"
