"""Top-level alias for the pure-Python ``xml.etree.ElementTree`` module.

In CPython, ``_elementtree`` is the C accelerator that shadows the
pure-Python implementation when available.  Molt does not ship a
C accelerator; this module simply re-exports the public API from
``xml.etree.ElementTree`` so direct importers (CPython tests,
third-party code that imports ``_elementtree`` directly) continue to
function.
"""

from xml.etree.ElementTree import (
    Comment,
    Element,
    ElementTree,
    ParseError,
    PI,
    ProcessingInstruction,
    QName,
    SubElement,
    TreeBuilder,
    XML,
    XMLID,
    XMLParser,
    XMLPullParser,
    canonicalize,
    dump,
    fromstring,
    fromstringlist,
    indent,
    iselement,
    iterparse,
    parse,
    register_namespace,
    tostring,
    tostringlist,
)

__all__ = [
    "Comment",
    "Element",
    "ElementTree",
    "ParseError",
    "PI",
    "ProcessingInstruction",
    "QName",
    "SubElement",
    "TreeBuilder",
    "XML",
    "XMLID",
    "XMLParser",
    "XMLPullParser",
    "canonicalize",
    "dump",
    "fromstring",
    "fromstringlist",
    "indent",
    "iselement",
    "iterparse",
    "parse",
    "register_namespace",
    "tostring",
    "tostringlist",
]


# CPython _elementtree exposes a private hook used by ElementTree to install
# the Comment / ProcessingInstruction factories on the C-level Element type.
# Our Element is pure Python; this hook is a no-op that exists for API parity.
def _set_factories(comment_factory, pi_factory):  # noqa: D401
    """API-compat shim; pure-Python Element does not need factory injection."""
    return None
