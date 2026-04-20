# Falcon-OCR Production Launch Guide

## Architecture

- **Browser**: Falcon-OCR WASM (3.9 MB) + INT4 weights (129 MB, cached in IndexedDB)
- **Edge**: Falcon-OCR CPU on Cloudflare Workers (INT4 sharded, 5-min CPU budget)
- **Fallback**: PaddleOCR (99.6% accuracy, no GPU needed)
- **NL Fill**: Workers AI (Llama 3.2 3B) for natural language invoice filling
- **Payment**: x402 USDC on Base ($0.001/request)

## Deployment Steps

1. Deploy the Worker:
   ```bash
   wrangler deploy --config deploy/cloudflare/wrangler.toml
   ```

2. Verify health:
   ```bash
   curl https://falcon-ocr.adpena.workers.dev/health
   ```

3. Verify API access (with required User-Agent):
   ```bash
   curl -X POST https://falcon-ocr.adpena.workers.dev/ocr \
     -H "Content-Type: application/json" \
     -H "User-Agent: FalconOCR-Client/1.0" \
     -H "X-Payment-402: <payment-proof>" \
     -d '{"image": "<base64>"}'
   ```

4. Monitor: Cloudflare Analytics dashboard

## CF Bot Protection

API clients **must** set a proper User-Agent header to avoid Cloudflare Bot Protection (error 1010, HTTP 403):

```
User-Agent: FalconOCR-Client/1.0
```

Recognized prefixes: `FalconOCR-Client/`, `enjoice/`, `molt-agent/`.

Requests carrying the `X-Payment-402` header are exempt from bot checks regardless of User-Agent.

## Known Limitations

- INT4 model: 64s per token on Workers (1 token per request)
- Micro model: <1s per token (lower quality)
- CF Bot Protection: API clients need proper User-Agent header (see above)
- WASM binary: 3.9 MB gzipped (target was 2 MB)

## Monitoring Checklist

- [ ] Error rate < 1% (Cloudflare Analytics)
- [ ] p95 latency < 5s for micro model
- [ ] Cache hit ratio > 50% after 24h
- [ ] x402 payments flowing to wallet
- [ ] No 403/1010 errors from legitimate API clients
