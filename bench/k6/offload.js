import http from "k6/http";
import { check, sleep } from "k6";

const target = Number(__ENV.K6_TARGET || 100);
const warmup = __ENV.K6_WARMUP || "10s";
const steady = __ENV.K6_STEADY || "20s";
const cooldown = __ENV.K6_COOLDOWN || "10s";

export const options = {
  stages: [
    { duration: warmup, target },
    { duration: steady, target },
    { duration: cooldown, target: 0 },
  ],
  thresholds: {
    http_req_failed: ["rate<0.01"],
    http_req_duration: ["p(95)<1000"],
  },
};

const baseUrl = __ENV.OFFLOAD_URL || "http://127.0.0.1:8000/offload/";
const query = __ENV.OFFLOAD_QUERY || "user_id=1&limit=50";
const sleepMs = Number(__ENV.K6_SLEEP_MS || 0);

export default function () {
  const res = http.get(`${baseUrl}?${query}`);
  check(res, {
    "status 200": (r) => r.status === 200,
  });
  if (sleepMs > 0) {
    sleep(sleepMs / 1000);
  }
}
