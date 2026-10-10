"""``compression.lzma`` — re-export from top-level ``lzma`` module."""


from lzma import *  # noqa: F401, F403
from lzma import __all__ as __all__  # noqa: F811
from lzma import LZMACompressor, LZMADecompressor, LZMAError  # noqa: F401
