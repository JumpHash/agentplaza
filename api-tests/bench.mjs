// bench.mjs SPEC BASE OUT: the timings in the API report. Every GET in the spec without path
// parameters (no streams, no sockets, nothing marked x-outbound: those would reach the internet) is
// requested 20 times in a row; OUT gets, per route, the status, the p50, p95 and slowest time in
// milliseconds, and the body size.
import { readFileSync, writeFileSync } from "node:fs";

const [specPath, base, outPath] = process.argv.slice(2);
const spec = JSON.parse(readFileSync(specPath, "utf8"));
const RUNS = 20;

const routes = [];
for (const [path, ops] of Object.entries(spec.paths)) {
  const op = ops.get;
  if (!op || !path.startsWith("/v1/") || path.includes("{")) continue;
  if (JSON.stringify(op.responses).includes("text/event-stream") || "405" in op.responses) continue;
  if (op["x-outbound"]) continue; // it would reach the internet
  routes.push({ path, summary: op.summary ?? "", tag: op.tags?.[0] ?? "" });
}

const pct = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))];
const results = [];
for (const route of routes) {
  const times = [];
  let status = 0;
  let bytes = 0;
  for (let i = 0; i < RUNS; i++) {
    const start = performance.now();
    try {
      const res = await fetch(base + route.path, { signal: AbortSignal.timeout(5000) });
      const body = await res.arrayBuffer();
      status = res.status;
      bytes = body.byteLength;
    } catch {
      status = 0;
    }
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  const round = (ms) => Math.round(ms * 100) / 100;
  results.push({ ...route, status, bytes, runs: RUNS, p50: round(pct(times, 50)), p95: round(pct(times, 95)), max: round(times.at(-1)) });
}
writeFileSync(outPath, JSON.stringify({ runs: RUNS, routes: results }, null, 2));
console.log(`timings: ${results.length} routes x ${RUNS}`);
