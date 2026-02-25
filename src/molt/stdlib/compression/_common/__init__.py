"""``compression._common`` — shared utilities for compression modules."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

from compression._common._streams import BUFFER_SIZE, BaseStream, DecompressReader

__all__ = ["BUFFER_SIZE", "BaseStream", "DecompressReader"]
