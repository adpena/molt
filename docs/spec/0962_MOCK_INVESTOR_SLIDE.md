# Molt — Performance That Avoids Rewrites
**Mock Investor Slide (Single Slide Content)**

---

## Headline
**Keep Python. Lose the Rewrite Tax.**

Predictable performance and concurrency for Python services — without switching languages.

---

## Visual Section (Center of Slide)

### Graph 1: P99 Latency — DB-heavy Endpoint
- Log-scale bar chart
- Django / FastAPI / Celery vs **Molt**
- Molt bar highlighted

*(Annotation example: Django 320ms → Molt 42ms)*

---

### Graph 2: Throughput per Core
- Bar chart
- req/sec per core
- Same order as Graph 1

---

### Graph 3: RSS Memory per Worker
- Bar chart
- sustained load
- Molt shows materially lower footprint

---

## Right Column — Why This Matters
- Tail latency defines user experience
- Throughput defines infrastructure cost
- Memory defines stability and density
- All three drive rewrite decisions

---

## Bottom Strip — Operational Simplicity

| Stack | Components |
|-----|------------|
| Django + Celery | Web, Broker, Workers, Scheduler |
| **Molt** | **Single binary** |

---

## Footer
- Same logic, same DB, same hardware
- Metrics defined by Molt Benchmark Contract (0961)
- Canonical metrics defined in Molt Metrics Slide (0960)

---

## Speaker Note (not shown)
> “Most teams don’t rewrite Python because they want to — they do it because the runtime forces them to. Molt removes that pressure.”
