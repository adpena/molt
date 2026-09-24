"""Common base for the SimpleTIRGenerator mixins.

At runtime every visitor/lowering mixin is a plain ``object`` subclass; the
assembled ``SimpleTIRGenerator`` (``molt.frontend``) derives from all of them
and from ``ast.NodeVisitor``. Under ``TYPE_CHECKING`` the base is the assembled
surface: ``ast.NodeVisitor`` first, so its concrete traversal dispatch is what
``self.visit`` / ``self.generic_visit`` resolve to, then the generated
``_GeneratorProtocol`` (tools/gen_protocol.py) for every cross-mixin
``self.<method>`` / ``self.<attr>`` reference. Keeping the protocol behind the
real base class is what lets a checker prove the assembled class is concrete.
"""

from __future__ import annotations

import ast
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

    class GeneratorMixinBase(ast.NodeVisitor, _GeneratorProtocol):
        pass

else:
    GeneratorMixinBase = object


__all__ = ["GeneratorMixinBase"]
