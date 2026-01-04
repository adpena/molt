# Molt “Modern Core Pandas” Compatibility Matrix (Dream → Plan)
**Spec ID:** 0503
**Status:** Draft (living document)
**Audience:** product owners, implementers, test authors, AI coding agents
**Goal:** Provide a staged, testable target for “modern core pandas compatibility” without promising historical/legacy quirks.

## 0. Important framing
This matrix targets **modern core pandas behaviors**:
- stable, widely used DataFrame/Series operations
- avoids deprecated APIs and legacy edge-case commitments
- correctness is enforced via **differential testing** vs pandas as the oracle

This document is a roadmap, not a marketing promise.

## 1. Tiers
- DF0: FastFrame (production fast path)
- DF1: Pandas-ish (migration)
- DF2: Modern Core Pandas (long-term)

## 2. Categories and staging
Legend:
- ✅ = planned/required
- 🟡 = optional/phase-in
- ❌ = out-of-scope (or requires explicit slow path)

### 2.1 Construction and basics
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| DataFrame from dict/arrays | ✅ | ✅ | ✅ |
| DataFrame from Arrow | ✅ | ✅ | ✅ |
| Series basics | ✅ | ✅ | ✅ |
| Column selection `df[col]` | ✅ | ✅ | ✅ |
| `df[['a','b']]` | ✅ | ✅ | ✅ |
| `assign`, `rename`, `drop` | ✅ | ✅ | ✅ |
| `astype` (basic casts) | ✅ | ✅ | ✅ |

### 2.2 Filtering and boolean logic
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| boolean mask filtering | ✅ | ✅ | ✅ |
| `query` string language | ❌ | 🟡 | 🟡 |

### 2.3 Missing data
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| null bitmap semantics | ✅ | ✅ | ✅ |
| `fillna`, `dropna` | ✅ | ✅ | ✅ |
| pandas NA edge cases | 🟡 | ✅ | ✅ |

### 2.4 Groupby and aggregation
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| groupby keys | ✅ | ✅ | ✅ |
| agg: count/sum/mean/min/max | ✅ | ✅ | ✅ |
| agg: nunique/median/quantile | 🟡 | 🟡 | ✅ |
| groupby apply (Python UDF) | ❌ | 🟡 (slow) | 🟡 (slow) |

### 2.5 Joins / merge
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| inner/left join | ✅ | ✅ | ✅ |
| outer join | 🟡 | ✅ | ✅ |
| asof join | ❌ | 🟡 | 🟡 |
| join with complex index alignment | ❌ | 🟡 | ✅ |

### 2.6 Sorting
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| `sort_values` | ✅ | ✅ | ✅ |
| `sort_index` (simple) | 🟡 | ✅ | ✅ |
| stable sort guarantees | ✅ (config) | ✅ | ✅ |

### 2.7 String ops
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| contains/starts/ends/replace | 🟡 | ✅ | ✅ |
| regex heavy semantics | ❌ | 🟡 | 🟡 |

### 2.8 Datetime
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| naive datetime | 🟡 | ✅ | ✅ |
| timezone-aware | ❌ | 🟡 | ✅ |

### 2.9 Index semantics (the big dragon)
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| no index / row-id index | ✅ | ✅ | ✅ |
| single-level index | 🟡 | ✅ | ✅ |
| alignment on arithmetic | ❌ | 🟡 | ✅ |
| MultiIndex | ❌ | 🟡 | 🟡 |

### 2.10 Object dtype
| Feature | DF0 | DF1 | DF2 |
|---|---:|---:|---:|
| object dtype default | ❌ | ❌ | ❌ |
| object dtype opt-in | 🟡 (slow) | ✅ (slow) | ✅ (slow) |

## 3. Policy: “fast mode” vs “compat mode”
Molt DataFrame must expose a policy switch:
- **fast mode (DF0)**: refuses semantics that sabotage performance
- **compat mode (DF1/DF2)**: enables more pandas behaviors, possibly slower

## 4. Measuring “core pandas” scope
We define “core pandas” operationally by:
- usage-driven telemetry from real repos (optional)
- public API stability and deprecations (tracked manually)
- a curated test suite representing modern usage patterns

## 5. Exit criteria for DF2 claim
Molt can claim “modern core pandas compatibility” only when:
- curated test suite passes against pandas oracle
- major behavioral divergences are documented
- performance baseline targets are met for core ops
