// build.mjs OUT SITE: the hub API report, one HTML page in the console's own design (its tokens and
// faces, apps/web/src/styles/tokens.css and assets/fonts), from what api-tests/run.sh left in OUT:
//
//   bruno.json          the Bruno run: every request, its assertions and tests, the response
//   bench.json          the timings: p50, p95 and slowest of 20 requests per parameterless GET
//   schemathesis/*.xml  Schemathesis's JUnit report: 5xx answers, dropped connections, schema errors
//   docs/api/openapi.json (from the repository) for the coverage
//
// The page is public (GitHub Pages), so every body is scrubbed: a value under a key that names a
// token, secret, password, key or cookie becomes "[scrubbed]", and so does a bearer token anywhere.
// Optional environment: REPORT_PR, REPORT_SHA, REPORT_RUN_URL, REPORT_BASELINE (a previous run's
// bench.json, for the timings' change against main).
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [outDir, siteDir] = process.argv.slice(2);
const read = (path, fallback) => (existsSync(path) ? readFileSync(path, "utf8") : fallback);
const json = (path, fallback) => {
  try {
    return JSON.parse(read(path, ""));
  } catch {
    return fallback;
  }
};

const bruno = json(join(outDir, "bruno.json"), [])[0] ?? { results: [], summary: {} };
// The data may come from a fork's run (api-report.yml): every number is made a number here, and
// every string is escaped where it is printed.
const num = (v) => (Number.isFinite(Number(v)) ? Number(v) : 0);
const numOrNull = (v) => (v == null || !Number.isFinite(Number(v)) ? null : Number(v));
const cleanBench = (b) => ({
  runs: num(b?.runs) || 20,
  routes: (Array.isArray(b?.routes) ? b.routes : []).map((r) => ({
    path: String(r?.path ?? ""),
    summary: String(r?.summary ?? ""),
    tag: String(r?.tag ?? ""),
    status: num(r?.status),
    bytes: numOrNull(r?.bytes),
    p50: num(r?.p50),
    p95: num(r?.p95),
    max: num(r?.max),
  })),
});
const bench = cleanBench(json(join(outDir, "bench.json"), {}));
const baseline = process.env.REPORT_BASELINE ? cleanBench(json(process.env.REPORT_BASELINE, {})) : null;
const spec = json(process.env.REPORT_SPEC || "docs/api/openapi.json", { paths: {} });
if (!spec.paths || typeof spec.paths !== "object") spec.paths = {};
const findings = readSchemathesis(join(outDir, "schemathesis"));

// ---- scrubbing ---------------------------------------------------------------------------------

const SECRET_KEY = /token|secret|passw|api[-_]?key|^key$|authorization|cookie|credential/i;
function scrub(value) {
  if (typeof value === "string") return value.replace(/(bearer\s+)[A-Za-z0-9._~+/=-]+/gi, "$1[scrubbed]");
  if (Array.isArray(value)) return value.map(scrub);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([k, v]) => [k, SECRET_KEY.test(k) && v != null && v !== "" ? "[scrubbed]" : scrub(v)]),
    );
  }
  return value;
}
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const excerpt = (data, max = 1200) => {
  const text = typeof data === "string" ? data : JSON.stringify(scrub(data), null, 2);
  const clean = scrub(text ?? "");
  return clean.length > max ? `${clean.slice(0, max)}\n… (${clean.length - max} more characters)` : clean;
};

// ---- the data ----------------------------------------------------------------------------------

const safeDecode = (s) => {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
};
const requests = (Array.isArray(bruno.results) ? bruno.results : []).map((r) => {
  const url = new URL(r.request?.url ?? "http://x/");
  const checks = [
    ...(r.assertionResults ?? []).map((a) => ({ text: `${a.lhsExpr} ${a.rhsExpr}`, status: a.status, error: a.error && String(a.error) })),
    ...(r.testResults ?? []).map((t) => ({ text: String(t.description ?? ""), status: t.status, error: t.error && String(t.error) })),
  ];
  return {
    folder: String(r.path ?? "").split("/")[0],
    name: String(r.name ?? ""),
    method: String(r.request?.method ?? ""),
    path: safeDecode(url.pathname) + url.search,
    status: num(r.response?.status),
    ms: numOrNull(r.response?.responseTime),
    size: numOrNull(r.response?.size),
    ok: r.status === "pass" && !r.error && checks.every((c) => c.status === "pass"),
    error: r.error,
    checks,
    body: r.response?.data,
  };
});
const passed = requests.filter((r) => r.ok).length;
const failed = requests.length - passed;
const checksTotal = requests.reduce((n, r) => n + r.checks.length, 0);
const checksPassed = requests.reduce((n, r) => n + r.checks.filter((c) => c.status === "pass").length, 0);

// Coverage: which spec operations the Bruno run reached (a request matches a pattern segment by segment).
const operations = Object.entries(spec.paths).flatMap(([path, ops]) =>
  Object.entries(ops ?? {}).map(([method, op]) => ({ method: method.toUpperCase(), path, tag: String(op?.tags?.[0] ?? ""), summary: String(op?.summary ?? "") })),
);
const matches = (pattern, path) => {
  const a = pattern.split("/");
  const b = path.split("?")[0].split("/");
  if (a.length !== b.length && !pattern.includes("..}")) return false;
  return a.every((seg, i) => seg.startsWith("{") || seg === b[i]);
};
for (const op of operations) op.tested = requests.some((r) => r.method === op.method && matches(op.path, r.path));
const tested = operations.filter((o) => o.tested).length;
const byTag = new Map();
for (const op of operations) {
  const t = byTag.get(op.tag) ?? { tag: op.tag, total: 0, tested: 0 };
  t.total++;
  if (op.tested) t.tested++;
  byTag.set(op.tag, t);
}

const timings = [...bench.routes].sort((a, b) => b.p95 - a.p95);
const median = (xs) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)] : 0);
const baseOf = new Map((baseline?.routes ?? []).map((r) => [r.path, r]));
const slowest = Math.max(1, ...timings.map((t) => t.p95));

function readSchemathesis(dir) {
  if (!existsSync(dir)) return [];
  const xml = readdirSync(dir).filter((f) => f.endsWith(".xml")).map((f) => readFileSync(join(dir, f), "utf8")).join("\n");
  const out = [];
  const unxml = (t) => t.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&");
  for (const m of xml.matchAll(/<testcase\b[^>]*name="([^"]*)"[^>]*>([\s\S]*?)<\/testcase>/g)) {
    for (const f of m[2].matchAll(/<(failure|error)\b([^>]*)>([\s\S]*?)<\/\1>|<(failure|error)\b([^>]*)\/>/g)) {
      const kind = f[1] ?? f[4];
      const attrs = f[2] ?? f[5] ?? "";
      const message = unxml((attrs.match(/message="([^"]*)"/)?.[1] ?? "") + "\n" + (f[3] ?? "")).trim();
      const label = /Network Error/.test(message)
        ? "Dropped connection"
        : /Server error/.test(message)
          ? "Server error"
          : /Schema Error/.test(message)
            ? "Spec problem"
            : kind === "failure"
              ? "Failed check"
              : "Error";
      out.push({ operation: unxml(m[1]), kind: label, message });
    }
  }
  return out;
}
const findingKinds = new Map();
for (const f of findings) findingKinds.set(f.kind, (findingKinds.get(f.kind) ?? 0) + 1);

// ---- the page ----------------------------------------------------------------------------------

const verdict = failed === 0 && requests.length > 0;
const sha = process.env.REPORT_SHA ?? "";
const pr = process.env.REPORT_PR ?? "";
const runUrl = process.env.REPORT_RUN_URL ?? "";
const when = new Date().toISOString().replace("T", " ").slice(0, 16) + " UTC";
const fmtMs = (ms) => (ms == null ? "" : ms < 10 ? ms.toFixed(2) : ms < 100 ? ms.toFixed(1) : Math.round(ms).toString());
const fmtBytes = (b) => (b == null ? "" : b < 1024 ? `${b} B` : `${(b / 1024).toFixed(1)} KiB`);
const statusClass = (code) => (code >= 500 || code === 0 ? "crit" : code >= 400 ? "warn" : "ok");

const tile = (label, value, note, tone = "") =>
  `<div class="tile ${tone}"><div class="tile-label">${esc(label)}</div><div class="tile-value">${value}</div><div class="tile-note">${note}</div></div>`;

const folders = [...new Set(requests.map((r) => r.folder))];
const requestRows = folders
  .map((folder) => {
    const rows = requests.filter((r) => r.folder === folder);
    const bad = rows.filter((r) => !r.ok).length;
    return `<details class="group" ${bad ? "open" : ""}>
<summary><span class="group-name">${esc(folder)}</span><span class="meta">${rows.length} requests${bad ? ` · <b class="crit">${bad} failed</b>` : ""}</span></summary>
<table><thead><tr><th>Request</th><th>Call</th><th class="num">Status</th><th class="num">ms</th><th class="num">Size</th><th>Result</th></tr></thead><tbody>
${rows
  .map(
    (r) => `<tr class="${r.ok ? "" : "row-fail"}"><td>${esc(r.name)}</td><td class="mono"><span class="method">${esc(r.method)}</span> ${esc(r.path)}</td>
<td class="num"><span class="badge ${statusClass(r.status)}">${r.status || "—"}</span></td><td class="num mono">${fmtMs(r.ms)}</td><td class="num mono">${fmtBytes(r.size)}</td>
<td>${r.ok ? '<span class="pass">pass</span>' : '<span class="fail">fail</span>'}</td></tr>
${
  r.ok
    ? ""
    : `<tr class="detail"><td colspan="6">${r.error ? `<p class="crit">${esc(r.error)}</p>` : ""}<ul class="checks">${r.checks
        .map((c) => `<li class="${c.status === "pass" ? "pass" : "fail"}">${esc(c.text)}${c.error ? ` — <span class="mono">${esc(c.error)}</span>` : ""}</li>`)
        .join("")}</ul><pre>${esc(excerpt(r.body))}</pre></td></tr>`
}`,
  )
  .join("")}
</tbody></table></details>`;
  })
  .join("\n");

const timingRows = timings
  .map((t) => {
    const before = baseOf.get(t.path);
    const change = before && before.p95 > 0 ? ((t.p95 - before.p95) / before.p95) * 100 : null;
    return `<tr><td class="mono">${esc(t.path)}</td><td>${esc(t.summary)}</td><td class="num"><span class="badge ${statusClass(t.status)}">${t.status || "—"}</span></td>
<td class="num mono">${fmtMs(t.p50)}</td><td class="num mono">${fmtMs(t.p95)}</td><td class="num mono">${fmtMs(t.max)}</td>
<td class="bar-cell"><span class="bar" style="width:${Math.max(1, (t.p95 / slowest) * 100).toFixed(1)}%"></span></td>
<td class="num mono">${fmtBytes(t.bytes)}</td>${baseline ? `<td class="num mono ${change > 15 ? "crit" : change < -15 ? "ok" : ""}">${change == null ? "new" : `${change > 0 ? "+" : ""}${change.toFixed(0)}%`}</td>` : ""}</tr>`;
  })
  .join("");

const findingGroups = [...findingKinds.keys()]
  .map((kind) => {
    const items = findings.filter((f) => f.kind === kind);
    return `<details class="group"><summary><span class="group-name">${esc(kind)}</span><span class="meta">${items.length}</span></summary>
<table><thead><tr><th>Operation</th><th>What happened</th></tr></thead><tbody>
${items.map((f) => `<tr><td class="mono nowrap">${esc(f.operation)}</td><td><pre>${esc(excerpt(f.message, 900))}</pre></td></tr>`).join("")}
</tbody></table></details>`;
  })
  .join("\n");

const coverageRows = [...byTag.values()]
  .sort((a, b) => b.total - a.total)
  .map(
    (t) => `<tr><td>${esc(t.tag)}</td><td class="num mono">${t.tested}/${t.total}</td>
<td class="bar-cell"><span class="bar ok-bar" style="width:${((t.tested / t.total) * 100).toFixed(1)}%"></span></td></tr>`,
  )
  .join("");

const tokens = read("apps/web/src/styles/tokens.css", "");
const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Hub API report${pr ? ` · #${esc(pr)}` : ""}</title>
<style>
${tokens}
@media (prefers-color-scheme: light) { :root:not([data-theme="dark"]) { color-scheme: light; --paper: #e9ece5; --sheet: #f4f6f1; --rule: #c7cfc3; --rule-faint: #dbe1d7; --margin: #d07a72; --ink: #151916; --ink-2: #4c544d; --ink-3: #7d867e; --signal: #e8590c; --signal-ink: #a63c00; --crit: #a3182b; --warn-ink: #85640e; --ok-ink: #2e6a40; --info-ink: #2d5f8a; --hover: #e6eae2; --print: 2px 2px 0 var(--ink); --shadow: 3px 3px 0 var(--ink); } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--paper); color: var(--ink); font: var(--type-prose, 400 15px/1.6 var(--sans)); }
main { max-width: var(--content-max); margin: 0 auto; padding: 32px 24px 64px; border-left: 2px solid var(--margin); }
h1 { font: 600 24px/1.25 var(--sans); margin: 0 0 4px; }
h2 { font: 600 17px/1.4 var(--sans); margin: 40px 0 12px; padding-bottom: 6px; border-bottom: 1px solid var(--rule); }
.meta, .tile-note, th { color: var(--ink-3); font: 400 12px/1.4 var(--sans); }
.mono, pre, code { font-family: var(--mono); font-size: 12px; }
.head { display: flex; flex-wrap: wrap; gap: 16px; align-items: baseline; justify-content: space-between; }
.verdict { font: 600 13px/1 var(--sans); padding: 8px 12px; border: 2px solid var(--ink); box-shadow: var(--print); background: var(--sheet); }
.verdict.ok { color: var(--ok-ink); } .verdict.fail { color: var(--crit); }
.tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 12px; margin-top: 24px; }
.tile { background: var(--sheet); border: 1px solid var(--rule); padding: 14px 16px; }
.tile-label { font: 600 11px/1.3 var(--sans); text-transform: uppercase; letter-spacing: .06em; color: var(--ink-2); }
.tile-value { font: 600 26px/1.2 var(--sans); margin: 6px 0 2px; font-variant-numeric: tabular-nums; }
.tile.crit .tile-value { color: var(--crit); } .tile.ok .tile-value { color: var(--ok-ink); } .tile.warn .tile-value { color: var(--warn-ink); }
.group { background: var(--sheet); border: 1px solid var(--rule); margin: 10px 0; }
.group > summary { cursor: pointer; padding: 10px 14px; display: flex; justify-content: space-between; gap: 12px; list-style: none; }
.group > summary::-webkit-details-marker { display: none; }
.group > summary:hover { background: var(--hover); }
.group-name { font: 600 14px/1.4 var(--sans); }
.scroll { overflow-x: auto; }
table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: 7px 12px; border-top: 1px solid var(--rule-faint); vertical-align: top; }
th { font-weight: 600; text-transform: uppercase; letter-spacing: .05em; font-size: 11px; }
td.num, th.num { text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
.nowrap { white-space: nowrap; }
.method { font-weight: 600; color: var(--ink-2); }
.badge { display: inline-block; min-width: 38px; text-align: center; font: 600 11px/1.6 var(--mono); border: 1px solid currentColor; padding: 0 4px; }
.ok, .pass { color: var(--ok-ink); } .warn { color: var(--warn-ink); } .crit, .fail { color: var(--crit); }
.pass, .fail { font: 600 12px/1 var(--sans); text-transform: uppercase; letter-spacing: .05em; }
.row-fail td { background: color-mix(in srgb, var(--crit) 7%, transparent); }
tr.detail td { background: var(--paper); }
.checks { margin: 0 0 8px; padding-left: 18px; font-size: 13px; }
pre { white-space: pre-wrap; word-break: break-word; margin: 0; padding: 10px 12px; background: var(--paper); border: 1px solid var(--rule-faint); max-height: 320px; overflow: auto; }
.bar-cell { width: 22%; min-width: 120px; }
.bar { display: block; height: 8px; background: var(--signal); }
.ok-bar { background: var(--ok-ink); }
.note { color: var(--ink-2); font-size: 13px; max-width: 80ch; }
footer { margin-top: 48px; color: var(--ink-3); font-size: 12px; }
a { color: var(--info-ink); }
@media (max-width: 640px) { main { padding: 20px 16px 48px; } .bar-cell { display: none; } }
</style>
</head>
<body>
<main>
<div class="head">
  <div>
    <h1>Hub API report</h1>
    <div class="meta">${pr ? `Pull request #${esc(pr)} · ` : ""}${sha ? `commit <span class="mono">${esc(sha.slice(0, 10))}</span> · ` : ""}${esc(when)}${runUrl ? ` · <a href="${esc(runUrl)}">the CI run</a>` : ""}</div>
  </div>
  <div class="verdict ${verdict ? "ok" : "fail"}">${verdict ? "All API tests pass" : requests.length ? `${failed} API test${failed === 1 ? "" : "s"} failed` : "No API tests ran"}</div>
</div>

<section class="tiles">
${tile("Requests", `${passed}/${requests.length}`, "Bruno requests passing", failed ? "crit" : "ok")}
${tile("Checks", `${checksPassed}/${checksTotal}`, "assertions and tests")}
${tile("Spec coverage", `${tested}/${operations.length}`, `operations the tests call (${operations.length ? Math.round((tested / operations.length) * 100) : 0}%)`)}
${tile("p95, median route", `${fmtMs(median(timings.map((t) => t.p95)))} ms`, `${timings.length} GET routes × ${bench.runs ?? 20}`)}
${tile("Slowest p95", `${fmtMs(timings[0]?.p95)} ms`, esc(timings[0]?.path ?? ""))}
${tile("Fuzzing findings", findings.length.toString(), [...findingKinds].map(([k, n]) => `${n} ${esc(k.toLowerCase())}`).join(" · ") || "none", findings.length ? "warn" : "ok")}
</section>

<h2>API tests (Bruno)</h2>
<p class="note">The collection in <code>api-tests/bruno</code>, run against a scratch hub: no database, no node daemon, no git host. A failing request fails the pull request.</p>
<div class="scroll">${requestRows || '<p class="note">No requests ran.</p>'}</div>

<h2>Timings</h2>
<p class="note">Every GET in the spec without path parameters, requested ${bench.runs ?? 20} times in a row on the CI runner, slowest p95 first. Times are milliseconds on a shared runner: compare them run to run, not to production.${baseline ? " The last column is the p95's change against main." : ""}</p>
<div class="group scroll"><table><thead><tr><th>Route</th><th>What it does</th><th class="num">Status</th><th class="num">p50</th><th class="num">p95</th><th class="num">max</th><th>p95</th><th class="num">Size</th>${baseline ? '<th class="num">vs main</th>' : ""}</tr></thead><tbody>${timingRows}</tbody></table></div>

<h2>Fuzzing (Schemathesis)</h2>
<p class="note">Requests generated from <code>docs/api/openapi.json</code>: a 5xx answer or a connection dropped without an answer is a finding. 502 and 503 come from the scratch hub's missing node daemon and git host. Reported here; findings do not fail the pull request yet.</p>
${findingGroups || '<p class="note">No findings.</p>'}

<h2>Coverage by area</h2>
<div class="group scroll"><table><thead><tr><th>Area</th><th class="num">Tested</th><th></th></tr></thead><tbody>${coverageRows}</tbody></table></div>

<footer>Made by <code>api-tests/run.sh</code> in the pull request gate. Bodies are scrubbed of tokens and secrets before they are published.</footer>
</main>
</body>
</html>
`;

mkdirSync(siteDir, { recursive: true });
writeFileSync(join(siteDir, "index.html"), html);
for (const font of ["InterVariable.woff2", "InterVariable-Italic.woff2", "MartianMono-Regular.ttf", "MartianMono-Medium.ttf", "MartianMono-Bold.ttf"]) {
  if (existsSync(join("assets/fonts", font))) copyFileSync(join("assets/fonts", font), join(siteDir, font));
}
writeFileSync(
  join(siteDir, "summary.json"),
  JSON.stringify({ requests: requests.length, passed, failed, checks: checksTotal, checksPassed, operations: operations.length, tested, findings: Object.fromEntries(findingKinds) }, null, 2),
);
console.log(`report: ${passed}/${requests.length} requests, ${tested}/${operations.length} operations, ${findings.length} fuzzing findings -> ${join(siteDir, "index.html")}`);
