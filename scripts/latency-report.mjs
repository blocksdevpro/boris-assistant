// Boris latency report: p50/p90 of speech_end_to_first_audio_ms, before vs after
// the new timing fields were introduced.
//
// Reads: $BORIS_HOME/traces/turns.jsonl (default ~/.boris/traces/turns.jsonl),
//        or the path given as the first argument. Nothing else is read.
// Usage: node scripts/latency-report.mjs [path/to/turns.jsonl]
//        bun  scripts/latency-report.mjs [path/to/turns.jsonl]
// Split rule: "after" starts at the first trace (file order) with a top-level
// `agent_ms` key and includes everything after it; "before" is everything
// earlier. If no trace has `agent_ms`, only "before" is printed.
// Percentiles: nearest-rank as in percentile_ms in crates/boris-agent/src/trace.rs
// (index = round(pct/100 * (n-1)) on the sorted values).

import { createReadStream } from "node:fs";
import { createInterface } from "node:readline";
import { homedir } from "node:os";
import { join } from "node:path";

const file =
  process.argv[2] ||
  join(process.env.BORIS_HOME || join(homedir(), ".boris"), "traces", "turns.jsonl");

const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);

function pct(values, p) {
  if (!values.length) return null;
  const v = [...values].sort((a, b) => a - b);
  return v[Math.min(Math.round((p / 100) * (v.length - 1)), v.length - 1)];
}

const mkGroup = () => ({ traces: 0, first: [], agent: [], answer: [], minT: null, maxT: null });
const before = mkGroup();
const after = mkGroup();
let cur = before;
let found = false;
let malformed = 0;

try {
  const rl = createInterface({ input: createReadStream(file, { encoding: "utf8" }), crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    let t;
    try {
      t = JSON.parse(line);
    } catch {
      malformed++;
      continue;
    }
    if (!t || typeof t !== "object" || Array.isArray(t)) {
      malformed++;
      continue;
    }
    if (!found && "agent_ms" in t) {
      found = true;
      cur = after;
    }
    cur.traces++;
    const f = num(t.speech_end_to_first_audio_ms);
    if (f !== null) cur.first.push(f);
    const a = num(t.agent_ms);
    if (a !== null) cur.agent.push(a);
    const s = num(t.speech_end_to_answer_audio_ms);
    if (s !== null) cur.answer.push(s);
    const ts = num(t.started_unix_ms);
    if (ts !== null) {
      cur.minT = cur.minT === null ? ts : Math.min(cur.minT, ts);
      cur.maxT = cur.maxT === null ? ts : Math.max(cur.maxT, ts);
    }
  }
} catch (e) {
  console.error(`cannot read ${file}: ${e.code || e.message}`);
  process.exit(1);
}

const day = (ms) => new Date(ms).toISOString().slice(0, 10);
const stats = (vals) =>
  vals.length ? `p50=${pct(vals, 50)} ms  p90=${pct(vals, 90)} ms  n=${vals.length}` : "n=0";
const range = (g) => (g.minT === null ? "n/a" : `${day(g.minT)} .. ${day(g.maxT)}`);

function print(name, g, extra) {
  console.log(`${name} (${g.traces} traces, ${range(g)})`);
  console.log(`  speech_end_to_first_audio_ms: ${stats(g.first)}`);
  if (extra) {
    console.log(`  agent_ms:                     ${stats(g.agent)}`);
    console.log(`  speech_end_to_answer_audio_ms: ${stats(g.answer)}`);
  }
}

console.log(`file: ${file}`);
console.log(`skipped malformed lines (blank lines ignored): ${malformed}`);
print("before", before, false);
if (found) print("after", after, true);
else console.log("split marker (agent_ms) not found; no 'after' group");
