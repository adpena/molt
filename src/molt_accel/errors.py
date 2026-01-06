from __future__ import annotations


class MoltAccelError(Exception):
    """Base error for molt_accel client failures."""


class MoltWorkerUnavailable(MoltAccelError):
    """Worker could not be started or exited unexpectedly."""


class MoltTimeout(MoltAccelError):
    """Request exceeded the client-side timeout."""


class MoltBusy(MoltAccelError):
    """Worker queue is full or otherwise busy."""


class MoltCancelled(MoltAccelError):
    """Request was cancelled before completion."""


class MoltInvalidInput(MoltAccelError):
    """Invalid request payload or entrypoint arguments."""


class MoltInternalError(MoltAccelError):
    """Worker returned an internal error."""


class MoltProtocolError(MoltAccelError):
    """IPC framing or response protocol error."""
