"""PatternMatchMixin: structural pattern matching (PEP 634 `match`/`case`).

Move-only extraction from frontend/__init__.py (F1 phase 1). Covers visit_Match
and the per-pattern emit helpers (sequence/mapping/class/or/capture), pattern
validation, and the match-local cell/load/store scratch helpers. self.<method>
references resolve through the SimpleTIRGenerator MRO at runtime.
"""

from __future__ import annotations

import ast
from typing import (
    TYPE_CHECKING,
    Callable,
)

from molt.frontend._types import (
    MoltOp,
    MoltValue,
    ScratchCell,
)
from molt.frontend.diagnostics import FrontendDiagnostic as Diagnostic
from molt.frontend.diagnostics import FrontendRejection

if TYPE_CHECKING:
    from molt.frontend._protocol import _GeneratorProtocol

if TYPE_CHECKING:
    _MixinBase = _GeneratorProtocol
else:
    _MixinBase = object


class PatternMatchMixin(_MixinBase):
    def _emit_match_cell(self, initial: bool) -> tuple[MoltValue, MoltValue]:
        initial_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[initial], result=initial_val))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[initial_val], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        return cell, idx

    def _emit_match_load(self, cell: MoltValue, idx: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
        return res

    def _emit_match_store(
        self, cell: MoltValue, idx: MoltValue, value: MoltValue
    ) -> None:
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, value],
                result=MoltValue("none"),
            )
        )

    def _emit_match_and(
        self,
        cell: MoltValue,
        idx: MoltValue,
        compute: Callable[[], MoltValue],
    ) -> None:
        current = self._emit_match_load(cell, idx)
        self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
        result = compute()
        self._emit_match_store(cell, idx, result)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_match_capture(
        self,
        name: str | None,
        value: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        if not name or name == "_":
            return
        temp_cell = capture_map[name]
        current = self._emit_match_load(match_cell, match_idx)
        self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
        self._store_scratch_cell(temp_cell, value)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _match_irrefutable_reason(
        self, pattern: ast.pattern
    ) -> tuple[str, str | None] | None:
        if isinstance(pattern, ast.MatchAs):
            if pattern.pattern is None:
                if pattern.name is None:
                    return ("wildcard", None)
                return ("capture", pattern.name)
            inner = self._match_irrefutable_reason(pattern.pattern)
            if inner is None:
                return None
            if inner[0] == "wildcard":
                return ("wildcard", None)
            return inner
        if isinstance(pattern, ast.MatchOr):
            for sub in pattern.patterns:
                reason = self._match_irrefutable_reason(sub)
                if reason is not None:
                    return reason
        return None

    def _validate_match_pattern(self, pattern: ast.pattern) -> None:
        if isinstance(pattern, ast.MatchOr):
            # CPython requires each OR alternative to bind the same *set* of
            # names (order-independent); _collect_pattern_capture_names now
            # returns a source-ordered list, so compare as sets here.
            bindings = [
                set(self._collect_pattern_capture_names(sub))
                for sub in pattern.patterns
            ]
            if bindings:
                baseline = bindings[0]
                for binding in bindings[1:]:
                    if binding != baseline:
                        self._raise_syntax_error(
                            "alternative patterns bind different names",
                            pattern,
                        )
            for sub in pattern.patterns:
                self._validate_match_pattern(sub)
            return
        if isinstance(pattern, ast.MatchClass):
            seen: set[str] = set()
            for attr in pattern.kwd_attrs:
                if attr in seen:
                    raise SyntaxError(
                        f"attribute name repeated in class pattern: {attr}"
                    )
                seen.add(attr)
            for sub in pattern.patterns:
                self._validate_match_pattern(sub)
            for sub in pattern.kwd_patterns:
                self._validate_match_pattern(sub)
            return
        if isinstance(pattern, ast.MatchSequence):
            for sub in pattern.patterns:
                self._validate_match_pattern(sub)
            return
        if isinstance(pattern, ast.MatchMapping):
            for sub in pattern.patterns:
                self._validate_match_pattern(sub)
            return
        if isinstance(pattern, ast.MatchAs):
            if pattern.pattern is not None:
                self._validate_match_pattern(pattern.pattern)
            return
        if isinstance(pattern, ast.MatchStar):
            return
        if isinstance(pattern, (ast.MatchValue, ast.MatchSingleton)):
            return

    def _emit_match_or(
        self,
        pattern: ast.MatchOr,
        subject: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        capture_names = list(capture_map)
        current = self._emit_match_load(match_cell, match_idx)
        self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
        or_cell, or_idx = self._emit_match_cell(False)
        for sub in pattern.patterns:
            or_current = self._emit_match_load(or_cell, or_idx)
            not_current = self._emit_not(or_current)
            self.emit(MoltOp(kind="IF", args=[not_current], result=MoltValue("none")))
            alt_cell, alt_idx = self._emit_match_cell(True)
            alt_capture_map = {name: self._new_scratch_cell() for name in capture_names}
            self._emit_match_pattern(sub, subject, alt_cell, alt_idx, alt_capture_map)
            alt_result = self._emit_match_load(alt_cell, alt_idx)
            self._emit_match_store(or_cell, or_idx, alt_result)
            if capture_names:
                self.emit(
                    MoltOp(kind="IF", args=[alt_result], result=MoltValue("none"))
                )
                for name in capture_names:
                    alt_val = self._load_scratch_cell(alt_capture_map[name])
                    self._store_scratch_cell(capture_map[name], alt_val)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        or_result = self._emit_match_load(or_cell, or_idx)
        self._emit_match_store(match_cell, match_idx, or_result)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_match_sequence(
        self,
        pattern: ast.MatchSequence,
        subject: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        patterns = list(pattern.patterns)
        star_index = next(
            (idx for idx, sub in enumerate(patterns) if isinstance(sub, ast.MatchStar)),
            None,
        )

        def compute_is_seq() -> MoltValue:
            type_vals = [
                self._emit_builtin_type_value("list"),
                self._emit_builtin_type_value("tuple"),
                self._emit_builtin_type_value("range"),
                self._emit_builtin_type_value("memoryview"),
            ]
            seq_types = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=type_vals, result=seq_types))
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISINSTANCE", args=[subject, seq_types], result=res))
            return res

        self._emit_match_and(match_cell, match_idx, compute_is_seq)

        len_cell = self._new_scratch_cell(type_hint="int")

        def compute_len_cond() -> MoltValue:
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[subject], result=length))
            self._store_scratch_cell(len_cell, length)
            expected = len(patterns) if star_index is None else len(patterns) - 1
            expected_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[expected], result=expected_val))
            cond = MoltValue(self.next_var(), type_hint="bool")
            if star_index is None:
                self.emit(MoltOp(kind="EQ", args=[length, expected_val], result=cond))
            else:
                self.emit(MoltOp(kind="GE", args=[length, expected_val], result=cond))
            return cond

        self._emit_match_and(match_cell, match_idx, compute_len_cond)

        prefix_len = star_index or 0
        suffix_len = 0 if star_index is None else len(patterns) - star_index - 1

        for idx, subpattern in enumerate(patterns):
            if isinstance(subpattern, ast.MatchStar):
                if subpattern.name is None or subpattern.name == "_":
                    continue
                current = self._emit_match_load(match_cell, match_idx)
                self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
                len_val = self._load_scratch_cell(len_cell)
                start_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[prefix_len], result=start_val))
                if suffix_len == 0:
                    end_val = len_val
                else:
                    suffix_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="CONST", args=[suffix_len], result=suffix_val)
                    )
                    end_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="SUB", args=[len_val, suffix_val], result=end_val)
                    )
                slice_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="SLICE",
                        args=[subject, start_val, end_val],
                        result=slice_val,
                    )
                )
                rest_list = self._emit_list_from_iter(slice_val)
                self._store_scratch_cell(capture_map[subpattern.name], rest_list)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                continue

            # Allocate outside the guarded block so false branches retain a
            # deterministic initialized compiler-only cell.
            item_cell = self._new_scratch_cell()
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            len_val = None
            index_val: MoltValue
            if star_index is None or idx < prefix_len:
                index_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[idx], result=index_val))
            else:
                len_val = self._load_scratch_cell(len_cell)
                base_val = len_val
                if suffix_len:
                    suffix_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="CONST", args=[suffix_len], result=suffix_val)
                    )
                    base_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="SUB", args=[len_val, suffix_val], result=base_val)
                    )
                offset = idx - prefix_len - 1
                if offset:
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                    index_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="ADD", args=[base_val, offset_val], result=index_val
                        )
                    )
                else:
                    index_val = base_val
            item_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[subject, index_val], result=item_val))
            self._store_scratch_cell(item_cell, item_val)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            item_loaded = self._load_scratch_cell(item_cell)
            self._emit_match_pattern(
                subpattern, item_loaded, match_cell, match_idx, capture_map
            )

    def _emit_match_mapping(
        self,
        pattern: ast.MatchMapping,
        subject: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        def compute_is_dict() -> MoltValue:
            dict_type = self._emit_builtin_type_value("dict")
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISINSTANCE", args=[subject, dict_type], result=res))
            return res

        self._emit_match_and(match_cell, match_idx, compute_is_dict)

        key_cells: list[ScratchCell] = []
        for key_expr, subpattern in zip(pattern.keys, pattern.patterns):
            key_cell = self._new_scratch_cell()
            value_cell = self._new_scratch_cell()
            key_cells.append(key_cell)
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            key_val = self.visit(key_expr)
            if key_val is None:
                raise FrontendRejection(
                    Diagnostic.SYNTAX_FORM, "Unsupported mapping pattern key"
                )
            self._store_scratch_cell(key_cell, key_val)
            missing = self._emit_missing_value()
            item_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="DICT_GET",
                    args=[subject, key_val, missing],
                    result=item_val,
                )
            )
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[item_val, missing], result=is_missing))
            ok_val = self._emit_not(is_missing)
            self._emit_match_store(match_cell, match_idx, ok_val)
            self._store_scratch_cell(value_cell, item_val)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            item_loaded = self._load_scratch_cell(value_cell)
            self._emit_match_pattern(
                subpattern, item_loaded, match_cell, match_idx, capture_map
            )

        if pattern.rest and pattern.rest != "_":
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            rest_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_COPY", args=[subject], result=rest_dict))
            for key_cell in key_cells:
                key_val = self._load_scratch_cell(key_cell)
                missing = self._emit_missing_value()
                has_default = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=has_default))
                _ = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="DICT_POP",
                        args=[rest_dict, key_val, missing, has_default],
                        result=_,
                    )
                )
            self._store_scratch_cell(capture_map[pattern.rest], rest_dict)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_match_class(
        self,
        pattern: ast.MatchClass,
        subject: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        class_cell = self._new_scratch_cell(type_hint="type")

        def compute_isinstance() -> MoltValue:
            cls_val = self.visit(pattern.cls)
            if cls_val is None:
                raise FrontendRejection(
                    Diagnostic.SYNTAX_FORM, "Unsupported class pattern type"
                )
            self._store_scratch_cell(class_cell, cls_val)
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="ISINSTANCE", args=[subject, cls_val], result=res))
            return res

        self._emit_match_and(match_cell, match_idx, compute_isinstance)

        if not pattern.patterns and not pattern.kwd_patterns:
            return

        # CPython special case: builtin types (int, str, float, bool, bytes,
        # bytearray, complex) accept exactly 1 positional sub-pattern that
        # binds the subject itself — they don't have __match_args__.
        _MATCH_SELF_TYPES = {
            "int",
            "str",
            "float",
            "bool",
            "bytes",
            "bytearray",
            "complex",
        }
        if (
            isinstance(pattern.cls, ast.Name)
            and pattern.cls.id in _MATCH_SELF_TYPES
            and len(pattern.patterns) == 1
            and not pattern.kwd_patterns
        ):
            subpat = pattern.patterns[0]
            if (
                isinstance(subpat, ast.MatchAs)
                and subpat.pattern is None
                and subpat.name
            ):
                self._emit_match_capture(
                    subpat.name, subject, match_cell, match_idx, capture_map
                )
            elif (
                isinstance(subpat, ast.MatchAs)
                and subpat.pattern is None
                and subpat.name is None
            ):
                pass  # wildcard — already matched by isinstance
            else:
                # Nested pattern — emit recursive match against the subject itself
                self._emit_match_pattern(
                    subpat, subject, match_cell, match_idx, capture_map
                )
            return

        pos_count = len(pattern.patterns)
        pos_attr_list = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=pos_attr_list))
        match_args_cell = self._new_scratch_cell(type_hint="tuple")

        if pos_count:
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            cls_val = self._load_scratch_cell(class_cell)
            match_args_key = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=["__match_args__"], result=match_args_key)
            )
            missing = self._emit_missing_value()
            match_args = MoltValue(self.next_var(), type_hint="Any")
            self._emit_getattr_name_default(
                cls_val,
                match_args_key,
                missing,
                match_args,
                literal_name="__match_args__",
            )
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[match_args, missing], result=is_missing))
            placeholder = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=placeholder))
            args_cell = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=args_cell))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
            empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[args_cell, idx, empty_tuple],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            tuple_type = self._emit_builtin_type_value("tuple")
            is_tuple = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="ISINSTANCE", args=[match_args, tuple_type], result=is_tuple
                )
            )
            self.emit(MoltOp(kind="IF", args=[is_tuple], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[args_cell, idx, match_args],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            cls_name_val = self._emit_name_from_obj(cls_val)
            type_name = self._emit_type_name(match_args)
            msg = self._emit_string_join(
                [
                    cls_name_val,
                    self._emit_const_value(".__match_args__ must be a tuple (got "),
                    type_name,
                    self._emit_const_value(")"),
                ]
            )
            self._emit_type_error(msg)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            match_args_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="INDEX", args=[args_cell, idx], result=match_args_val)
            )
            self._store_scratch_cell(match_args_cell, match_args_val)
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[match_args_val], result=length))
            count_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[pos_count], result=count_val))
            too_few = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[length, count_val], result=too_few))
            self.emit(MoltOp(kind="IF", args=[too_few], result=MoltValue("none")))
            cls_name_val = self._emit_name_from_obj(cls_val)
            expected_str = self._emit_str_from_obj(length)
            given_str = self._emit_str_from_obj(count_val)
            msg = self._emit_string_join(
                [
                    cls_name_val,
                    self._emit_const_value("() accepts "),
                    expected_str,
                    self._emit_const_value(" positional sub-patterns ("),
                    given_str,
                    self._emit_const_value(" given)"),
                ]
            )
            self._emit_type_error(msg)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        for idx, subpattern in enumerate(pattern.patterns):
            attr_value_cell = self._new_scratch_cell()
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            match_args_val = self._load_scratch_cell(match_args_cell)
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
            attr_name = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="INDEX", args=[match_args_val, idx_val], result=attr_name)
            )
            str_type = self._emit_builtin_type_value("str")
            is_str = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="ISINSTANCE", args=[attr_name, str_type], result=is_str)
            )
            self.emit(MoltOp(kind="IF", args=[is_str], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="LIST_APPEND",
                    args=[pos_attr_list, attr_name],
                    result=MoltValue("none"),
                )
            )
            missing = self._emit_missing_value()
            attr_val = MoltValue(self.next_var(), type_hint="Any")
            self._emit_getattr_name_default(
                subject,
                attr_name,
                missing,
                attr_val,
                literal_name=None,
            )
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[attr_val, missing], result=is_missing))
            ok_val = self._emit_not(is_missing)
            self._emit_match_store(match_cell, match_idx, ok_val)
            self._store_scratch_cell(attr_value_cell, attr_val)
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            type_name = self._emit_type_name(attr_name)
            msg = self._emit_string_join(
                [
                    self._emit_const_value(
                        "__match_args__ elements must be strings (got "
                    ),
                    type_name,
                    self._emit_const_value(")"),
                ]
            )
            self._emit_type_error(msg)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            attr_loaded = self._load_scratch_cell(attr_value_cell)
            self._emit_match_pattern(
                subpattern, attr_loaded, match_cell, match_idx, capture_map
            )

        for attr_name, subpattern in zip(pattern.kwd_attrs, pattern.kwd_patterns):
            attr_value_cell = self._new_scratch_cell()
            current = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            key_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[attr_name], result=key_val))
            has_dup = self._emit_contains(pos_attr_list, key_val)
            self.emit(MoltOp(kind="IF", args=[has_dup], result=MoltValue("none")))
            cls_val = self._load_scratch_cell(class_cell)
            cls_name_val = self._emit_name_from_obj(cls_val)
            msg = self._emit_string_join(
                [
                    cls_name_val,
                    self._emit_const_value(
                        "() got multiple sub-patterns for attribute '"
                    ),
                    key_val,
                    self._emit_const_value("'"),
                ]
            )
            self._emit_type_error(msg)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            missing = self._emit_missing_value()
            attr_val = MoltValue(self.next_var(), type_hint="Any")
            self._emit_getattr_name_default(
                subject,
                key_val,
                missing,
                attr_val,
                literal_name=attr_name,
            )
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[attr_val, missing], result=is_missing))
            ok_val = self._emit_not(is_missing)
            self._emit_match_store(match_cell, match_idx, ok_val)
            self._store_scratch_cell(attr_value_cell, attr_val)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            attr_loaded = self._load_scratch_cell(attr_value_cell)
            self._emit_match_pattern(
                subpattern, attr_loaded, match_cell, match_idx, capture_map
            )

    def _emit_match_pattern(
        self,
        pattern: ast.pattern,
        subject: MoltValue,
        match_cell: MoltValue,
        match_idx: MoltValue,
        capture_map: dict[str, ScratchCell],
    ) -> None:
        if isinstance(pattern, ast.MatchValue):

            def compute_value() -> MoltValue:
                value = self.visit(pattern.value)
                if value is None:
                    raise FrontendRejection(
                        Diagnostic.SYNTAX_FORM,
                        "Unsupported match value pattern",
                    )
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="EQ", args=[subject, value], result=res))
                return res

            self._emit_match_and(match_cell, match_idx, compute_value)
            return
        if isinstance(pattern, ast.MatchSingleton):
            const_val = self._emit_const_value(pattern.value)

            def compute_singleton() -> MoltValue:
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[subject, const_val], result=res))
                return res

            self._emit_match_and(match_cell, match_idx, compute_singleton)
            return
        if isinstance(pattern, ast.MatchAs):
            if pattern.pattern is None:
                self._emit_match_capture(
                    pattern.name, subject, match_cell, match_idx, capture_map
                )
                return
            self._emit_match_pattern(
                pattern.pattern, subject, match_cell, match_idx, capture_map
            )
            self._emit_match_capture(
                pattern.name, subject, match_cell, match_idx, capture_map
            )
            return
        if isinstance(pattern, ast.MatchOr):
            self._emit_match_or(pattern, subject, match_cell, match_idx, capture_map)
            return
        if isinstance(pattern, ast.MatchSequence):
            self._emit_match_sequence(
                pattern, subject, match_cell, match_idx, capture_map
            )
            return
        if isinstance(pattern, ast.MatchMapping):
            self._emit_match_mapping(
                pattern, subject, match_cell, match_idx, capture_map
            )
            return
        if isinstance(pattern, ast.MatchClass):
            self._emit_match_class(pattern, subject, match_cell, match_idx, capture_map)
            return
        if isinstance(pattern, ast.MatchStar):
            self._emit_match_capture(
                pattern.name, subject, match_cell, match_idx, capture_map
            )
            return
        raise FrontendRejection(Diagnostic.SYNTAX_FORM, "Unsupported match pattern")

    def _raise_syntax_error(self, msg: str, node: ast.AST) -> None:
        """Raise SyntaxError with CPython-compatible line/column info."""
        err = SyntaxError(msg)
        err.filename = self.source_path or "<unknown>"
        err.lineno = getattr(node, "lineno", None)
        err.offset = getattr(node, "col_offset", 0) + 1  # 1-based
        err.end_offset = getattr(node, "end_col_offset", 0) + 1  # 1-based
        # Set text so traceback.format_exception_only can show the source
        # line and carets. We read directly to avoid linecache issues.
        if self.source_path:
            import linecache

            line = linecache.getline(self.source_path, err.lineno or 0)
            if line:
                err.text = line
        raise err

    def visit_Match(self, node: ast.Match) -> None:
        subject = self.visit(node.subject)
        if subject is None:
            raise FrontendRejection(Diagnostic.SYNTAX_FORM, "Unsupported match subject")
        subject_val = subject
        if not self.is_async() and self.current_func_name != "molt_main":
            assigned = self._collect_assigned_names([node])
            for name in sorted(assigned):
                if name not in self.scope_assigned or name in self.closure_locals:
                    self._box_local(name)

        provenance_flow = self._begin_module_provenance_flow(
            record_exception_prefixes=True
        )
        for case in node.cases:
            self._validate_match_pattern(case.pattern)
        done_cell, done_idx = self._emit_match_cell(False)
        for idx, case in enumerate(node.cases):
            is_last = idx == len(node.cases) - 1
            if case.guard is None and not is_last:
                reason = self._match_irrefutable_reason(case.pattern)
                if reason is not None:
                    kind, name = reason
                    if kind == "wildcard":
                        self._raise_syntax_error(
                            "wildcard makes remaining patterns unreachable",
                            case.pattern,
                        )
                    if kind == "capture" and name:
                        self._raise_syntax_error(
                            f"name capture '{name}' makes remaining patterns unreachable",
                            case.pattern,
                        )

            capture_names = sorted(self._collect_pattern_capture_names(case.pattern))
            capture_map = {name: self._new_scratch_cell() for name in capture_names}
            done_val = self._emit_match_load(done_cell, done_idx)
            not_done = self._emit_not(done_val)
            self.emit(MoltOp(kind="IF", args=[not_done], result=MoltValue("none")))
            match_cell, match_idx = self._emit_match_cell(True)
            self._emit_match_pattern(
                case.pattern, subject_val, match_cell, match_idx, capture_map
            )
            matched = self._emit_match_load(match_cell, match_idx)
            self.emit(MoltOp(kind="IF", args=[matched], result=MoltValue("none")))

            for name in capture_names:
                temp_val = self._load_scratch_cell(capture_map[name])
                self._store_local_value(name, temp_val)

            if case.guard is not None:
                guard_val = self.visit(case.guard)
                if guard_val is None:
                    raise FrontendRejection(
                        Diagnostic.SYNTAX_FORM, "Unsupported match guard"
                    )
                self.emit(MoltOp(kind="IF", args=[guard_val], result=MoltValue("none")))
                self._visit_block(case.body)
                done_true = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done_true))
                self._emit_match_store(done_cell, done_idx, done_true)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            else:
                self._visit_block(case.body)
                done_true = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done_true))
                self._emit_match_store(done_cell, done_idx, done_true)

            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self._finish_module_provenance_flow(provenance_flow)
        return None
