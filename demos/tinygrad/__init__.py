"""Molt tinygrad demos and model-serving apps.

These modules are application/model/driver code that CONSUMES the core
``tinygrad`` tensor primitive (``from tinygrad.tensor import Tensor``) and the
``molt.gpu`` runtime — they are not part of the compiler substrate. They live
here (outside ``src/molt/``) so the compiler wheel does not ship demo/app code
and so this tree is trivially extractable into a future standalone
``molt-demos`` repository.

Cross-demo imports use relative imports within this package; imports of the
compiler-owned tensor core use the external ``tinygrad.*`` package name, exactly
as a downstream user application would.
"""
