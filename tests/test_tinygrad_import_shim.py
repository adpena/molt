from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def test_tinygrad_import_exports_tensor_nn_and_dtypes() -> None:
    import tinygrad
    from tinygrad import Tensor, dtypes, nn

    t = Tensor(b"\x00\x01\x02")

    assert tinygrad.Tensor is Tensor
    assert hasattr(nn, "Linear")
    assert hasattr(nn, "Embedding")
    assert hasattr(nn, "RMSNorm")
    assert dtypes.float32 is float
    assert t.shape == (3,)
    assert t._buf.format_char == "B"


def test_tinygrad_nn_state_load_state_dict_assigns_nested_attrs() -> None:
    from tinygrad import Tensor
    from tinygrad.nn.state import load_state_dict

    class Layer:
        def __init__(self) -> None:
            self.weight = None

    class Model:
        def __init__(self) -> None:
            self.layers = [Layer()]
            self.output = Layer()

    model = Model()
    weight = Tensor([1.0, 2.0], shape=(2,))
    output = Tensor([3.0], shape=(1,))

    load_state_dict(
        model,
        {
            "layers.0.weight": weight,
            "output.weight": output,
        },
        strict=True,
    )

    assert model.layers[0].weight is weight
    assert model.output.weight is output


def test_tinygrad_import_shim_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "tinygrad_import_smoke.py"
    src.write_text(
        "from tinygrad import Tensor, dtypes, nn\n"
        "from tinygrad.nn.state import load_state_dict\n"
        "t = Tensor(b'\\x00\\x01\\x02')\n"
        "m = nn.RMSNorm(3)\n"
        "load_state_dict(m, {'weight': Tensor([1.0, 1.0, 1.0], shape=(3,))}, strict=True)\n"
        "print(t.shape)\n"
        "print(t._buf.format_char)\n"
        "print(dtypes.float32 is float)\n"
        "print(type(m).__name__)\n",
        encoding="utf-8",
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")

    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(src),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=900,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "(3,)",
        "B",
        "True",
        "RMSNorm",
    ]
