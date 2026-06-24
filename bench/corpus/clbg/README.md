# CLBG kernels (vendored) — molt perf corpus

The Computer Language Benchmarks Game canonical Python kernels, vendored verbatim for molt's benchmark-corpus union (doc 69 §5 task S4; catalog 69A §A.2). These are the recurring kernels at the root of pyperformance / PyPy / Codon suites — registered once in `bench/corpus/registry.toml` and tagged with every source.

- **Upstream:** https://salsa.debian.org/benchmarksgame-team/benchmarksgame
- **Pinned `repo_ref`:** `40296663ed350d5fe4a6ab5e367bab61cb77c219` (site 25.03)
- **License:** Revised BSD (3-clause) — see `LICENSE`. Per clause 3, do not use the CLBG / "Benchmarks Game" names to promote molt; cite results as "CLBG-derived kernels."
- **Fidelity:** extracted byte-exact from the pinned upstream zip and sha256-verified; LF-only (`.gitattributes` enforces `eol=lf`). The body sha256[:16] is recorded below.

## Kernels

| file | upstream zip entry | body sha256[:16] | timed N | Codon tier |
|---|---|---|---|---|
| `nbody.py` | `nbody/nbody.python3` | `101f5f50adf87324` | 50_000_000 | equivalent |
| `fannkuch_redux.py` | `fannkuchredux/fannkuchredux.python3-6.python3` | `d62d0ec075ad5264` | 12 | equivalent |
| `spectral_norm.py` | `spectralnorm/spectralnorm.python3-6.python3` | `bc4cf8abdaa4c49e` | 5500 | equivalent |
| `mandelbrot.py` | `mandelbrot/mandelbrot.python3-3.python3` | `26b2be1bb72913ba` | 16000 | equivalent |
| `fasta.py` | `fasta/fasta.python3` | `f8bc35bb3b1c35fe` | 25_000_000 | equivalent |
| `k_nucleotide.py` | `knucleotide/knucleotide.python3` | `e8da061e093461b7` | stdin=fasta(25M) | equivalent |
| `regex_redux.py` | `regexredux/regexredux.python3` | `955633de90f6d4d4` | stdin=fasta(~5MB) | NON_EQUIVALENT |
| `pidigits.py` | `pidigits/pidigits.python3-4.python3` | `cdfff9f6ad7154f1` | 10000 | NON_EQUIVALENT |
| `reverse_complement.py` | `revcomp/revcomp.python3-2.python3` | `3cdc068eba3f4180` | stdin=fasta(~25MB) | equivalent |
| `binary_trees.py` | `binarytrees/binarytrees.python3-2.python3` | `90069a6b07af6d77` | 21 | equivalent |

## Adapter notes (S4)

- **stdin data dependency:** `k_nucleotide`, `regex_redux`, `reverse_complement` read FASTA from stdin — pipe `fasta.py` output (N=25M for k-nucleotide/rev-comp, ~5MB for regex-redux) or a cached fixture. A real cross-benchmark dependency to wire in the adapter.
- **`regex_redux` uses `multiprocessing.Pool`** as a work distributor (the matching is stdlib `re` — the engine under test). No `multiprocessing`-free pure-Python variant exists upstream at this pin; the adapter must support the pool or run `var_find` single-process — NEVER hand-edit the vendored source (zero-workarounds policy).
- **Codon-NON-equivalent (never scored as a Codon win/loss):** `pidigits` (bignum) and `regex_redux` (engine-dependent).
- **Two N scales:** the CLBG timed-N above and a smaller pyperformance-style N for fast CI.
