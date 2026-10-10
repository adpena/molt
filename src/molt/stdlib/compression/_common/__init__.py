"""``compression._common`` — shared utilities for compression modules."""


from compression._common._streams import BUFFER_SIZE, BaseStream, DecompressReader

__all__ = ["BUFFER_SIZE", "BaseStream", "DecompressReader"]
