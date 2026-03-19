"""``compression._common`` — shared utilities for compression modules."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from compression._common._streams import BUFFER_SIZE, BaseStream, DecompressReader

__all__ = ["BUFFER_SIZE", "BaseStream", "DecompressReader"]
