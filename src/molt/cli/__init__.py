"""The ``molt`` command-line package.

``main`` is the console entry point and the package's only name. Importing
``molt.cli`` or any submodule therefore loads no other Molt module (HF-227);
code imports each name from the submodule that defines it.
"""

from __future__ import annotations


def main() -> int:
    from molt.cli import entrypoint

    return entrypoint.main()
