from __future__ import annotations

import os


_SIGNED_C_INT_BITS = 32
_SIGNED_C_INT_MAX = (1 << (_SIGNED_C_INT_BITS - 1)) - 1
_SIGNED_C_INT_MIN = -(1 << (_SIGNED_C_INT_BITS - 1))
_WINDOWS_DWORD_BITS = 32
_WINDOWS_DWORD_MODULUS = 1 << _WINDOWS_DWORD_BITS
_WINDOWS_DWORD_MASK = _WINDOWS_DWORD_MODULUS - 1


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def process_returncode_for_direct_os_exit(
    returncode: int | None,
    *,
    windows: bool | None = None,
) -> int:
    """Return a status accepted by os._exit while preserving platform meaning."""
    if returncode is None:
        return 1
    if windows is None:
        windows = _is_windows_process_model()
    if windows:
        return _windows_returncode_for_direct_os_exit(returncode)
    if returncode < 0:
        return 128 + abs(returncode)
    return returncode


def _windows_returncode_for_direct_os_exit(returncode: int) -> int:
    if _SIGNED_C_INT_MIN <= returncode <= _SIGNED_C_INT_MAX:
        return returncode
    status = returncode & _WINDOWS_DWORD_MASK
    if status <= _SIGNED_C_INT_MAX:
        return status
    return status - _WINDOWS_DWORD_MODULUS
