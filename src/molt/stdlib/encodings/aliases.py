"""Encoding Aliases Support

This module is used by the encodings package search function to
map encodings names to module names.

Note that the search function normalizes the encoding names before
doing the lookup, so the mapping will have to map normalized
encoding names to module names.

Contents:

    The following aliases dictionary contains mappings of all IANA
    character set names for which the Python core library provides
    codecs. In addition to these, a few Python specific codec
    aliases have also been added.

"""

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_encodings_aliases_map = _require_intrinsic("molt_encodings_aliases_map")

# The runtime map is generated from config/codec_aliases.toml. Reference data
# belongs to the generator and is not constructed during module initialization.
aliases: dict[str, str] = dict(_molt_encodings_aliases_map())

globals().pop("_require_intrinsic", None)
