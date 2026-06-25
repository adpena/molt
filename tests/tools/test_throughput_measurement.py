from __future__ import annotations

from pathlib import Path

from tools import memory_guard
from tools import throughput_measurement as tm


def test_timeout_returncode_uses_memory_guard_authority() -> None:
    assert tm.TIMEOUT_RETURN_CODE == memory_guard.TIMEOUT_RETURN_CODE


def test_command_result_records_file_output_size(tmp_path: Path) -> None:
    output = tmp_path / "program"
    output.write_bytes(b"abcdef")

    result = tm.command_result(
        command=["molt", "build"],
        cwd=tmp_path,
        returncode=0,
        elapsed=1.25,
        timed_out=False,
        stdout="",
        stderr="",
        output_path=output,
    )

    assert result.output_size_bytes == 6
    assert result.cwd == str(tmp_path)


def test_command_result_records_directory_output_size(tmp_path: Path) -> None:
    out_dir = tmp_path / "out"
    (out_dir / "nested").mkdir(parents=True)
    (out_dir / "a.bin").write_bytes(b"abc")
    (out_dir / "nested" / "b.bin").write_bytes(b"de")

    result = tm.command_result(
        command=["molt", "build"],
        cwd=tmp_path,
        returncode=0,
        elapsed=2.0,
        timed_out=False,
        stdout="",
        stderr="",
        output_path=out_dir,
    )

    assert result.output_size_bytes == 5


def test_command_result_timeout_returncode_is_canonical(tmp_path: Path) -> None:
    result = tm.command_result(
        command=["molt", "build"],
        cwd=tmp_path,
        returncode=0,
        elapsed=3.0,
        timed_out=True,
        stdout="\n".join(str(i) for i in range(20)),
        stderr="",
    )

    assert result.returncode == tm.TIMEOUT_RETURN_CODE
    assert result.stdout_tail == "\n".join(str(i) for i in range(8, 20))
