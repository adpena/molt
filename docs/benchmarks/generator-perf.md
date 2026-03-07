# Generator Benchmark: Molt-compiled vs CPython

**Date:** 2026-03-06 21:25
**Workload:** `site/world_engine/generator.py` (3D noise procedural zone generation)
**Iterations:** 5
**CPython:** Python 3.12.13
**Platform:** darwin (arm64)

## Results

| Metric | CPython | Molt Native | Ratio |
|--------|--------:|------------:|------:|
| Wall time (mean) | 42.0 ms | 114.0 ms | 0.37x |
| Wall time (min) | 30.0 ms | 100.0 ms |  |
| Wall time (max) | 60.0 ms | 130.0 ms |  |
| User time (mean) | 22.0 ms | 94.0 ms |  |
| Peak RSS (mean) | 17.3 MB | 48.3 MB | 0.36x |
| Peak RSS (min) | 17.1 MB | 48.3 MB |  |
| Peak RSS (max) | 17.3 MB | 48.4 MB |  |

## Summary

- Molt is **0.37x slower** than CPython (wall-clock mean)
- Molt uses **2.80x** the memory of CPython (peak RSS mean)

## Raw Data

### CPython runs
```
  run 1: wall=30.0 ms  rss=17.3 MB
  run 2: wall=60.0 ms  rss=17.2 MB
  run 3: wall=50.0 ms  rss=17.3 MB
  run 4: wall=30.0 ms  rss=17.3 MB
  run 5: wall=40.0 ms  rss=17.1 MB
```

### Molt runs
```
  run 1: wall=120.0 ms  rss=48.3 MB
  run 2: wall=110.0 ms  rss=48.3 MB
  run 3: wall=110.0 ms  rss=48.4 MB
  run 4: wall=100.0 ms  rss=48.4 MB
  run 5: wall=130.0 ms  rss=48.3 MB
```
