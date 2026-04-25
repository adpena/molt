from __future__ import annotations

import ast
import os
import subprocess
import sys
from pathlib import Path

import pytest

from tests.helpers.falcon_ocr_paths import FALCON_OCR_ARTIFACT_ROOT
from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def _native_molt_env(
    root: Path, *, hermetic: bool = False, module_roots: tuple[Path, ...] = ()
) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_EXT_ROOT"] = str(root)
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    env["TMPDIR"] = str(root / "tmp")
    if module_roots:
        env["MOLT_MODULE_ROOTS"] = os.pathsep.join(str(path) for path in module_roots)
    if hermetic:
        env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    return env


def _flatten_numeric(values):
    out = []
    for value in values:
        if isinstance(value, list):
            out.extend(_flatten_numeric(value))
        else:
            out.append(float(value))
    return out


def test_tinygrad_import_exports_tensor_nn_and_dtypes() -> None:
    import tinygrad
    from tinygrad import Tensor, dtypes, nn

    t = Tensor(b"\x00\x01\x02")

    assert tinygrad.Tensor is Tensor
    assert hasattr(nn, "Linear")
    assert hasattr(nn, "Embedding")
    assert hasattr(nn, "RMSNorm")
    # Upstream tinygrad: dtypes.float is an alias for dtypes.float32 (both
    # structured DType objects), not Python's float. Earlier versions of
    # this test compared against `float` directly which was never correct.
    assert dtypes.float is dtypes.float32
    assert t.shape == (3,)
    # Tensor from bytes uses uint8 backing — check via the public dtype attr
    # rather than the legacy ._buf accessor that no longer exists.
    assert t.dtype is dtypes.uint8


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
    weight = Tensor([1.0, 2.0])
    output = Tensor([3.0])

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
        "load_state_dict(m, {'weight': Tensor([1.0, 1.0, 1.0])}, strict=True)\n"
        "print(t.shape)\n"
        "print(t._buf.format_char)\n"
        "print(dtypes.float32 is float)\n"
        "print(type(m).__name__)\n",
        encoding="utf-8",
    )

    env = _native_molt_env(root, hermetic=True)

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
    assert cat.tolist() == [[1.0, 2.0], [3.0, 4.0]]

    transposed = cat.unsqueeze(0).transpose(-2, -1)
    assert transposed.shape == (1, 2, 2)

    x = Tensor([[-1.0, 2.0, 0.5]])
    assert x.maximum(0.0).tolist() == [[0.0, 2.0, 0.5]]
    assert Tensor([1.0, 3.0, 2.0]).argmax().item() == 1.0
    assert Tensor([[1.0], [2.0]]).squeeze(-1).shape == (2,)
    assert Tensor([1.0, 2.0]).cast(dtypes.float32).shape == (2,)


def test_tinygrad_tensor_indexing_covers_falcon_patterns() -> None:
    from tinygrad import Tensor, dtypes

    x = Tensor(list(range(24))).reshape(2, 3, 4).cast(dtypes.float32)
    assert x[..., :2].shape == (2, 3, 2)
    assert x[0, 1:3].tolist() == [[4.0, 5.0, 6.0, 7.0], [8.0, 9.0, 10.0, 11.0]]

    y = Tensor(list(range(12))).reshape(3, 4).cast(dtypes.float32)
    idx = Tensor([0, 2])
    assert y[idx].tolist() == [[0.0, 1.0, 2.0, 3.0], [8.0, 9.0, 10.0, 11.0]]

    packed = Tensor(list(range(8))).reshape(2, 4).cast(dtypes.float32)
    assert packed[..., 0::2].tolist() == [[0.0, 2.0], [4.0, 6.0]]
    assert packed[..., 1::2].tolist() == [[1.0, 3.0], [5.0, 7.0]]


def test_tinygrad_tensor_scalar_power_supports_rope_pattern() -> None:
    from tinygrad import Tensor

    exponents = Tensor.arange(0, 4, 2).float() / 4
    out = 10000.0**exponents

    assert out.shape == (2,)
    # float32 sqrt precision — 10000**0.5 = 99.99999237... in fp32
    assert out.tolist() == pytest.approx([1.0, 100.0], abs=1e-4)


def test_tinygrad_argmax_matches_upstream_surface() -> None:
    from tinygrad import Tensor

    t = Tensor([[1.0, 0.0, 2.0], [5.0, 4.0, 3.0]])
    assert t.argmax().item() == 3.0
    assert t.argmax(axis=0).tolist() == [1.0, 1.0, 1.0]
    assert t.argmax(axis=1).tolist() == [2.0, 0.0]
    assert t.argmax(axis=1, keepdim=True).tolist() == [[2.0], [0.0]]


def test_tinygrad_layernorm_and_rmsnorm_match_upstream_samples() -> None:
    from tinygrad import Tensor, nn

    x = Tensor.arange(6).reshape(2, 3).float()
    assert _flatten_numeric(x.layernorm().tolist()) == pytest.approx(
        [
            -1.2247356176376343,
            0.0,
            1.2247356176376343,
            -1.2247356176376343,
            0.0,
            1.2247356176376343,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(nn.LayerNorm(3)(x).tolist()) == pytest.approx(
        [
            -1.2247356176376343,
            0.0,
            1.2247356176376343,
            -1.2247356176376343,
            0.0,
            1.2247356176376343,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(nn.RMSNorm(3)(x).tolist()) == pytest.approx(
        [
            0.0,
            0.7745963931083679,
            1.5491927862167358,
            0.734846830368042,
            0.9797958135604858,
            1.2247447967529297,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_random_surface_matches_upstream_samples() -> None:
    from tinygrad import Tensor

    Tensor.manual_seed(42)
    rand_vals = _flatten_numeric(Tensor.rand(2, 3).tolist())
    assert rand_vals == pytest.approx(
        [
            0.9970332384109497,
            0.5899163484573364,
            0.2225480079650879,
            0.7550519704818726,
            0.9056503772735596,
            0.8648829460144043,
        ],
        abs=1e-7,
        rel=0.0,
    )

    Tensor.manual_seed(42)
    uniform_vals = _flatten_numeric(Tensor.uniform(2, 3, low=-1.0, high=1.0).tolist())
    assert uniform_vals == pytest.approx(
        [
            0.9940664768218994,
            0.17983269691467285,
            -0.5549039840698242,
            0.5101039409637451,
            0.8113007545471191,
            0.7297658920288086,
        ],
        abs=1e-7,
        rel=0.0,
    )

    Tensor.manual_seed(42)
    glorot_vals = _flatten_numeric(Tensor.glorot_uniform(2, 3).tolist())
    assert glorot_vals == pytest.approx(
        [
            1.0889452695846558,
            0.19699685275554657,
            -0.6078668832778931,
            0.5587908625602722,
            0.8887354731559753,
            0.7994185090065002,
        ],
        abs=1e-7,
        rel=0.0,
    )

    Tensor.manual_seed(42)
    randn_vals = _flatten_numeric(Tensor.randn(2, 3).tolist())
    assert randn_vals == pytest.approx(
        [
            0.9778566956520081,
            0.4677884578704834,
            0.5526347160339355,
            -0.32882529497146606,
            -0.8555141687393188,
            0.27526429295539856,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_random_surface_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_random_native.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.rand(2, 3).tolist())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.uniform(2, 3, low=-1.0, high=1.0).tolist())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.glorot_uniform(2, 3).tolist())\n",
        encoding="utf-8",
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root, hermetic=True),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert _flatten_numeric(lines[0]) == pytest.approx(
        [
            0.9970332384109497,
            0.5899163484573364,
            0.2225480079650879,
            0.7550519704818726,
            0.9056503772735596,
            0.8648829460144043,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[1]) == pytest.approx(
        [
            0.9940664768218994,
            0.17983269691467285,
            -0.5549039840698242,
            0.5101039409637451,
            0.8113007545471191,
            0.7297658920288086,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[2]) == pytest.approx(
        [
            1.0889452695846558,
            0.19699685275554657,
            -0.6078668832778931,
            0.5587908625602722,
            0.8887354731559753,
            0.7994185090065002,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_argmax_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_argmax_native.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "t = Tensor([[1.0, 0.0, 2.0], [5.0, 4.0, 3.0]])\n"
        "print(t.argmax().item())\n"
        "print(t.argmax(axis=0).tolist())\n"
        "print(t.argmax(axis=1).tolist())\n"
        "print(t.argmax(axis=1, keepdim=True).tolist())\n",
        encoding="utf-8",
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root, hermetic=True),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert lines[0] == 3.0
    assert lines[1] == [1.0, 1.0, 1.0]
    assert lines[2] == [2.0, 0.0]
    assert lines[3] == [[2.0], [0.0]]


def test_tinygrad_norm_layers_compile_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_norms_native.py"
    probe.write_text(
        "from tinygrad import Tensor, nn\n"
        "x = Tensor.arange(6).reshape(2, 3).float()\n"
        "print(x.layernorm().tolist())\n"
        "print(nn.LayerNorm(3)(x).tolist())\n"
        "print(nn.RMSNorm(3)(x).tolist())\n",
        encoding="utf-8",
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root, hermetic=True),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    expected = [
        -1.2247356176376343,
        0.0,
        1.2247356176376343,
        -1.2247356176376343,
        0.0,
        1.2247356176376343,
    ]
    assert _flatten_numeric(lines[0]) == pytest.approx(expected, abs=1e-7, rel=0.0)
    assert _flatten_numeric(lines[1]) == pytest.approx(expected, abs=1e-7, rel=0.0)
    assert _flatten_numeric(lines[2]) == pytest.approx(
        [
            0.0,
            0.7745963931083679,
            1.5491927862167358,
            0.734846830368042,
            0.9797958135604858,
            1.2247447967529297,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_nn_initializers_match_upstream_samples() -> None:
    from tinygrad import Tensor, nn

    Tensor.manual_seed(42)
    linear = nn.Linear(3, 4, bias=False)
    assert _flatten_numeric(linear.weight.tolist()) == pytest.approx(
        [
            -0.5485392212867737,
            0.39442524313926697,
            0.37015819549560547,
            -0.1927901804447174,
            0.14214147627353668,
            -0.37743058800697327,
            -0.13001607358455658,
            -0.2206762135028839,
            0.07370937615633011,
            -0.3515026271343231,
            0.01237936969846487,
            -0.3913246691226959,
        ],
        abs=1e-7,
        rel=0.0,
    )

    Tensor.manual_seed(42)
    conv = nn.Conv2d(1, 1, 3)
    assert _flatten_numeric(conv.weight.tolist()) == pytest.approx(
        [
            -0.21733888983726501,
            -0.22886650264263153,
            0.20126104354858398,
            0.2851662039756775,
            -0.2365218847990036,
            0.19731943309307098,
            0.005402088165283203,
            -0.004575650207698345,
            -0.13713280856609344,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(conv.bias.tolist()) == pytest.approx(
        [-0.27590489387512207],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_conv2d_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_conv2d_native.py"
    probe.write_text(
        "from tinygrad import Tensor, nn\n"
        "Tensor.manual_seed(42)\n"
        "conv = nn.Conv2d(1, 1, 3)\n"
        "x = Tensor.arange(16).reshape(1, 1, 4, 4).float()\n"
        "print(conv.weight.tolist())\n"
        "print(conv.bias.tolist())\n"
        "print(conv(x).tolist())\n",
        encoding="utf-8",
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root, hermetic=True),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert _flatten_numeric(lines[0]) == pytest.approx(
        [
            -0.21733888983726501,
            -0.22886650264263153,
            0.20126104354858398,
            0.2851662039756775,
            -0.2365218847990036,
            0.19731943309307098,
            0.005402088165283203,
            -0.004575650207698345,
            -0.13713280856609344,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[1]) == pytest.approx(
        [-0.27590489387512207],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[2]) == pytest.approx(
        [
            -0.32956963777542114,
            -0.4648566246032715,
            -0.8707174062728882,
            -1.0060044527053833,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_tensor_conv2d_matches_upstream_sample() -> None:
    from tinygrad import Tensor, nn

    Tensor.manual_seed(42)
    conv = nn.Conv2d(1, 1, 3)
    x = Tensor.arange(16).reshape(1, 1, 4, 4).float()
    assert _flatten_numeric(
        x.conv2d(
            conv.weight, conv.bias, 1, conv.stride, conv.dilation, conv.padding
        ).tolist()
    ) == pytest.approx(
        [
            -0.32956963777542114,
            -0.4648566246032715,
            -0.8707174062728882,
            -1.0060044527053833,
        ],
        abs=1e-7,
        rel=0.0,
    )


def _reference_conv_nd(
    x,
    weight,
    bias=None,
    *,
    groups=1,
    stride=1,
    dilation=1,
    padding=0,
):
    spatial_ndim = len(x.shape) - 2
    stride = (stride,) * spatial_ndim if isinstance(stride, int) else tuple(stride)
    dilation = (
        (dilation,) * spatial_ndim if isinstance(dilation, int) else tuple(dilation)
    )
    if isinstance(padding, int):
        pads = tuple((padding, padding) for _ in range(spatial_ndim))
    else:
        padding = tuple(padding)
        if len(padding) == spatial_ndim:
            pads = tuple((p, p) for p in padding)
        elif len(padding) == spatial_ndim * 2:
            pairs = [(padding[i], padding[i + 1]) for i in range(0, len(padding), 2)]
            pads = tuple(reversed(pairs))
        else:
            raise ValueError("invalid padding")

    x_data = x.tolist()
    w_data = weight.tolist()
    batch, cin = x.shape[:2]
    cout, cin_per_group = weight.shape[:2]
    spatial = x.shape[2:]
    kernels = weight.shape[2:]
    out_spatial = tuple(
        (spatial[i] + pads[i][0] + pads[i][1] - dilation[i] * (kernels[i] - 1) - 1)
        // stride[i]
        + 1
        for i in range(spatial_ndim)
    )
    cout_per_group = cout // groups

    def get_x(n, c, coords):
        cur = x_data[n][c]
        for coord in coords:
            cur = cur[coord]
        return cur

    def get_w(oc, ic, coords):
        cur = w_data[oc][ic]
        for coord in coords:
            cur = cur[coord]
        return cur

    def all_indices(shape):
        if not shape:
            yield ()
            return
        for i in range(shape[0]):
            for rest in all_indices(shape[1:]):
                yield (i, *rest)

    out = []
    for n in range(batch):
        batch_out = []
        for oc in range(cout):
            group = oc // cout_per_group
            channel_out = {}
            for out_coords in all_indices(out_spatial):
                acc = 0.0 if bias is None else bias.tolist()[oc]
                for ic in range(cin_per_group):
                    src_channel = group * cin_per_group + ic
                    for kernel_coords in all_indices(kernels):
                        src_coords = tuple(
                            out_coords[i] * stride[i]
                            - pads[i][0]
                            + kernel_coords[i] * dilation[i]
                            for i in range(spatial_ndim)
                        )
                        if all(0 <= src_coords[i] < spatial[i] for i in range(spatial_ndim)):
                            acc += get_x(n, src_channel, src_coords) * get_w(
                                oc, ic, kernel_coords
                            )
                channel_out[out_coords] = acc

            def build(shape, prefix=()):
                if not shape:
                    return channel_out[prefix]
                return [build(shape[1:], (*prefix, i)) for i in range(shape[0])]

            batch_out.append(build(out_spatial))
        out.append(batch_out)
    return out


def test_tinygrad_tensor_conv2d_supports_groups_dilation_and_explicit_padding() -> None:
    from tinygrad import Tensor

    x = Tensor.arange(50).reshape(1, 2, 5, 5).float()
    weight = Tensor.arange(8).reshape(2, 1, 2, 2).float()
    bias = Tensor([0.5, -1.0])

    out = x.conv2d(
        weight,
        bias,
        groups=2,
        stride=(2, 1),
        dilation=(2, 1),
        padding=(1, 0, 0, 1),
    )

    assert out.shape == (1, 2, 2, 5)
    assert _flatten_numeric(out.tolist()) == pytest.approx(
        _flatten_numeric(
            _reference_conv_nd(
                x,
                weight,
                bias,
                groups=2,
                stride=(2, 1),
                dilation=(2, 1),
                padding=(1, 0, 0, 1),
            )
        )
    )


def test_tinygrad_tensor_conv_transpose2d_matches_upstream_sample() -> None:
    from tinygrad import Tensor

    x = Tensor.arange(9).reshape(1, 1, 3, 3).float()
    weight = Tensor.ones(1, 1, 2, 2)

    assert x.conv_transpose2d(weight).tolist() == [
        [
            [
                [0.0, 1.0, 3.0, 2.0],
                [3.0, 8.0, 12.0, 7.0],
                [9.0, 20.0, 24.0, 13.0],
                [6.0, 13.0, 15.0, 8.0],
            ]
        ]
    ]


def test_tinygrad_nn_groupnorm_matches_upstream_composition() -> None:
    from tinygrad import Tensor, nn

    x = Tensor.arange(16).reshape(1, 4, 2, 2).float()
    norm = nn.GroupNorm(2, 4)
    norm.weight = Tensor([1.0, 1.5, -1.0, 0.5])
    norm.bias = Tensor([0.0, 1.0, 2.0, -2.0])

    expected = (
        x.reshape(1, 2, -1)
        .layernorm(eps=norm.eps)
        .reshape(x.shape)
        * norm.weight.reshape(1, -1, 1, 1)
        + norm.bias.reshape(1, -1, 1, 1)
    )

    assert _flatten_numeric(norm(x).tolist()) == pytest.approx(
        _flatten_numeric(expected.tolist())
    )
    assert _flatten_numeric(nn.GroupNorm(2, 4, affine=False)(x).tolist()) == pytest.approx(
        _flatten_numeric(
            x.reshape(1, 2, -1).layernorm(eps=norm.eps).reshape(x.shape).tolist()
        )
    )


def test_molt_tinygrad_stdlib_nn_contract_matches_supported_upstream_surface() -> None:
    with tinygrad_stdlib_context("nn") as modules:
        Tensor = modules["tensor"].Tensor
        nn = modules["nn"]

        x = Tensor(list(range(9))).reshape(1, 1, 3, 3)
        weight = Tensor.ones(1, 1, 2, 2)
        assert x.conv_transpose2d(weight).tolist() == [
            [
                [
                    [0.0, 1.0, 3.0, 2.0],
                    [3.0, 8.0, 12.0, 7.0],
                    [9.0, 20.0, 24.0, 13.0],
                    [6.0, 13.0, 15.0, 8.0],
                ]
            ]
        ]

        grouped = Tensor(list(range(18))).reshape(1, 2, 3, 3).conv2d(
            Tensor.ones(2, 1, 2, 2),
            groups=2,
            padding=1,
        )
        assert grouped.tolist() == [
            [
                [
                    [0.0, 1.0, 3.0, 2.0],
                    [3.0, 8.0, 12.0, 7.0],
                    [9.0, 20.0, 24.0, 13.0],
                    [6.0, 13.0, 15.0, 8.0],
                ],
                [
                    [9.0, 19.0, 21.0, 11.0],
                    [21.0, 44.0, 48.0, 25.0],
                    [27.0, 56.0, 60.0, 31.0],
                    [15.0, 31.0, 33.0, 17.0],
                ],
            ]
        ]

        norm = nn.GroupNorm(2, 4, affine=False)
        y = Tensor(list(range(16))).reshape(1, 4, 2, 2)
        expected = y.reshape(1, 2, -1).layernorm(eps=norm.eps).reshape(y.shape)
        assert _flatten_numeric(norm(y).tolist()) == pytest.approx(
            _flatten_numeric(expected.tolist())
        )


def test_tinygrad_tensor_conv2d_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_tensor_conv2d_native.py"
    probe.write_text(
        "from tinygrad import Tensor, nn\n"
        "Tensor.manual_seed(42)\n"
        "conv = nn.Conv2d(1, 1, 3)\n"
        "x = Tensor.arange(16).reshape(1, 1, 4, 4).float()\n"
        "print(x.conv2d(conv.weight, conv.bias, 1, conv.stride, conv.dilation, conv.padding).tolist())\n",
        encoding="utf-8",
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root, hermetic=True),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert _flatten_numeric(lines[0]) == pytest.approx(
        [
            -0.32956963777542114,
            -0.4648566246032715,
            -0.8707174062728882,
            -1.0060044527053833,
        ],
        abs=1e-7,
        rel=0.0,
    )


def _falcon_source_text() -> str:
    root = Path(__file__).resolve().parents[1]
    return (
        root / "src" / "molt" / "stdlib" / "tinygrad" / "examples" / "falcon_ocr.py"
    ).read_text(encoding="utf-8")


def test_tinygrad_falcon_source_requires_rope_intrinsic() -> None:
    source = _falcon_source_text()

    assert "_load_optional_intrinsic" not in source
    assert "_MOLT_GPU_ROPE_APPLY_CONTIGUOUS = _require_intrinsic(" in source
    assert '"molt_gpu_rope_apply_contiguous"' in source


def test_tinygrad_falcon_source_requires_attention_sinks_weight() -> None:
    source = _falcon_source_text()

    assert 'state.get(f"{prefix}.attention.sinks"' not in source
    assert '_require_weight(state, f"{prefix}.attention.sinks")' in source


def test_tinygrad_falcon_source_requires_named_weight_lookup() -> None:
    source = _falcon_source_text()

    assert "def _require_weight(state, name: str):" in source
    assert "Falcon-OCR weights missing required tensor" in source
    assert '_tok_embeddings = _require_weight(state, "tok_embeddings.weight")' in source


def test_tinygrad_falcon_helper_modules_compile_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_falcon_helper_probe.py"
    probe.write_text(
        "import rope\n"
        "import mask\n"
        "from tinygrad import Tensor, dtypes\n"
        "f = rope.precompute_freqs_cis_1d(4, 3, 10000.0)\n"
        "print('freqs', f.shape)\n"
        "m = mask.build_hybrid_mask([229,244,245,246,247,248,227,230], 244, 230)\n"
        "print('mask', m.shape)\n"
        "a = Tensor.arange(3).float().unsqueeze(1)\n"
        "print('a', a.shape)\n"
        "print('dtype', dtypes.float32 is float)\n",
        encoding="utf-8",
    )
    env = _native_molt_env(
        root,
        hermetic=True,
        module_roots=(FALCON_OCR_ARTIFACT_ROOT,),
    )
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "freqs (3, 2, 2)",
        "mask (1, 1, 8, 8)",
        "a (3, 1)",
        "dtype True",
    ]


def test_tinygrad_tensor_cat_nonzero_dim_compiles_in_native_molt(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_cat_nonzero_dim_probe.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "y = Tensor([[5.0], [6.0]])\n"
        "print(x.cat(y, dim=1).tolist())\n",
        encoding="utf-8",
    )
    env = _native_molt_env(root, hermetic=True)
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[[1.0, 2.0, 5.0], [3.0, 4.0, 6.0]]"


def test_tinygrad_tensor_stack_nonzero_dim_compiles_in_native_molt(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_stack_nonzero_dim_probe.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "y = Tensor([[5.0, 6.0], [7.0, 8.0]])\n"
        "print(Tensor.stack(x, y, dim=-1).tolist())\n",
        encoding="utf-8",
    )
    env = _native_molt_env(root, hermetic=True)
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[[[1.0, 5.0], [2.0, 6.0]], [[3.0, 7.0], [4.0, 8.0]]]"


def test_tinygrad_tensor_randn_and_linear_compile_in_native_molt(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_randn_linear_native.py"
    probe.write_text(
        "from tinygrad import Tensor, nn\n"
        "Tensor.manual_seed(7)\n"
        "print('before_randn')\n"
        "x = Tensor.randn(2, 3)\n"
        "print('randn_shape', x.shape)\n"
        "layer = nn.Linear(3, 4, bias=False)\n"
        "y = layer(Tensor([[1.0, 2.0, 3.0]]))\n"
        "print('linear_shape', y.shape)\n",
        encoding="utf-8",
    )
    env = _native_molt_env(root, hermetic=True)
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "before_randn",
        "randn_shape (2, 3)",
        "linear_shape (1, 4)",
    ]
