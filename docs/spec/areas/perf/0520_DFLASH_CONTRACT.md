# DFlash Contract And Provenance

**Status:** Active contract
**Owner:** runtime/gpu

## Provenance

Molt's DFlash support is derived from these public sources:

- Jian Chen, Yesheng Liang, and Zhijian Liu, **"DFlash: Block Diffusion for
  Flash Speculative Decoding"**, arXiv:2602.06036:
  <https://arxiv.org/abs/2602.06036>
- Official DFlash project page, Z Lab:
  <https://z-lab.ai/projects/dflash/>
- Official project repository:
  <https://github.com/z-lab/dflash>
- Baseline speculative decoding provenance:
  Yaniv Leviathan, Matan Kalman, and Yossi Matias, **"Fast Inference from
  Transformers via Speculative Decoding"**, ICML 2023:
  <https://proceedings.mlr.press/v202/leviathan23a>
- Baseline speculative sampling provenance:
  Charlie Chen, Sebastian Borgeaud, Geoffrey Irving, Jean-Baptiste Lespiau,
  Laurent Sifre, and John Jumper, **"Accelerating Large Language Model
  Decoding with Speculative Sampling"**, arXiv:2302.01318:
  <https://arxiv.org/abs/2302.01318>

## Non-Negotiable Contract

DFlash in Molt must mean target-conditioned block-diffusion drafting, not a
generic speculative-decoding loop.

A DFlash adapter must provide:

- a target/verifier step that owns correctness and refreshed target
  conditioning;
- a separate drafter step;
- target hidden-feature payloads;
- target KV payloads or target-derived KV injection material;
- position IDs for the conditioned draft context;
- the last verified token ID;
- explicit failure when no trained adapter/drafter matches the requested target.

The runtime must fail closed when a DFlash-capable backend is requested and no
adapter resolves. Plain greedy fallback is only allowed when DFlash is disabled
or no DFlash-capable backend is selected.

## Current Scope

The current Molt implementation provides the adapter/runtime contract in
`src/molt/gpu/dflash/`.

Generic speculative-decoding helpers live under
`src/molt/stdlib/tinygrad/speculative.py`. The `tinygrad.dflash` import path
fails closed with an explicit error so generic helpers cannot be mistaken for
paper-faithful DFlash.
