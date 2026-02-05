"""Purpose: stdlib import smoke for codecs/encodings."""

import codecs
import encodings
import copyreg

modules = [
    codecs,
    encodings,
    copyreg,
]
print([mod.__name__ for mod in modules])
