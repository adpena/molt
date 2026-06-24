#!/usr/bin/env python3
"""Deterministic perf-red attribution from cycle symbols to missing IR facts.

The perf scoreboard measures *that* a cell is red. This module owns the first
machine-checkable answer to "which compiler fact would make this red class
unexpressible as slow?" It consumes cycle-profile symbols when available and
falls back to the benchmark taxonomy only when no cycle evidence exists.
"""

from __future__ import annotations

import argparse
import json
from collections.abc import Iterable, Mapping
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from typing import Any

import perf_schema


@dataclass(frozen=True)
class CausalityRule:
    fact_class: str
    suspected_missing_fact: str
    symbol_needles: tuple[str, ...]
    benchmark_needles: tuple[str, ...]
    pypy_advantage_class: str | None = None
    reference_class: str | None = None
    codon_semantics: str | None = None


@dataclass(frozen=True)
class PerfAttribution:
    benchmark: str
    fact_class: str
    suspected_missing_fact: str
    attribution_confidence: float
    evidence_symbols: tuple[str, ...]
    evidence_samples: int
    total_samples: int
    source: str
    pypy_advantage_class: str | None = None
    reference_class: str | None = None
    codon_semantics: str | None = None
    evidence_sources: tuple[str, ...] = ()
    pass_delta_score: int = 0
    pass_delta_passes: tuple[str, ...] = ()
    pass_delta_fact_classes: tuple[str, ...] = ()
    call_fact_attached: int | None = None
    call_fact_transient: int | None = None

    def to_json(self) -> dict[str, Any]:
        return asdict(self)


CAUSALITY_RULES: tuple[CausalityRule, ...] = (
    CausalityRule(
        fact_class="exception_region",
        suspected_missing_fact="ExceptionRegion/ownership",
        symbol_needles=(
            "molt_runtime::builtins::exceptions",
            "molt_raise",
            "molt_exception",
            "record_exception",
            "exception_stack",
            "clear_exception",
            "inc_ref_obj",
            "dec_ref_obj",
        ),
        benchmark_needles=("exception", "raise", "try", "except"),
        pypy_advantage_class="borrow_inference",
        reference_class="dynamic",
        codon_semantics="non_equivalent",
    ),
    CausalityRule(
        fact_class="shape_facts",
        suspected_missing_fact="ShapeFacts/string-repr",
        symbol_needles=(
            "split_field",
            "utf8chunks",
            "dataclass_new",
            "attr_name_bits",
            "molt_string_split_field",
            "from_utf8_lossy",
            "alloc_string",
        ),
        benchmark_needles=("etl", "orders", "record", "row", "csv", "field", "split"),
        pypy_advantage_class="shape_propagation",
        reference_class="static_equiv",
        codon_semantics="non_equivalent",
    ),
    CausalityRule(
        fact_class="ownership_lattice",
        suspected_missing_fact="ownership lattice/Repr",
        symbol_needles=(
            "inc_ref",
            "dec_ref",
            "alloc_object",
            "malloc",
            "mi_heap",
            "mi_page",
        ),
        benchmark_needles=("alloc", "object", "refcount", "borrow"),
        pypy_advantage_class="borrow_inference",
        reference_class="dynamic",
        codon_semantics="non_equivalent",
    ),
    CausalityRule(
        fact_class="call_facts",
        suspected_missing_fact="CallFacts/typed callable target",
        symbol_needles=(
            "generic_call",
            "bound_method",
            "method",
            "call_function",
            "guard_type",
            "molt_call",
        ),
        benchmark_needles=("attr", "method", "dispatch", "class"),
        pypy_advantage_class="ic_tiering",
        reference_class="dynamic",
        codon_semantics="non_equivalent",
    ),
    CausalityRule(
        fact_class="repr_tir_type_lattice",
        suspected_missing_fact="Repr/TirType numeric lane",
        symbol_needles=(
            "to_bigint",
            "bigint",
            "box_int",
            "unbox_int",
            "int_subclass",
            "index_bigint",
        ),
        benchmark_needles=("fib", "loop", "sum", "range", "sieve", "matrix", "numeric"),
        pypy_advantage_class="loop_specialization",
        reference_class="numeric",
        codon_semantics="equivalent",
    ),
    CausalityRule(
        fact_class="shape_facts",
        suspected_missing_fact="string/bytes Repr + borrowed-view extraction",
        symbol_needles=(
            "ops_string",
            "bytes",
            "bytearray",
            "format_inner",
            "write_str",
        ),
        benchmark_needles=("bytes", "bytearray", "str", "format", "json", "parse"),
        pypy_advantage_class="shape_propagation",
        reference_class="static_equiv",
        codon_semantics="non_equivalent",
    ),
    CausalityRule(
        fact_class="ownership_lattice",
        suspected_missing_fact="frame ownership/resumable-state/fusion",
        symbol_needles=("generator", "yield", "async", "await", "coroutine"),
        benchmark_needles=("generator", "gen", "yield", "async", "await", "coro"),
        pypy_advantage_class="generator_fusion",
        reference_class="dynamic",
        codon_semantics="non_equivalent",
    ),
)

_PASS_DELTA_SIGNAL_FACT_CLASSES: dict[str, str] = {
    "added_box_ops": "repr_tir_type_lattice",
    "call_results_dynbox_delta": "repr_tir_type_lattice",
    "lost_call_results_typed_repr": "repr_tir_type_lattice",
    "added_generic_calls": "call_facts",
    "added_runtime_helper_calls": "call_facts",
    "added_type_guard_ops": "call_facts",
    "added_rc_events": "ownership_lattice",
    "added_heap_alloc_ops": "ownership_lattice",
    "added_exception_events": "exception_region",
}

_COMPATIBLE_PASS_DELTA_CLASSES: dict[str, frozenset[str]] = {
    "shape_facts": frozenset({"repr_tir_type_lattice", "ownership_lattice"}),
    "exception_region": frozenset({"ownership_lattice"}),
}


@dataclass(frozen=True)
class _PassDeltaSupport:
    score: int
    passes: tuple[str, ...]
    fact_classes: tuple[str, ...]


@dataclass(frozen=True)
class _CallFactSupport:
    attached: int
    transient: int


def derive_attribution(
    benchmark: str,
    symbols: Iterable[Mapping[str, Any]] | None = None,
    *,
    pass_delta_dashboard: Mapping[str, Any] | None = None,
    call_fact_coverage: Mapping[str, Any] | None = None,
) -> PerfAttribution:
    """Return a deterministic missing-fact attribution for one benchmark."""

    symbol_rows = _normalize_symbols(symbols or ())
    if symbol_rows:
        score, rule = max(
            (
                (_score_rule(rule, benchmark, symbol_rows), rule)
                for rule in CAUSALITY_RULES
            ),
            key=lambda item: item[0],
        )
        if score.evidence_samples > 0:
            attr = _attribution_from_score(benchmark, rule, score)
            return _join_evidence(
                attr,
                rule,
                pass_delta_dashboard=pass_delta_dashboard,
                call_fact_coverage=call_fact_coverage,
            )

    attr, rule = _fallback_attribution(benchmark)
    return _join_evidence(
        attr,
        rule,
        pass_delta_dashboard=pass_delta_dashboard,
        call_fact_coverage=call_fact_coverage,
    )


def derive_cell_attribution(
    cell: Mapping[str, Any],
    *,
    pass_delta_dashboard: Mapping[str, Any] | None = None,
    call_fact_coverage: Mapping[str, Any] | None = None,
) -> PerfAttribution:
    benchmark = str(cell.get("benchmark") or "unknown")
    return derive_attribution(
        benchmark,
        _profile_symbols_from_cell(cell),
        pass_delta_dashboard=pass_delta_dashboard,
        call_fact_coverage=call_fact_coverage,
    )


def derive_hot_profile_attributions(
    doc: Mapping[str, Any],
    *,
    pass_delta_dashboard: Mapping[str, Any] | None = None,
    call_fact_coverage: Mapping[str, Any] | None = None,
) -> list[PerfAttribution]:
    cells = doc.get("cells")
    if not isinstance(cells, list):
        return []
    return [
        derive_cell_attribution(
            c,
            pass_delta_dashboard=pass_delta_dashboard,
            call_fact_coverage=call_fact_coverage,
        )
        for c in cells
        if isinstance(c, Mapping)
    ]


def _normalize_symbols(
    symbols: Iterable[Mapping[str, Any]],
) -> tuple[dict[str, Any], ...]:
    out: list[dict[str, Any]] = []
    for row in symbols:
        symbol = row.get("symbol")
        if not isinstance(symbol, str) or not symbol:
            continue
        samples = row.get("self_samples", 0)
        if not isinstance(samples, int | float) or isinstance(samples, bool):
            samples = 0
        out.append({"symbol": symbol, "self_samples": int(samples)})
    return tuple(out)


def _profile_symbols_from_cell(cell: Mapping[str, Any]) -> tuple[dict[str, Any], ...]:
    profile = cell.get("profile_result")
    if not isinstance(profile, Mapping):
        profile = cell.get("cycle_profile")
    if not isinstance(profile, Mapping):
        return ()
    for key in ("in_binary_top", "top_symbols"):
        rows = profile.get(key)
        if isinstance(rows, list) and rows:
            return _normalize_symbols(r for r in rows if isinstance(r, Mapping))
    return ()


@dataclass(frozen=True)
class _RuleScore:
    evidence_samples: int
    total_samples: int
    matched_symbols: tuple[str, ...]
    benchmark_match: bool

    def __lt__(self, other: "_RuleScore") -> bool:
        return (
            self.evidence_samples,
            self.benchmark_match,
            len(self.matched_symbols),
        ) < (
            other.evidence_samples,
            other.benchmark_match,
            len(other.matched_symbols),
        )


def _score_rule(
    rule: CausalityRule, benchmark: str, symbols: tuple[dict[str, Any], ...]
) -> _RuleScore:
    matched: list[str] = []
    samples = 0
    needles = tuple(n.lower() for n in rule.symbol_needles)
    for row in symbols:
        symbol = str(row["symbol"])
        lower = symbol.lower()
        if any(needle in lower for needle in needles):
            samples += int(row.get("self_samples") or 0)
            if len(matched) < 8:
                matched.append(symbol)
    return _RuleScore(
        evidence_samples=samples,
        total_samples=sum(int(row.get("self_samples") or 0) for row in symbols),
        matched_symbols=tuple(matched),
        benchmark_match=_benchmark_matches(rule, benchmark),
    )


def _attribution_from_score(
    benchmark: str, rule: CausalityRule, score: _RuleScore
) -> PerfAttribution:
    total = max(1, score.total_samples)
    coverage = score.evidence_samples / total
    confidence = min(0.99, max(0.5, coverage + (0.2 if score.benchmark_match else 0.0)))
    return _make_attribution(
        benchmark=benchmark,
        rule=rule,
        confidence=round(confidence, 4),
        evidence_symbols=score.matched_symbols,
        evidence_samples=score.evidence_samples,
        total_samples=score.total_samples,
        source="cycle_profile",
    )


def _fallback_attribution(benchmark: str) -> tuple[PerfAttribution, CausalityRule]:
    for rule in CAUSALITY_RULES:
        if _benchmark_matches(rule, benchmark):
            return (
                _make_attribution(
                    benchmark=benchmark,
                    rule=rule,
                    confidence=0.35,
                    evidence_symbols=(),
                    evidence_samples=0,
                    total_samples=0,
                    source="benchmark_taxonomy",
                ),
                rule,
            )
    default = CausalityRule(
        fact_class="ownership_lattice",
        suspected_missing_fact="representation/ownership fact",
        symbol_needles=(),
        benchmark_needles=(),
    )
    return (
        _make_attribution(
            benchmark=benchmark,
            rule=default,
            confidence=0.1,
            evidence_symbols=(),
            evidence_samples=0,
            total_samples=0,
            source="unknown",
        ),
        default,
    )


def _make_attribution(
    *,
    benchmark: str,
    rule: CausalityRule,
    confidence: float,
    evidence_symbols: tuple[str, ...],
    evidence_samples: int,
    total_samples: int,
    source: str,
) -> PerfAttribution:
    _assert_schema_vocab(rule)
    return PerfAttribution(
        benchmark=benchmark,
        fact_class=rule.fact_class,
        suspected_missing_fact=rule.suspected_missing_fact,
        attribution_confidence=confidence,
        evidence_symbols=evidence_symbols,
        evidence_samples=evidence_samples,
        total_samples=total_samples,
        source=source,
        pypy_advantage_class=rule.pypy_advantage_class,
        reference_class=rule.reference_class,
        codon_semantics=rule.codon_semantics,
        evidence_sources=(source,),
    )


def _join_evidence(
    attr: PerfAttribution,
    rule: CausalityRule,
    *,
    pass_delta_dashboard: Mapping[str, Any] | None,
    call_fact_coverage: Mapping[str, Any] | None,
) -> PerfAttribution:
    evidence_sources = list(attr.evidence_sources or (attr.source,))
    confidence = attr.attribution_confidence
    pass_delta = _pass_delta_support(attr.benchmark, pass_delta_dashboard)
    call_facts = _call_fact_support(rule, call_fact_coverage)

    updates: dict[str, Any] = {}
    if pass_delta is not None:
        evidence_sources.append("pass_delta")
        confidence += 0.15 if _pass_delta_supports_rule(rule, pass_delta) else 0.05
        updates.update(
            pass_delta_score=pass_delta.score,
            pass_delta_passes=pass_delta.passes,
            pass_delta_fact_classes=pass_delta.fact_classes,
        )
    if call_facts is not None:
        evidence_sources.append("call_fact_census")
        if call_facts.transient > 0:
            confidence += 0.1
        updates.update(
            call_fact_attached=call_facts.attached,
            call_fact_transient=call_facts.transient,
        )
    unique_sources = tuple(dict.fromkeys(evidence_sources))
    updates["evidence_sources"] = unique_sources
    updates["source"] = "+".join(unique_sources)
    updates["attribution_confidence"] = round(min(0.99, confidence), 4)
    return replace(attr, **updates)


def _pass_delta_support(
    benchmark: str,
    dashboard: Mapping[str, Any] | None,
) -> _PassDeltaSupport | None:
    if not isinstance(dashboard, Mapping):
        return None
    selected = _risk_record_support(benchmark, dashboard)
    if not selected:
        selected = _by_pass_support(benchmark, dashboard)
    if not selected:
        return None
    selected.sort(key=lambda item: (-item[0], item[1], item[2]))
    fact_classes_seen = {
        fact_class for _score, _pass_name, classes in selected for fact_class in classes
    }
    return _PassDeltaSupport(
        score=sum(score for score, _pass_name, _classes in selected),
        passes=tuple(pass_name for _score, pass_name, _classes in selected[:8]),
        fact_classes=tuple(sorted(fact_classes_seen)),
    )


def _risk_record_support(
    benchmark: str, dashboard: Mapping[str, Any]
) -> list[tuple[int, str, tuple[str, ...]]]:
    rows = dashboard.get("risk_records")
    if not isinstance(rows, list):
        return []
    benchmark_stem = Path(benchmark).stem.lower()
    scoped = _dashboard_is_scoped_to_benchmark(dashboard, benchmark_stem)
    selected: list[tuple[int, str, tuple[str, ...]]] = []

    for row in rows:
        if not isinstance(row, Mapping):
            continue
        if not scoped and not _risk_record_matches(row, benchmark_stem):
            continue
        row_classes = _risk_record_fact_classes(row)
        if not row_classes:
            continue
        score = _risk_record_score(row)
        if score <= 0:
            continue
        selected.append((score, str(row.get("pass") or "<unknown>"), row_classes))
    return selected


def _by_pass_support(
    benchmark: str, dashboard: Mapping[str, Any]
) -> list[tuple[int, str, tuple[str, ...]]]:
    rows = dashboard.get("by_pass")
    if not isinstance(rows, list):
        return []
    benchmark_stem = Path(benchmark).stem.lower()
    scoped = _dashboard_is_scoped_to_benchmark(dashboard, benchmark_stem)
    selected: list[tuple[int, str, tuple[str, ...]]] = []

    for row in rows:
        if not isinstance(row, Mapping):
            continue
        if not scoped and not _pass_row_matches(row, benchmark_stem):
            continue
        row_classes = _by_pass_fact_classes(row)
        if not row_classes:
            continue
        score = _pass_row_score(row)
        if score <= 0:
            continue
        selected.append((score, str(row.get("pass") or "<unknown>"), row_classes))
    return selected


def _dashboard_is_scoped_to_benchmark(
    dashboard: Mapping[str, Any], benchmark_stem: str
) -> bool:
    if not benchmark_stem:
        return False
    filters = dashboard.get("filters")
    if not isinstance(filters, Mapping):
        return False
    for key in ("benchmark", "function"):
        value = filters.get(key)
        if isinstance(value, str) and (
            benchmark_stem in Path(value).stem.lower()
            or benchmark_stem in value.lower()
        ):
            return True
    return False


def _risk_record_matches(row: Mapping[str, Any], benchmark_stem: str) -> bool:
    if not benchmark_stem:
        return True
    for key in ("benchmark", "function"):
        value = row.get(key)
        if isinstance(value, str) and benchmark_stem in value.lower():
            return True
    return False


def _risk_record_fact_classes(row: Mapping[str, Any]) -> tuple[str, ...]:
    fact_classes: set[str] = set()
    signals = row.get("signals")
    if isinstance(signals, Mapping):
        for field, value in signals.items():
            if _int(value) > 0 and field in _PASS_DELTA_SIGNAL_FACT_CLASSES:
                fact_classes.add(_PASS_DELTA_SIGNAL_FACT_CLASSES[field])
    lost_repr = row.get("lost_repr_values")
    if isinstance(lost_repr, Mapping) and any(
        _int(value) > 0 for value in lost_repr.values()
    ):
        fact_classes.add("repr_tir_type_lattice")
    return tuple(sorted(fact_classes))


def _risk_record_score(row: Mapping[str, Any]) -> int:
    score = _nonnegative_int(row.get("score"))
    if score > 0:
        return score
    total = 0
    signals = row.get("signals")
    if isinstance(signals, Mapping):
        total += sum(_nonnegative_int(value) for value in signals.values())
    lost_repr = row.get("lost_repr_values")
    if isinstance(lost_repr, Mapping):
        total += sum(_nonnegative_int(value) for value in lost_repr.values())
    return total


def _by_pass_fact_classes(row: Mapping[str, Any]) -> tuple[str, ...]:
    fact_classes = {
        _PASS_DELTA_SIGNAL_FACT_CLASSES[field]
        for field in _PASS_DELTA_SIGNAL_FACT_CLASSES
        if _positive_signal(row, field)
    }
    lost_repr = row.get("lost_repr_values")
    if isinstance(lost_repr, Mapping) and any(
        _int(value) > 0 for value in lost_repr.values()
    ):
        fact_classes.add("repr_tir_type_lattice")
    return tuple(sorted(fact_classes))


def _pass_row_score(row: Mapping[str, Any]) -> int:
    score = _nonnegative_int(row.get("score"))
    if score > 0:
        return score
    total = 0
    for field in _PASS_DELTA_SIGNAL_FACT_CLASSES:
        if field == "lost_call_results_typed_repr":
            total += max(0, -_int(row.get("call_results_typed_repr_delta")))
        else:
            total += _nonnegative_int(row.get(field))
    lost_repr = row.get("lost_repr_values")
    if isinstance(lost_repr, Mapping):
        total += sum(_nonnegative_int(value) for value in lost_repr.values())
    return total


def _pass_delta_supports_rule(rule: CausalityRule, support: _PassDeltaSupport) -> bool:
    compatible = {rule.fact_class}
    compatible.update(_COMPATIBLE_PASS_DELTA_CLASSES.get(rule.fact_class, frozenset()))
    return any(fact_class in compatible for fact_class in support.fact_classes)


def _pass_row_matches(row: Mapping[str, Any], benchmark_stem: str) -> bool:
    functions = row.get("functions")
    if not isinstance(functions, list) or not benchmark_stem:
        return True
    return any(benchmark_stem in str(function).lower() for function in functions)


def _positive_signal(row: Mapping[str, Any], field: str) -> bool:
    if field == "lost_call_results_typed_repr":
        return _int(row.get("call_results_typed_repr_delta")) < 0
    return _int(row.get(field)) > 0


def _call_fact_support(
    rule: CausalityRule, coverage: Mapping[str, Any] | None
) -> _CallFactSupport | None:
    if rule.fact_class != "call_facts":
        return None
    if not isinstance(coverage, Mapping):
        return None
    census = coverage.get("census")
    if isinstance(census, Mapping):
        coverage = census
    stale = coverage.get("stale_evidence")
    if isinstance(stale, list) and stale:
        return None
    attached = coverage.get("attached")
    transient = coverage.get("transient")
    if not isinstance(attached, int) or isinstance(attached, bool):
        return None
    if not isinstance(transient, int) or isinstance(transient, bool):
        return None
    return _CallFactSupport(attached=attached, transient=transient)


def _nonnegative_int(value: Any) -> int:
    parsed = _int(value)
    return parsed if parsed > 0 else 0


def _int(value: Any) -> int:
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return value
    return 0


def _assert_schema_vocab(rule: CausalityRule) -> None:
    if rule.fact_class not in perf_schema.FACT_CLASSES:
        raise AssertionError(f"unknown fact class: {rule.fact_class}")
    if (
        rule.pypy_advantage_class is not None
        and rule.pypy_advantage_class not in perf_schema.PYPY_ADVANTAGE_CLASSES
    ):
        raise AssertionError(f"unknown pypy advantage: {rule.pypy_advantage_class}")
    if (
        rule.reference_class is not None
        and rule.reference_class not in perf_schema.REFERENCE_CLASSES
    ):
        raise AssertionError(f"unknown reference class: {rule.reference_class}")
    if (
        rule.codon_semantics is not None
        and rule.codon_semantics not in perf_schema.CODON_SEMANTICS
    ):
        raise AssertionError(f"unknown codon semantics: {rule.codon_semantics}")


def _benchmark_matches(rule: CausalityRule, benchmark: str) -> bool:
    name = Path(benchmark).stem.lower()
    return any(needle in name for needle in rule.benchmark_needles)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="derive missing-IR-fact attribution from a hot-profile board"
    )
    parser.add_argument("profile", type=Path, help="hot_profile_*.json")
    parser.add_argument(
        "--pass-delta-dashboard",
        type=Path,
        help="optional JSON output from tools/pass_delta_dashboard.py --json",
    )
    parser.add_argument(
        "--call-fact-coverage",
        type=Path,
        help="optional JSON output from tools/call_fact_coverage.py --json",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON")
    ns = parser.parse_args(argv)

    try:
        doc = _read_json_mapping(ns.profile)
        pass_delta_dashboard = _read_optional_json(ns.pass_delta_dashboard)
        call_fact_coverage = _read_optional_json(ns.call_fact_coverage)
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        print(f"cannot read input: {exc}")
        return 2
    attributions = derive_hot_profile_attributions(
        doc,
        pass_delta_dashboard=pass_delta_dashboard,
        call_fact_coverage=call_fact_coverage,
    )
    if ns.json:
        print(json.dumps([a.to_json() for a in attributions], indent=2))
    else:
        for attr in attributions:
            print(
                f"{attr.benchmark}: {attr.fact_class} -> "
                f"{attr.suspected_missing_fact} "
                f"(confidence={attr.attribution_confidence:.2f}, "
                f"source={attr.source})"
            )
    return 0 if attributions else 1


def _read_optional_json(path: Path | None) -> Mapping[str, Any] | None:
    if path is None:
        return None
    return _read_json_mapping(path)


def _read_json_mapping(path: Path) -> Mapping[str, Any]:
    doc = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(doc, Mapping):
        raise ValueError(f"{path}: expected a JSON object")
    return doc


if __name__ == "__main__":
    raise SystemExit(main())
