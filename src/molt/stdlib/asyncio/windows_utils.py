"""asyncio.windows_utils — Windows-specific utilities for asyncio.

On Windows: provides pipe creation, PipeHandle, and Popen.
On non-Windows: module-level __getattr__ raises ImportError.
"""

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): asyncio.windows_utils provides PipeHandle/pipe/Popen wrappers; overlapped I/O semantics are simplified.

__all__ = ["pipe", "Popen", "PIPE", "STDOUT", "DEVNULL", "PipeHandle"]

BUFSIZE = 8192

if _sys.platform != "win32":

    def __getattr__(attr: str):
        raise ImportError("asyncio.windows_utils is only available on Windows")

else:
    import subprocess as _subprocess

    PIPE = _subprocess.PIPE
    STDOUT = _subprocess.STDOUT
    DEVNULL = _subprocess.DEVNULL

    class PipeHandle:
        """Wrapper for an idealized pipe handle on Windows."""

        def __init__(self, handle: int) -> None:
            self._handle = handle

        @property
        def handle(self) -> int:
            return self._handle

        def fileno(self) -> int:
            return self._handle

        def close(self) -> None:
            self._handle = -1

        def __del__(self) -> None:
            if self._handle != -1:
                self.close()

        def __repr__(self) -> str:
            if self._handle == -1:
                return "PipeHandle(closed)"
            return "PipeHandle(%r)" % self._handle

        def __enter__(self):
            return self

        def __exit__(self, t, v, tb):
            self.close()

    def pipe(
        *,
        duplex: bool = False,
        overlapped: tuple = (True, True),
        bufsize: int = BUFSIZE,
    ):
        """Create a pipe pair (read_handle, write_handle).

        On CPython this creates an overlapped pipe via Windows API.
        Molt simplifies to os.pipe() wrapped in PipeHandle.
        """
        import os as _os

        r, w = _os.pipe()
        return PipeHandle(r), PipeHandle(w)

    class Popen(_subprocess.Popen):
        """Subclass of subprocess.Popen using PipeHandle on Windows."""

        pass
