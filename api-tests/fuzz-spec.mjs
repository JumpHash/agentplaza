// fuzz-spec.mjs SPEC OUT: the spec Schemathesis generates requests from: the hub's /v1 API without
// its event streams and sockets (a stream never ends, a socket wants an upgrade) and the calls marked
// x-outbound (they would reach the internet).
import { readFileSync, writeFileSync } from "node:fs";

const [specPath, outPath] = process.argv.slice(2);
const spec = JSON.parse(readFileSync(specPath, "utf8"));
let kept = 0;
for (const [path, ops] of Object.entries(spec.paths)) {
  for (const [method, op] of Object.entries(ops)) {
    const streams = JSON.stringify(op.responses).includes("text/event-stream");
    const socket = "405" in op.responses || /socket/i.test(op.summary ?? "");
    if (!path.startsWith("/v1/") || streams || socket || op["x-outbound"]) delete ops[method];
    else kept++;
  }
  if (Object.keys(ops).length === 0) delete spec.paths[path];
}
writeFileSync(outPath, JSON.stringify(spec));
console.log(`fuzz-spec: ${kept} operations`);
