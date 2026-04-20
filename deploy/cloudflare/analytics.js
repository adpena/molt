/**
 * Cloudflare Analytics Engine query helpers.
 *
 * Provides functions for extracting dashboard-quality metrics from the
 * Analytics Engine dataset written by monitoring.js.
 *
 * Usage:
 *   import { getAnalyticsSummary } from "./analytics.js";
 *   const summary = await getAnalyticsSummary(env);
 *
 * All queries are scoped to the Worker's own Analytics Engine dataset
 * (bound as env.ANALYTICS in wrangler.toml).
 */

/**
 * @typedef {{
 *   requests_per_minute: number,
 *   error_rate: number,
 *   latency_p50_ms: number,
 *   latency_p95_ms: number,
 *   latency_p99_ms: number,
 *   cache_hit_ratio: number,
 *   total_requests: number,
 *   total_errors: number,
 *   time_window_minutes: number,
 * }} AnalyticsSummary
 */

/**
 * Query the Analytics Engine SQL API for aggregated metrics.
 *
 * @param {object} env - Worker environment bindings
 * @param {number} [windowMinutes=60] - Time window to query
 * @returns {Promise<AnalyticsSummary>}
 */
export async function queryAnalyticsEngine(env, windowMinutes = 60) {
  const accountId = env.CF_ACCOUNT_ID;
  const apiToken = env.CF_API_TOKEN;
  const datasetName = env.ANALYTICS_DATASET || "falcon_ocr";

  if (!accountId || !apiToken) {
    return null;
  }

  const query = `
    SELECT
      count() as total_requests,
      countIf(double1 >= 400) as total_errors,
      quantile(0.50)(double2) as p50_latency,
      quantile(0.95)(double2) as p95_latency,
      quantile(0.99)(double2) as p99_latency
    FROM ${datasetName}
    WHERE timestamp > now() - INTERVAL '${windowMinutes}' MINUTE
  `;

  const resp = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${accountId}/analytics_engine/sql`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${apiToken}`,
        "Content-Type": "text/plain",
      },
      body: query,
    },
  );

  if (!resp.ok) {
    return null;
  }

  const data = await resp.json();
  if (!data.data || data.data.length === 0) {
    return null;
  }

  const row = data.data[0];
  const totalRequests = Number(row.total_requests) || 0;
  const totalErrors = Number(row.total_errors) || 0;

  return {
    requests_per_minute: totalRequests / windowMinutes,
    error_rate: totalRequests > 0 ? totalErrors / totalRequests : 0,
    latency_p50_ms: Math.round(Number(row.p50_latency) || 0),
    latency_p95_ms: Math.round(Number(row.p95_latency) || 0),
    latency_p99_ms: Math.round(Number(row.p99_latency) || 0),
    cache_hit_ratio: 0, // Computed separately from KV metadata
    total_requests: totalRequests,
    total_errors: totalErrors,
    time_window_minutes: windowMinutes,
  };
}

/**
 * Compute cache hit ratio from KV metadata.
 *
 * Reads the running counters stored in KV by the cache layer.
 * Keys: _meta:cache_hits, _meta:cache_misses
 *
 * @param {object} env - Worker environment bindings
 * @returns {Promise<{hits: number, misses: number, ratio: number}>}
 */
export async function getCacheHitRatio(env) {
  if (!env.CACHE) {
    return { hits: 0, misses: 0, ratio: 0 };
  }

  const [hitsRaw, missesRaw] = await Promise.all([
    env.CACHE.get("_meta:cache_hits"),
    env.CACHE.get("_meta:cache_misses"),
  ]);

  const hits = parseInt(hitsRaw || "0", 10);
  const misses = parseInt(missesRaw || "0", 10);
  const total = hits + misses;

  return {
    hits,
    misses,
    ratio: total > 0 ? hits / total : 0,
  };
}

/**
 * Increment cache hit counter (call on cache hit).
 *
 * @param {object} env
 * @param {object} ctx - Execution context for waitUntil
 */
export function recordCacheHit(env, ctx) {
  if (!env.CACHE) return;
  ctx.waitUntil(
    (async () => {
      const current = parseInt((await env.CACHE.get("_meta:cache_hits")) || "0", 10);
      await env.CACHE.put("_meta:cache_hits", String(current + 1));
    })(),
  );
}

/**
 * Increment cache miss counter (call on cache miss).
 *
 * @param {object} env
 * @param {object} ctx - Execution context for waitUntil
 */
export function recordCacheMiss(env, ctx) {
  if (!env.CACHE) return;
  ctx.waitUntil(
    (async () => {
      const current = parseInt((await env.CACHE.get("_meta:cache_misses")) || "0", 10);
      await env.CACHE.put("_meta:cache_misses", String(current + 1));
    })(),
  );
}

/**
 * Get a full analytics summary suitable for the /health endpoint.
 *
 * Combines Analytics Engine data (if available) with KV cache metrics.
 *
 * @param {object} env
 * @returns {Promise<AnalyticsSummary>}
 */
export async function getAnalyticsSummary(env) {
  const [engineData, cacheData] = await Promise.all([
    queryAnalyticsEngine(env),
    getCacheHitRatio(env),
  ]);

  if (engineData) {
    engineData.cache_hit_ratio = cacheData.ratio;
    return engineData;
  }

  // Fallback: return just cache data when Analytics Engine is not configured
  return {
    requests_per_minute: 0,
    error_rate: 0,
    latency_p50_ms: 0,
    latency_p95_ms: 0,
    latency_p99_ms: 0,
    cache_hit_ratio: cacheData.ratio,
    total_requests: 0,
    total_errors: 0,
    time_window_minutes: 60,
  };
}
