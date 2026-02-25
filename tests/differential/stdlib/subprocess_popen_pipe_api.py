"""Purpose: differential coverage for subprocess.Popen pipe API wrappers."""

import subprocess
import sys


def main():
    proc = subprocess.Popen(  # noqa: S603
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "data = sys.stdin.buffer.read(); "
                "sys.stdout.buffer.write(data.upper() + b'\\nline2\\n')"
            ),
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    print("stdin_closed_before", proc.stdin.closed)
    print("write_bytes", proc.stdin.write(b"hello"))
    proc.stdin.flush()
    proc.stdin.close()
    print("stdin_closed_after", proc.stdin.closed)
    line1 = proc.stdout.readline()
    line2 = proc.stdout.readline()
    tail = proc.stdout.read()
    print("line1", line1.decode("utf-8").strip())
    print("line2", line2.decode("utf-8").strip())
    print("tail_len", len(tail))
    proc.stdout.close()
    print("stdout_closed", proc.stdout.closed)
    print("rc", proc.wait())

    proc_text = subprocess.Popen(  # noqa: S603
        [
            sys.executable,
            "-c",
            "import sys; data = sys.stdin.read(); sys.stdout.write(data + '\\n')",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    print("text_write", proc_text.stdin.write("abc"))
    proc_text.stdin.flush()
    proc_text.stdin.close()
    print("text_line", proc_text.stdout.readline().strip())
    print("text_tail", repr(proc_text.stdout.read()))
    print("text_rc", proc_text.wait())


if __name__ == "__main__":
    main()
