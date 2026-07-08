from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

"""
tinygrad.device — Device selection and management.

Provides Device.DEFAULT for automatic backend selection and Device.set()
for explicit backend choice.
"""

import sys as _sys


class _Device:
    """Device selector. Access as `Device.DEFAULT`, `Device.set("CPU")`, etc."""

    _default: str = ""

    @property
    def DEFAULT(self) -> str:
        if not self._default:
            self._default = self._detect()
        return self._default

    @DEFAULT.setter
    def DEFAULT(self, value: str) -> None:
        self._default = value.upper()

    def set(self, name: str) -> None:
        """Set the active device by name."""
        self._default = name.upper()

    @staticmethod
    def _detect() -> str:
        """Auto-detect the best available device."""
        platform = _sys.platform
        if platform == "darwin":
            return "METAL"
        # Default to CPU for maximum compatibility
        return "CPU"

    def __repr__(self) -> str:
        return f"<Device: {self.DEFAULT}>"


Device = _Device()
