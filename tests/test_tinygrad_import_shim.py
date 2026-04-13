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


def test_tinygrad_tensor_methods_cover_rope_style_surface() -> None:
    from tinygrad import Tensor, dtypes

    t = Tensor.arange(4).float().unsqueeze(1).expand(4, 2)
    assert t.shape == (4, 2)

    freqs = (Tensor.arange(0, 4, 2).float() / 4).unsqueeze(0)
    angles = Tensor.arange(3).float().unsqueeze(1) * freqs
    cos = angles.cos()
    sin = angles.sin()
    stacked = Tensor.stack(cos, sin, dim=-1)

    assert stacked.shape == (3, 2, 2)

    left = Tensor([[1.0, 2.0]])
    right = Tensor([[3.0, 4.0]])
    cat = left.cat(right, dim=0)
    assert cat.to_list() == [[1.0, 2.0], [3.0, 4.0]]

    transposed = cat.unsqueeze(0).transpose(-2, -1)
    assert transposed.shape == (1, 2, 2)

    x = Tensor([[-1.0, 2.0, 0.5]])
    assert x.maximum(0.0).to_list() == [[0.0, 2.0, 0.5]]
    assert Tensor([1.0, 3.0, 2.0]).argmax().item() == 1.0
    assert Tensor([[1.0], [2.0]]).squeeze(-1).shape == (2,)
    assert Tensor([1.0, 2.0]).cast(dtypes.float32).shape == (2,)


def test_tinygrad_tensor_indexing_covers_falcon_patterns() -> None:
    from tinygrad import Tensor, dtypes

    x = Tensor(list(range(24))).reshape(2, 3, 4).cast(dtypes.float32)
    assert x[..., :2].shape == (2, 3, 2)
    assert x[0, 1:3].to_list() == [[4.0, 5.0, 6.0, 7.0], [8.0, 9.0, 10.0, 11.0]]

    y = Tensor(list(range(12))).reshape(3, 4).cast(dtypes.float32)
    idx = Tensor([0, 2])
    assert y[idx].to_list() == [[0.0, 1.0, 2.0, 3.0], [8.0, 9.0, 10.0, 11.0]]

    packed = Tensor(list(range(8))).reshape(2, 4).cast(dtypes.float32)
    assert packed[..., 0::2].to_list() == [[0.0, 2.0], [4.0, 6.0]]
    assert packed[..., 1::2].to_list() == [[1.0, 3.0], [5.0, 7.0]]


def test_tinygrad_tensor_scalar_power_supports_rope_pattern() -> None:
    from tinygrad import Tensor

    exponents = Tensor.arange(0, 4, 2).float() / 4
    out = 10000.0 ** exponents

    assert out.shape == (2,)
    assert out.to_list() == [1.0, 100.0]
