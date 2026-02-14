from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_pickle_hooks_surface_buffers(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "pickle_hooks_surface_buffers.py"
    src.write_text(
        textwrap.dedent(
            """\
            import io
            import pickle


            class WithGetState:
                def __init__(self, x):
                    self.x = x

                def __getstate__(self):
                    return {"x": self.x + 1}


            class WithGetStateNone:
                def __init__(self):
                    self.x = 1

                def __getstate__(self):
                    return None


            class BadReduceScalar:
                def __reduce__(self):
                    return 1


            class BadReduceCallable:
                def __reduce__(self):
                    return (1, ())


            class BadReduceArgs:
                def __reduce__(self):
                    return (len, 1)


            class BadReduceSetter:
                def __reduce__(self):
                    return (len, (), None, None, None, 1)


            for proto in (2, 4, 5):
                out = pickle.loads(pickle.dumps(WithGetState(3), protocol=proto))
                print("getstate", proto, out.x)

            for proto in (2, 4, 5):
                out = pickle.loads(pickle.dumps(WithGetStateNone(), protocol=proto))
                print("getstate_none", proto, hasattr(out, "x"))

            for cls in (BadReduceScalar, BadReduceCallable, BadReduceArgs, BadReduceSetter):
                try:
                    pickle.dumps(cls(), protocol=5)
                except Exception as exc:
                    print("bad", cls.__name__, type(exc).__name__, str(exc))

            pickler = pickle.Pickler(io.BytesIO())
            print("pickler_fast_bin", pickler.fast, pickler.bin)
            print("pickler_dispatch_table_attr", hasattr(pickler, "dispatch_table"))

            pb = pickle.PickleBuffer(bytearray(b"abc"))
            payload = [pb, pb]
            captured = []

            def callback(view):
                captured.append(bytes(view))
                return None

            blob = pickle.dumps(payload, protocol=5, buffer_callback=callback)
            print("captured", len(captured), blob.count(b"\\x97"), blob.count(b"\\x98"))
            restored = pickle.loads(blob, buffers=[memoryview(raw) for raw in captured])
            print("loaded", restored[0] is restored[1], type(restored[0]).__name__, bytes(restored[0]))

            pb_readonly = pickle.PickleBuffer(b"xyz")
            captured_readonly = []

            def callback_readonly(view):
                captured_readonly.append(bytes(view))
                return None

            readonly_blob = pickle.dumps(pb_readonly, protocol=5, buffer_callback=callback_readonly)
            readonly = pickle.loads(readonly_blob, buffers=[memoryview(captured_readonly[0])])
            print("readonly", type(readonly).__name__, readonly.readonly, bytes(readonly))
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == (
        "getstate 2 4\n"
        "getstate 4 4\n"
        "getstate 5 4\n"
        "getstate_none 2 False\n"
        "getstate_none 4 False\n"
        "getstate_none 5 False\n"
        "bad BadReduceScalar PicklingError __reduce__ must return a string or tuple\n"
        "bad BadReduceCallable PicklingError first item of the tuple returned by __reduce__ must be callable\n"
        "bad BadReduceArgs PicklingError second item of the tuple returned by __reduce__ must be a tuple\n"
        "bad BadReduceSetter PicklingError sixth element of the tuple returned by __reduce__ must be a function, not int\n"
        "pickler_fast_bin 0 1\n"
        "pickler_dispatch_table_attr False\n"
        "captured 2 2 0\n"
        "loaded False memoryview b'abc'\n"
        "readonly memoryview True b'xyz'"
    )
