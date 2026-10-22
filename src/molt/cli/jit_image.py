"""The JIT image: one executable file that holds one program for ``molt-jit``.

``molt build --emit jit`` publishes it (HF-371, ``docs/design/dev_jit_tier.md``)
and ``runtime/molt-jit/src/image.rs`` reads it. The layout is::

    #!<runner> --image
    #molt.jit-image.v3 <header bytes> <program bytes>
    <header: JSON>
    <program: backend IR, MessagePack>

The first line makes the runner the image's interpreter, so ``execve`` of the
image starts ``<runner> --image <image> [args...]`` with no shell between: the
program keeps its caller's environment, and its argv[0] (``sys.executable``)
re-runs it, as an AOT executable's path does. ``subprocess.run([sys.executable,
...])`` and spawn-mode ``multiprocessing`` start the same program again, and
each start compiles it again. The kernel splits an interpreter line at
whitespace and reads only its first bytes (256 on Linux, 512 on macOS), so the
runner path must be absolute, without whitespace and short enough; publication
refuses any other. The whole file is published with one rename, so a header
can never pair with another build's program.
"""

from __future__ import annotations

import json
import os
import sys
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from molt import file_publication
from molt.cli.backend_ir_transport import write_backend_ir
from molt.cli.native_launch_abi_generated import (
    JIT_IMAGE_MARKER,
    JIT_RUNNER_IMAGE_FLAG,
)

# The program length is patched in after the program streamed out, so it has
# a fixed width; the runner parses leading zeros.
_LENGTH_WIDTH = 20
# The bytes of a `#!` line, newline included, that execve reads: Linux's
# BINPRM_BUF_SIZE and the 512-byte script header macOS reads.
_INTERPRETER_LINE_BYTES = {"linux": 256, "darwin": 512}
# Characters besides whitespace that end the interpreter path in an `#!` line.
_INTERPRETER_PATH_STOPS = frozenset({"#", "\0"})


def jit_image_interpreter_line(runner: Path, *, platform: str = sys.platform) -> str:
    """The `#!` line that makes ``runner`` the image's interpreter."""
    text = os.fspath(runner)
    if not os.path.isabs(text):
        raise ValueError(f"the JIT runner path is not absolute: {text!r}")
    # The kernel ends the interpreter path at whitespace, and macOS's parser
    # also at '#' (XNU exec_shell_imgact); a NUL ends any C path.
    stops = sorted(
        {
            repr(character)
            for character in text
            if character.isspace() or character in _INTERPRETER_PATH_STOPS
        }
    )
    if stops:
        raise ValueError(
            "the JIT runner path contains a character that ends an interpreter "
            f"line's path ({', '.join(stops)}): {text!r}"
        )
    line = f"#!{text} {JIT_RUNNER_IMAGE_FLAG}"
    limit = _INTERPRETER_LINE_BYTES.get(platform)
    if limit is None:
        raise ValueError(f"no interpreter line limit is known for {platform}")
    if len(line.encode("utf-8")) + 1 > limit:
        raise ValueError(
            f"the JIT runner path makes a {len(line.encode('utf-8')) + 1}-byte interpreter "
            f"line; {platform} reads {limit} bytes. Use a shorter Cargo target directory: {text}"
        )
    return line


class _CountingWriter:
    def __init__(self, handle: Any) -> None:
        self._handle = handle
        self.count = 0

    def write(self, data: bytes) -> int:
        self.count += len(data)
        return self._handle.write(data)


def publish_jit_image(
    destination: Path,
    *,
    runner: Path,
    header: Mapping[str, Any],
    ir: Any,
) -> None:
    """Write the image for ``ir`` and publish it at ``destination`` atomically."""
    interpreter = jit_image_interpreter_line(runner)
    header_bytes = json.dumps(
        dict(header), sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")
    prefix = (f"{interpreter}\n{JIT_IMAGE_MARKER} {len(header_bytes)} ").encode("utf-8")
    staged = file_publication.staged_file_path(
        destination, purpose="jit-image", suffix=".jit"
    )
    try:
        # Create it executable through the umask, as a linker creates its
        # output: under umask 077 the image is -rwx------, like the AOT binary.
        descriptor = os.open(
            staged,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
            0o777,
        )
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(prefix)
            length_offset = handle.tell()
            handle.write(b"0" * _LENGTH_WIDTH + b"\n")
            handle.write(header_bytes)
            program = _CountingWriter(handle)
            write_backend_ir(program, ir)
            handle.seek(length_offset)
            handle.write(str(program.count).rjust(_LENGTH_WIDTH, "0").encode("ascii"))
            handle.flush()
            os.fsync(handle.fileno())
        file_publication.durable_namespace_replace(staged, destination)
    except BaseException:
        staged.unlink(missing_ok=True)
        raise
