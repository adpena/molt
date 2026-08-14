"""OwnershipLoweringMixin: canonical RC operation construction."""

from __future__ import annotations

from typing import TYPE_CHECKING

from molt.frontend._types import MoltOp, MoltValue

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class OwnershipLoweringMixin(_MixinBase):
    def _emit_inc_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="INC_REF", args=[value], result=res))
        return res

    def _emit_dec_ref(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=res))
        return res

    def _emit_drop_owned_value(self, value: MoltValue | None) -> None:
        if value is None or value.name == "none":
            return
        self.emit(MoltOp(kind="DEC_REF", args=[value], result=MoltValue("none")))

    def _emit_borrow(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="BORROW", args=[value], result=res))
        return res

    def _emit_release(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=value.type_hint)
        self.emit(MoltOp(kind="RELEASE", args=[value], result=res))
        return res

    def _emit_box(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="BOX", args=[value], result=res))
        return res

    def _emit_unbox(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="UNBOX", args=[value], result=res))
        return res

    def _emit_cast(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="CAST", args=[value], result=res))
        return res

    def _emit_widen(self, value: MoltValue, *, hint: str | None = None) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint or value.type_hint)
        self.emit(MoltOp(kind="WIDEN", args=[value], result=res))
        return res
