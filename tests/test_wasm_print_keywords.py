import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_print_keywords_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "print_keywords.py"
    src.write_text(
        textwrap.dedent(
            """\
            class Sink:
                def __init__(self):
                    self.parts = []
                    self.flushes = 0

                def write(self, value):
                    self.parts.append(value)
                    return len(value)

                def flush(self):
                    self.flushes += 1


            class SinkNoFlush:
                def write(self, value):
                    return len(value)


            sink = Sink()
            print("a", "b", sep=":", end="!", file=sink, flush=True)
            print("sink1", repr("".join(sink.parts)), sink.flushes)

            sink2 = Sink()
            print("x", "y", sep=None, end=None, file=sink2)
            print("sink2", repr("".join(sink2.parts)), sink2.flushes)

            print("end-empty", end="")
            print("tail")


            def show_err(label, **kwargs):
                try:
                    print("err", **kwargs)
                except Exception as exc:
                    print(label, type(exc).__name__, exc)


            show_err("sep-int", sep=1)
            show_err("end-int", end=1)
            show_err("file-object", file=object())

            try:
                print("flush-missing", file=SinkNoFlush(), flush=True)
            except Exception as exc:
                print("flush-missing", type(exc).__name__, exc)
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    expected = (
        "sink1 'a:b!' 1\n"
        "sink2 'x y\\n' 0\n"
        "end-emptytail\n"
        "sep-int TypeError sep must be None or a string, not int\n"
        "end-int TypeError end must be None or a string, not int\n"
        "file-object AttributeError 'object' object has no attribute 'write'\n"
        "flush-missing AttributeError 'SinkNoFlush' object has no attribute 'flush'"
    )
    assert run.stdout.strip() == expected
