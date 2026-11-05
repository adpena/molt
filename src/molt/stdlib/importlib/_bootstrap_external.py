"""Compatibility surface for CPython ``importlib._bootstrap_external``.

CPython binds this name to the ``_frozen_importlib_external`` module itself.
Molt forwards attribute reads to that module instead of copying its names at
import time: ``_frozen_importlib_external`` imports ``importlib.machinery``,
whose package imports this module while ``_frozen_importlib_external`` may
still be initializing, so either module can be imported first.
"""

import _frozen_importlib_external as _external


def __getattr__(name):
    return getattr(_external, name)


def __dir__():
    return list(_external.__all__)
