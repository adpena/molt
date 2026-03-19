"""Entry point for ``python -m zipfile``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from . import main

_require_intrinsic("molt_capabilities_has")


if __name__ == "__main__":
    main()
