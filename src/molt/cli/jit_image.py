"""The JIT image: one executable file that holds one program for ``molt-jit``.

``molt build --emit jit`` publishes it (HF-371, ``docs/design/dev_jit_tier.md``)
and ``runtime/molt-jit/src/image.rs`` reads it. The layout is::

    #!/bin/sh
    exec <runner> --launch "$0" -- "$0" "$@"
    #molt.jit-image.v1 <header bytes> <program bytes>
    <header: JSON>
    <program: backend IR, MessagePack>

The first two lines make the image a program that ``execve`` can start, so a
program's ``sys.executable`` (its argv[0]) re-runs it, as an AOT executable's
path does: ``subprocess.run([sys.executable, ...])`` and spawn-mode
``multiprocessing`` start the same program again. Each start compiles the
program again. The whole file is published with one rename, so a header can
never pair with another build's program.
"""

from __future__ import annotations

import json
import os
import shlex
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from molt import file_publication
from molt.cli.backend_ir_transport import write_backend_ir
from molt.cli.run_tier import jit_runner_argv

JIT_IMAGE_SHEBANG = "#!/bin/sh"
JIT_IMAGE_MARKER = "#molt.jit-image.v1"
# The program length is patched in after the program streamed out, so it has
# a fixed width; the runner parses leading zeros.
_LENGTH_WIDTH = 20
# The shell expands these; every other word of the launch line is quoted.
_SHELL_SELF = '"$0"'
_SHELL_ARGUMENTS = '"$@"'


def jit_image_launch_line(runner: Path) -> str:
    """The shell line that starts the image's program under ``runner``."""
    words = jit_runner_argv(str(runner), _SHELL_SELF, [_SHELL_ARGUMENTS])
    return "exec " + " ".join(
        word if word in (_SHELL_SELF, _SHELL_ARGUMENTS) else shlex.quote(word)
        for word in words
    )


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
    launch_line = jit_image_launch_line(runner)
    if "\n" in launch_line:
        raise ValueError(f"the JIT runner path contains a newline: {runner!r}")
    header_bytes = json.dumps(
        dict(header), sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")
    prefix = (
        f"{JIT_IMAGE_SHEBANG}\n{launch_line}\n{JIT_IMAGE_MARKER} {len(header_bytes)} "
    ).encode("utf-8")
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
