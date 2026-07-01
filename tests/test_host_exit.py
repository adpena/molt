from __future__ import annotations

from molt._host_exit import process_returncode_for_direct_os_exit


def test_direct_os_exit_code_preserves_posix_signal_convention() -> None:
    assert process_returncode_for_direct_os_exit(None, windows=False) == 1
    assert process_returncode_for_direct_os_exit(-15, windows=False) == 143
    assert process_returncode_for_direct_os_exit(7, windows=False) == 7


def test_direct_os_exit_code_preserves_windows_ntstatus_values() -> None:
    assert process_returncode_for_direct_os_exit(None, windows=True) == 1
    assert process_returncode_for_direct_os_exit(7, windows=True) == 7
    assert (
        process_returncode_for_direct_os_exit(0xC000013A, windows=True) == -1073741510
    )
    assert process_returncode_for_direct_os_exit(0xFFFFFFFF, windows=True) == -1
    assert (
        process_returncode_for_direct_os_exit(-1073741510, windows=True) == -1073741510
    )
