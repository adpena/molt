from __future__ import annotations

import ast
import os
import subprocess
import sys
from pathlib import Path

import pytest


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


def test_tinygrad_argmax_matches_upstream_surface() -> None:
    from tinygrad import Tensor

    t = Tensor([[1.0, 0.0, 2.0], [5.0, 4.0, 3.0]])
    assert t.argmax().item() == 3.0
    assert t.argmax(axis=0).to_list() == [1.0, 1.0, 1.0]
    assert t.argmax(axis=1).to_list() == [2.0, 0.0]
    assert t.argmax(axis=1, keepdim=True).to_list() == [[2.0], [0.0]]


def test_tinygrad_layernorm_and_rmsnorm_match_upstream_samples() -> None:
    from tinygrad import Tensor, nn

    x = Tensor.arange(6).reshape(2, 3).float()
    assert _flatten_numeric(x.layernorm().to_list()) == pytest.approx(
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
    assert _flatten_numeric(nn.LayerNorm(3)(x).to_list()) == pytest.approx(
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
    assert _flatten_numeric(nn.RMSNorm(3)(x).to_list()) == pytest.approx(
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
    rand_vals = _flatten_numeric(Tensor.rand(2, 3).to_list())
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
    uniform_vals = _flatten_numeric(Tensor.uniform(2, 3, low=-1.0, high=1.0).to_list())
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
    glorot_vals = _flatten_numeric(Tensor.glorot_uniform(2, 3).to_list())
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
    randn_vals = _flatten_numeric(Tensor.randn(2, 3).to_list())
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
        "print(Tensor.rand(2, 3).to_list())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.uniform(2, 3, low=-1.0, high=1.0).to_list())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.glorot_uniform(2, 3).to_list())\n",
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
        "print(t.argmax(axis=0).to_list())\n"
        "print(t.argmax(axis=1).to_list())\n"
        "print(t.argmax(axis=1, keepdim=True).to_list())\n",
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
        "print(x.layernorm().to_list())\n"
        "print(nn.LayerNorm(3)(x).to_list())\n"
        "print(nn.RMSNorm(3)(x).to_list())\n",
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
    assert _flatten_numeric(linear.weight.to_list()) == pytest.approx(
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
    assert _flatten_numeric(conv.weight.to_list()) == pytest.approx(
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
    assert _flatten_numeric(conv.bias.to_list()) == pytest.approx(
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
        "print(conv.weight.to_list())\n"
        "print(conv.bias.to_list())\n"
        "print(conv(x).to_list())\n",
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
        x.conv2d(conv.weight, conv.bias, 1, conv.stride, conv.dilation, conv.padding).to_list()
    ) == pytest.approx(
        [-0.32956963777542114, -0.4648566246032715, -0.8707174062728882, -1.0060044527053833],
        abs=1e-7,
        rel=0.0,
    )


def test_tinygrad_tensor_conv2d_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_tensor_conv2d_native.py"
    probe.write_text(
        "from tinygrad import Tensor, nn\n"
        "Tensor.manual_seed(42)\n"
        "conv = nn.Conv2d(1, 1, 3)\n"
        "x = Tensor.arange(16).reshape(1, 1, 4, 4).float()\n"
        "print(x.conv2d(conv.weight, conv.bias, 1, conv.stride, conv.dilation, conv.padding).to_list())\n",
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

def test_tinygrad_falcon_main_runs_with_tiny_config_and_empty_weights() -> None:
    root = Path(__file__).resolve().parents[1]
    probe = root / "tmp" / "tinygrad_falcon_main_probe_test.py"
    probe.write_text(
        "import struct\n"
        "from main import init, ocr_tokens\n"
        "from tinygrad import Tensor\n"
        "Tensor.manual_seed(42)\n"
        "config_json = '''{\\n"
        '  "dim": 8,\\n'
        '  "n_layers": 1,\\n'
        '  "n_heads": 2,\\n'
        '  "head_dim": 4,\\n'
        '  "n_kv_heads": 1,\\n'
        '  "ffn_dim": 16,\\n'
        '  "vocab_size": 300,\\n'
        '  "max_seq_len": 32,\\n'
        '  "rope_theta": 10000.0,\\n'
        '  "norm_eps": 1e-5,\\n'
        '  "channel_size": 3,\\n'
        '  "spatial_patch_size": 16,\\n'
        '  "temporal_patch_size": 1,\\n'
        '  "eos_id": 11,\\n'
        '  "img_id": 227,\\n'
        '  "img_row_sep_id": 228,\\n'
        '  "img_start_id": 229,\\n'
        '  "img_end_id": 230,\\n'
        '  "coord_token_id": 240,\\n'
        '  "size_token_id": 241,\\n'
        '  "image_cls_token_id": 244,\\n'
        '  "image_reg_1_token_id": 245,\\n'
        '  "image_reg_2_token_id": 246,\\n'
        '  "image_reg_3_token_id": 247,\\n'
        '  "image_reg_4_token_id": 248,\\n'
        '  "seg_token_id": 262\\n'
        "}'''\n"
        "weights = struct.pack('<Q', 2) + b'{}'\n"
        "init(weights, config_json)\n"
        "print(ocr_tokens(16, 16, bytes(16*16*3), [229], 1))\n",
        encoding="utf-8",
    )
    env = os.environ.copy()
    env["PYTHONPATH"] = (
        f"{root / 'src'}:/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr"
    )
    run = subprocess.run(
        [sys.executable, str(probe)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[44]"


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
        module_roots=(
            Path("/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr"),
        ),
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


def test_tinygrad_tensor_cat_nonzero_dim_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_cat_nonzero_dim_probe.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "y = Tensor([[5.0], [6.0]])\n"
        "print(x.cat(y, dim=1).to_list())\n",
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


def test_tinygrad_tensor_stack_nonzero_dim_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_stack_nonzero_dim_probe.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "x = Tensor([[1.0, 2.0], [3.0, 4.0]])\n"
        "y = Tensor([[5.0, 6.0], [7.0, 8.0]])\n"
        "print(Tensor.stack(x, y, dim=-1).to_list())\n",
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


def test_tinygrad_tensor_randn_and_linear_compile_in_native_molt(tmp_path: Path) -> None:
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


def test_tinygrad_falcon_main_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_falcon_main_native.py"
    probe.write_text(
        "import struct\n"
        "from main import init, ocr_tokens\n"
        "from tinygrad import Tensor\n"
        "Tensor.manual_seed(42)\n"
        "config_json = '''{\\n"
        '  "dim": 8,\\n'
        '  "n_layers": 1,\\n'
        '  "n_heads": 2,\\n'
        '  "head_dim": 4,\\n'
        '  "n_kv_heads": 1,\\n'
        '  "ffn_dim": 16,\\n'
        '  "vocab_size": 300,\\n'
        '  "max_seq_len": 32,\\n'
        '  "rope_theta": 10000.0,\\n'
        '  "norm_eps": 1e-5,\\n'
        '  "channel_size": 3,\\n'
        '  "spatial_patch_size": 16,\\n'
        '  "temporal_patch_size": 1,\\n'
        '  "eos_id": 11,\\n'
        '  "img_id": 227,\\n'
        '  "img_row_sep_id": 228,\\n'
        '  "img_start_id": 229,\\n'
        '  "img_end_id": 230,\\n'
        '  "coord_token_id": 240,\\n'
        '  "size_token_id": 241,\\n'
        '  "image_cls_token_id": 244,\\n'
        '  "image_reg_1_token_id": 245,\\n'
        '  "image_reg_2_token_id": 246,\\n'
        '  "image_reg_3_token_id": 247,\\n'
        '  "image_reg_4_token_id": 248,\\n'
        '  "seg_token_id": 262\\n'
        "}'''\n"
        "weights = struct.pack('<Q', 2) + b'{}'\n"
        "init(weights, config_json)\n"
        "print(ocr_tokens(16, 16, bytes(16*16*3), [229], 1))\n",
        encoding="utf-8",
    )
    env = _native_molt_env(
        root,
        hermetic=True,
        module_roots=(
            Path("/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr"),
        ),
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
        timeout=180,
        check=False,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[44]"
