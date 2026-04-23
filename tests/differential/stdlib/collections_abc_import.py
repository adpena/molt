"""Purpose: importing _collections_abc must preserve abc class identity."""

import _collections_abc

print(_collections_abc.Hashable.__name__)
