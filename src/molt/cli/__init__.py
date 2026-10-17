"""The ``molt`` command-line package.

``main`` runs one command in the calling process and is the package's only
name. Importing ``molt.cli`` or any submodule therefore loads no other Molt
module (HF-227); code imports each name from the submodule that defines it.
``molt.cli.__main__.run`` is the process entry point: the ``molt`` script,
``python -m molt.cli`` and the release bootstrap.
"""

from __future__ import annotations


def main() -> int:
    from molt.cli import entrypoint

    return entrypoint.main()
