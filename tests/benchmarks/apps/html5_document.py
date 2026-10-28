"""Complete HTML5 document parsing through the unmodified html5lib package."""

import json
from pathlib import Path
import sys
from xml.etree import ElementTree

import html5lib


def node_value(node):
    tag = "#comment" if node.tag is ElementTree.Comment else node.tag
    return [
        tag,
        sorted(node.attrib.items()),
        node.text,
        node.tail,
        [node_value(child) for child in node],
    ]


def main():
    source = Path(sys.argv[1]).read_bytes()
    repetitions = int(sys.argv[2])
    if repetitions < 1:
        raise ValueError("repetitions must be positive")
    result = None
    for _ in range(repetitions):
        result = node_value(
            html5lib.parse(source, treebuilder="etree", namespaceHTMLElements=True)
        )
    print(json.dumps(result, ensure_ascii=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
