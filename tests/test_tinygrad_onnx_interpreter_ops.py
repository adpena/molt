from __future__ import annotations

from contextlib import contextmanager
import importlib.util
from pathlib import Path
import sys
import types

import pytest


ROOT = Path(__file__).resolve().parents[1]
TINYGRAD_STDLIB = ROOT / "src" / "molt" / "stdlib" / "tinygrad"


def _load_module(module_name: str, path: Path):
    spec = importlib.util.spec_from_file_location(module_name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


@contextmanager
def _onnx_interpreter_module():
    module_names = (
        "_intrinsics",
        "tinygrad",
        "tinygrad.dtypes",
        "tinygrad.lazy",
        "tinygrad.realize",
        "tinygrad.tensor",
        "tinygrad.onnx_interpreter",
    )
    sentinel = object()
    saved = {name: sys.modules.get(name, sentinel) for name in module_names}

    try:
        intrinsics = types.ModuleType("_intrinsics")
        intrinsics.require_intrinsic = lambda _name: (lambda *args, **kwargs: None)
        sys.modules["_intrinsics"] = intrinsics

        package = types.ModuleType("tinygrad")
        package.__path__ = [str(TINYGRAD_STDLIB)]
        sys.modules["tinygrad"] = package

        for leaf in ("dtypes", "lazy", "realize", "tensor", "onnx_interpreter"):
            module = _load_module(f"tinygrad.{leaf}", TINYGRAD_STDLIB / f"{leaf}.py")
            setattr(package, leaf, module)

        yield sys.modules["tinygrad.onnx_interpreter"]
    finally:
        for name, module in saved.items():
            if module is sentinel:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = module


def test_onnx_conv_uses_general_path_for_asymmetric_strides() -> None:
    with _onnx_interpreter_module() as onnx:
        x = onnx._make_tensor([float(i) for i in range(1, 16)], (1, 1, 3, 5))
        weight = onnx._make_tensor([1.0, 10.0], (1, 1, 1, 2))

        out = onnx._op_conv(
            [x, weight, None],
            {"strides": [1, 2], "pads": [0, 0, 0, 0], "dilations": [1, 1]},
        )[0]

        assert out.shape == (1, 1, 3, 2)
        assert onnx._realize_floats(out) == [21.0, 43.0, 76.0, 98.0, 131.0, 153.0]


def test_onnx_max_pool_dispatch_matches_nchw_reference() -> None:
    with _onnx_interpreter_module() as onnx:
        x = onnx._make_tensor([float(i) for i in range(1, 13)], (1, 1, 3, 4))

        out = onnx._OP_DISPATCH["MaxPool"](
            [x],
            {"kernel_shape": [2, 2], "strides": [1, 2], "pads": [0, 0, 0, 0]},
        )[0]

        assert out.shape == (1, 1, 2, 2)
        assert onnx._realize_floats(out) == [6.0, 8.0, 10.0, 12.0]


def test_onnx_interpreter_rejects_unimplemented_declared_outputs() -> None:
    with _onnx_interpreter_module() as onnx:
        interp = onnx.OnnxInterpreter()
        interp._values = {"x": onnx._make_tensor([1.0], (1,))}
        interp._graph_nodes = [
            {
                "op_type": "Identity",
                "inputs": ["x"],
                "outputs": ["y", "unimplemented_optional_output"],
                "attrs": {},
            }
        ]
        interp._output_names = ["y"]

        with pytest.raises(ValueError, match="produced 1 outputs for 2 declared"):
            interp.run({})
