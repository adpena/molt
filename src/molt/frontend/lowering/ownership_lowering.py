"""OwnershipLoweringMixin: canonical RC operation construction."""

from __future__ import annotations


from molt.frontend._types import MoltOp, MoltValue
from molt.frontend._mixin_base import GeneratorMixinBase


class OwnershipLoweringMixin(GeneratorMixinBase):
    def _emit_drop_owned_value(self, value: MoltValue | None) -> None:
        if value is None or value.name == "none":
            return
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=MoltValue("none")))
