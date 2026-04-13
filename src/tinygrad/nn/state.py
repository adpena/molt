"""tinygrad.nn.state compatibility helpers backed by Molt."""

from __future__ import annotations

from molt.gpu.interop import load_safetensors_bytes
from molt.gpu.tensor import Tensor


def _tensor_to_bytes(tensor: Tensor) -> bytes:
    width = tensor._buf.itemsize
    return bytes(tensor._buf._data[: tensor.size * width])


def safe_load(blob) -> dict[str, Tensor]:
    """Load a SafeTensors mapping from bytes or a byte tensor."""
    if isinstance(blob, Tensor):
        payload = _tensor_to_bytes(blob)
    elif isinstance(blob, (bytes, bytearray, memoryview)):
        payload = bytes(blob)
    else:
        raise TypeError(f"safe_load expects bytes-like or Tensor, got {type(blob).__name__}")
    return load_safetensors_bytes(payload)


def load_state_dict(model, state, *, strict: bool = True, verbose: bool = False) -> None:
    """Populate dotted-path tensor attributes on a model object."""
    missing: list[str] = []
    for key, value in state.items():
        parts = key.split(".")
        target = model
        ok = True
        for part in parts[:-1]:
            if part.isdigit():
                index = int(part)
                if isinstance(target, (list, tuple)) and 0 <= index < len(target):
                    target = target[index]
                else:
                    ok = False
                    break
            elif hasattr(target, part):
                target = getattr(target, part)
            else:
                ok = False
                break
        if not ok or not hasattr(target, parts[-1]):
            if strict:
                missing.append(key)
            continue
        setattr(target, parts[-1], value)
        if verbose:
            print(key)
    if strict and missing:
        raise KeyError(f"missing state target(s): {missing}")


__all__ = ["load_state_dict", "safe_load"]
