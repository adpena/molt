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
from dataclasses import asdict, dataclass
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


def derive_attribution(
    benchmark: str, symbols: Iterable[Mapping[str, Any]] | None = None
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
            return _attribution_from_score(benchmark, rule, score)

    return _fallback_attribution(benchmark)


def derive_cell_attribution(cell: Mapping[str, Any]) -> PerfAttribution:
    benchmark = str(cell.get("benchmark") or "unknown")
    return derive_attribution(benchmark, _profile_symbols_from_cell(cell))


def derive_hot_profile_attributions(doc: Mapping[str, Any]) -> list[PerfAttribution]:
    cells = doc.get("cells")
    if not isinstance(cells, list):
        return []
    return [derive_cell_attribution(c) for c in cells if isinstance(c, Mapping)]


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


def _fallback_attribution(benchmark: str) -> PerfAttribution:
    for rule in CAUSALITY_RULES:
        if _benchmark_matches(rule, benchmark):
            return _make_attribution(
                benchmark=benchmark,
                rule=rule,
                confidence=0.35,
                evidence_symbols=(),
                evidence_samples=0,
                total_samples=0,
                source="benchmark_taxonomy",
            )
    default = CausalityRule(
        fact_class="ownership_lattice",
        suspected_missing_fact="representation/ownership fact",
        symbol_needles=(),
        benchmark_needles=(),
    )
    return _make_attribution(
        benchmark=benchmark,
        rule=default,
        confidence=0.1,
        evidence_symbols=(),
        evidence_samples=0,
        total_samples=0,
        source="unknown",
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
    )


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
    parser.add_argument("--json", action="store_true", help="emit JSON")
    ns = parser.parse_args(argv)

    try:
        doc = json.loads(ns.profile.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"cannot read {ns.profile}: {exc}")
        return 2
    attributions = derive_hot_profile_attributions(doc)
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


if __name__ == "__main__":
    raise SystemExit(main())
