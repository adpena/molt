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
- Official DFlash model/checkpoint collection:
  <https://huggingface.co/collections/z-lab/dflash>
- vLLM Speculators DFlash documentation:
  <https://docs.vllm.ai/projects/speculators/en/latest/user_guide/algorithms/dflash/>
- SGLang/Modal/Z Lab Spec V2 implementation note:
  <https://www.lmsys.org/blog/2026-06-15-next-generation-speculative-decoding-dflash-v2/>
- NVIDIA TensorRT-LLM/vLLM/SGLang deployment note:
  <https://developer.nvidia.com/blog/boost-inference-performance-up-to-15x-on-nvidia-blackwell-using-dflash-speculative-decoding/>
- TensorRT-LLM speculative-decoding feature documentation:
  <https://github.com/NVIDIA/TensorRT-LLM/blob/main/docs/source/features/speculative-decoding.md>
- DDTree follow-on, Liran Ringel and Yaniv Romano, **"Accelerating
  Speculative Decoding with Block Diffusion Draft Trees"**, arXiv:2604.12989:
  <https://arxiv.org/abs/2604.12989>
- DFlare follow-on, Jiebin Zhang et al., **"DFlare: Scaling Up Draft Capacity
  for Block Diffusion Speculative Decoding"**, arXiv:2606.02091:
  <https://arxiv.org/abs/2606.02091>
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

- explicit target model id, draft model id, and adapter provenance/source;
- a target/verifier step that owns correctness and refreshed target
  conditioning;
- a separate drafter step;
- a trained target-specific or explicitly verifier-compatible drafter
  checkpoint, including tokenizer, mask-token, target-layer, target-hidden-state,
  and KV/cross-attention schema metadata;
- target hidden-feature payloads;
- target KV payloads or target-derived KV injection material;
- position IDs for the conditioned draft context;
- the last verified token ID;
- block-parallel draft generation from mask positions in a single drafter
  forward pass, not token-by-token autoregressive drafting;
- non-causal/bidirectional draft attention over target context features and mask
  tokens, projected through the target vocabulary head or an explicitly
  equivalent tied head;
- target-feature injection into every draft layer through KV injection,
  cross-attention, or a versioned successor mechanism that preserves the same
  conditioning invariant; a one-shot input feature blob is not sufficient for a
  DFlash fidelity claim;
- verifier acceptance of the longest valid prefix, with the target-owned next
  token becoming the next round's bonus/root token and with refreshed
  conditioning produced by the verifier;
- explicit failure when no trained adapter/drafter matches the requested target.

The drafter and verifier callables must be distinct at the Molt contract layer;
collapsing both roles into one callable is generic speculative plumbing, not a
DFlash adapter. Test-only adapters must use explicit synthetic `test://...`
model ids and test-only provenance so they cannot be mistaken for production
model support.

The runtime must fail closed when a DFlash-capable backend is requested and no
adapter resolves. Plain greedy fallback is only allowed when DFlash is disabled
or no DFlash-capable backend is selected.

DDTree, DFlare, SGLang Spec V2, vLLM Speculators, TensorRT-LLM DFlash, and
future serving-engine integrations are versioned DFlash-family routes. They may
extend the selection tree, scheduler overlap, layer-wise target fusion, or
hardware execution plan, but they must not relax the core DFlash identity above.
If an implementation uses DDTree or DFlare semantics, the adapter/version must
say so explicitly; plain `DFlash` still means target-conditioned block-diffusion
drafting with verifier-owned lossless acceptance.

## 2026-06 Research Refresh

Fresh public sources reviewed on 2026-06-24/25 change the implementation
routing, not the core identity:

- The original DFlash paper remains the canonical algorithm root: a lightweight
  block-diffusion drafter predicts a draft block in one pass, conditioned on
  target-model context features, and the target model verifies accepted tokens
  losslessly.
- The official z-lab repository now lists trained checkpoints for Gemma 4,
  MiniMax, Kimi, Qwen 3/3.5/3.6, gpt-oss, Qwen Coder, and Llama 3.1 families,
  plus install routes for Transformers, SGLang, vLLM, and MLX. Missing model
  support is therefore a registry/checkpoint issue, not permission to synthesize
  an untrained generic drafter.
- vLLM Speculators describes DFlash as a small diffusion-LLM draft model that
  predicts an entire block in one forward pass, conditioned on target hidden
  states, using non-causal attention over verifier hidden states and mask token
  embeddings. Molt's adapter boundary must preserve that shape.
- SGLang Spec V2 moves DFlash from a research loop into a scheduler/KV-cache
  integration problem: `DFlashWorker`/draft-model execution wraps target
  verification, target latents are projected into draft KV state, and scheduler
  synchronization/overlap become part of performance correctness.
- NVIDIA's June 2026 deployment note reports DFlash paths across TensorRT-LLM,
  vLLM, and SGLang on Blackwell/Hopper-class NVIDIA GPUs. Molt performance
  claims must name the serving stack, GPU, concurrency/interactivity target,
  model pair, draft length, and checkpoint revision; a raw tokens/sec number is
  not a portable DFlash claim.
- DDTree shows that one DFlash pass produces per-position marginals, not
  path-conditioned continuation probabilities. A tree verifier can consume
  those marginals under a node budget, but that is a distinct DFlash-family
  mode and must be exposed as such.
- DFlare attacks a DFlash conditioning bottleneck with per-layer fusion from a
  broader set of target layers. It is evidence that the adapter contract needs
  explicit layer/feature/KV metadata, not a reason to erase target-conditioned
  DFlash requirements.

## Current Scope

The current Molt implementation provides the adapter/runtime contract in
`src/molt/gpu/dflash/`.

Generic speculative-decoding helpers live under
`src/molt/stdlib/tinygrad/speculative.py`. The `tinygrad.dflash` import path
fails closed with an explicit error so generic helpers cannot be mistaken for
paper-faithful DFlash.
