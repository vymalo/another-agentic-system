// Load test of the dev stack: many signed-in people, each run an AG-UI POST held open as a stream, ramped in steps against the
// WireMock agents and the scripted `mock-model`. Node standard library only (Node 22: fetch, streams). Mocks only, no secret:
// tokens come from the mock issuer through dev/auth-header.sh. See dev/README.md "Load test".
import { execFile } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const sh = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const env = process.env;
const num = (k, d) => (env[k] !== undefined && env[k] !== "" ? Number(env[k]) : d);

const cfg = {
  base: (env.BASE_URL || `http://127.0.0.1:${env.EDGE_PORT || 8080}`).replace(/\/$/, ""),
  agent: env.AGENT || "chat",
  steps: (env.STEPS || "10,50,100,200").split(",").map(Number),
  hold: num("HOLD_SECS", 25), // seconds each step is held and sampled after its target is reached
  runTimeout: num("RUN_TIMEOUT_SECS", 120),
  sample: num("SAMPLE_SECS", 5),
  message: env.MESSAGE || "[mock:slow] load test", // `[mock:slow]`: the model takes 20 s, so a run stays open
  users: (env.USERS || "").split(",").filter(Boolean),
  mock: (env.MOCK_MODEL_URL || `http://127.0.0.1:${env.MOCK_MODEL_PORT || 8094}`).replace(/\/$/, ""),
  metricsCmd: env.METRICS_CMD || "docker compose exec -T edge wget -qO- http://orchestrator:8080/metrics",
  out: env.OUT || "",
  errorBudget: num("MAX_ERROR_RATE", 0.02),
};
if (!cfg.users.length) {
  const u = JSON.parse(readFileSync(join(here, "../mock-oidc/users.json"), "utf8")).users;
  cfg.users = Object.entries(u).filter(([, v]) => v.roles.includes("user")).map(([k]) => k);
}

const pct = (a, p) => (a.length ? [...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.floor((a.length * p) / 100))] : null);
const fmt = (v, d = 0) => (v === null || v === undefined || Number.isNaN(v) ? "n/a" : Number(v).toFixed(d));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function token(email) {
  const { stdout } = await sh("sh", [join(here, "../auth-header.sh"), email], { env });
  return stdout.trim().replace(/^Authorization: /, "");
}
const auth = new Map();
async function authFor(email) {
  const hit = auth.get(email);
  if (hit && Date.now() - hit.at < 30 * 60_000) return hit.v; // a token lasts an hour
  const v = await token(email);
  auth.set(email, { v, at: Date.now() });
  return v;
}

// ---- one run: POST /agui/agents/{agent}, read the SSE stream to its terminal event -------------------------------
const stats = { started: 0, finished: 0, errors: 0, errorKinds: {}, active: 0, openStreams: 0, post: [], first: [], dur: [] };
let window = null; // per step
const err = (k) => { stats.errors++; stats.errorKinds[k] = (stats.errorKinds[k] || 0) + 1; if (window) window.errors++; };

async function oneRun(i) {
  const email = cfg.users[i % cfg.users.length];
  const t0 = performance.now();
  stats.started++; stats.active++;
  if (window) window.started++;
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), cfg.runTimeout * 1000);
  try {
    const body = JSON.stringify({ threadId: randomUUID(), runId: randomUUID(), state: {}, tools: [], context: [],
      messages: [{ id: randomUUID(), role: "user", content: cfg.message }], forwardedProps: {} });
    const res = await fetch(`${cfg.base}/agui/agents/${cfg.agent}`, { method: "POST", signal: ctl.signal, body,
      headers: { authorization: (await authFor(email)).replace(/^Authorization: /, ""), "content-type": "application/json", accept: "text/event-stream" } });
    const tHead = performance.now();
    if (res.status !== 200) { err(`http_${res.status}`); await res.body?.cancel(); return; }
    stats.post.push(tHead - t0); if (window) window.post.push(tHead - t0);
    stats.openStreams++;
    let first = false, terminal = null, buf = "";
    try {
      const dec = new TextDecoder();
      for await (const chunk of res.body) {
        buf += dec.decode(chunk, { stream: true });
        let nl;
        while ((nl = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, nl); buf = buf.slice(nl + 1);
          if (!line.startsWith("data:")) continue;
          if (!first) { first = true; const d = performance.now() - t0; stats.first.push(d); if (window) window.first.push(d); }
          try { const ev = JSON.parse(line.slice(5)); if (ev.type === "RUN_FINISHED") terminal = "ok"; else if (ev.type === "RUN_ERROR") terminal = "run_error"; } catch { /* a partial frame */ }
        }
      }
    } finally { stats.openStreams--; }
    if (terminal === "ok") { stats.finished++; const d = performance.now() - t0; stats.dur.push(d); if (window) window.finished++; }
    else err(terminal || "no_terminal_event");
  } catch (e) {
    err(e.name === "AbortError" ? "timeout" : "transport");
  } finally { clearTimeout(timer); stats.active--; }
}

// ---- samplers (each optional: n/a when the tool or the stack part is missing) -----------------------------------
async function outbox() {
  try {
    const [bin, ...args] = cfg.metricsCmd.split(" ");
    const { stdout } = await sh(bin, args, { timeout: 8000, env });
    const get = (re) => { const m = stdout.match(re); return m ? Number(m[1]) : null; };
    return { due: get(/orch_outbox_rows\{state="due"\} (\d+)/), leased: get(/orch_outbox_rows\{state="leased"\} (\d+)/),
      age: get(/orch_outbox_oldest_due_age_seconds (\d+)/) };
  } catch { return null; }
}
async function dbConns() {
  try {
    const { stdout } = await sh("docker", ["compose", "exec", "-T", "postgres", "psql", "-U", "postgres", "-d", "orch", "-Atc",
      "select count(*) from pg_stat_activity where datname='orch'"], { timeout: 8000 });
    return Number(stdout.trim());
  } catch { return null; }
}
async function cpu() {
  try {
    const { stdout } = await sh("docker", ["stats", "--no-stream", "--format", "{{.Name}} {{.CPUPerc}}"], { timeout: 15000 });
    let orch = 0, pg = 0, wm = 0;
    for (const l of stdout.split("\n")) {
      const [name, p] = l.split(" "); const v = parseFloat(p);
      if (Number.isNaN(v)) continue;
      if (/orchestrator/.test(name)) orch += v; else if (/postgres/.test(name)) pg += v; else if (/mock-model|mock-/.test(name)) wm += v;
    }
    return { orch, pg, wm };
  } catch { return null; }
}
async function mockProbe() { // is the mock itself the limit? time WireMock's own health route directly, bypassing the stack
  const ts = [];
  for (let i = 0; i < 10; i++) {
    const t = performance.now();
    try { const r = await fetch(`${cfg.mock}/__admin/health`, { signal: AbortSignal.timeout(5000) }); await r.arrayBuffer(); ts.push(performance.now() - t); } catch { return null; }
  }
  return pct(ts, 50);
}

// ---- the ramp ------------------------------------------------------------------------------------------------------
async function main() {
  const rows = [];
  let n = 0;
  console.log(`load test: ${cfg.base}, agent ${cfg.agent}, steps ${cfg.steps.join(",")}, hold ${cfg.hold}s, ${cfg.users.length} user(s), message "${cfg.message}"`);
  try {
    const r = await fetch(`${cfg.base}/readyz`, { signal: AbortSignal.timeout(10000) });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
  } catch (e) { console.error(`FAIL the stack is not ready at ${cfg.base}/readyz (${e.message}): docker compose --profile app up -d --wait`); process.exit(2); }
  const baseMock = await mockProbe();
  for (const target of cfg.steps) {
    window = { target, started: 0, finished: 0, errors: 0, post: [], first: [], samples: [] };
    const w = window;
    const tStep = Date.now();
    // top up to the target; keep topping up while the step is held, since runs end
    const topUp = () => { while (stats.active < target) oneRun(n++); };
    topUp();
    let reached = null;
    let heldFrom = null;
    while (true) {
      await sleep(cfg.sample * 1000);
      topUp();
      if (reached === null && stats.active >= target) { reached = (Date.now() - tStep) / 1000; heldFrom = Date.now(); }
      const [ob, db, c] = await Promise.all([outbox(), dbConns(), cpu()]);
      w.samples.push({ active: stats.active, streams: stats.openStreams, ob, db, c });
      if (heldFrom && Date.now() - heldFrom >= cfg.hold * 1000) break;
      if (Date.now() - tStep > (cfg.hold + 4 * cfg.runTimeout) * 1000) break;
    }
    const mp = await mockProbe();
    const s = w.samples, mx = (f) => { const v = s.map(f).filter((x) => x !== null && x !== undefined); return v.length ? Math.max(...v) : null; };
    const row = { target, reached, started: w.started, finished: w.finished, errors: w.errors,
      postP50: pct(w.post, 50), postP95: pct(w.post, 95), firstP50: pct(w.first, 50), firstP95: pct(w.first, 95),
      streams: mx((x) => x.streams), outboxDue: mx((x) => x.ob?.due), outboxLeased: mx((x) => x.ob?.leased), outboxAge: mx((x) => x.ob?.age),
      dbConns: mx((x) => x.db), cpuOrch: mx((x) => x.c?.orch), cpuPg: mx((x) => x.c?.pg), mockMs: mp };
    rows.push(row);
    console.log(`step ${target} done: ${row.started} runs started, ${row.finished} finished, ${row.errors} errors`);
  }
  window = null;
  // drain: let the open runs end, without starting more
  const drainBy = Date.now() + cfg.runTimeout * 1000;
  while (stats.active > 0 && Date.now() < drainBy) await sleep(1000);

  const head = ["target", "reached s", "started", "ok", "err", "post p50/p95 ms", "first ev p50/p95 ms", "streams", "outbox due/leased", "oldest due s", "db conns", "cpu% orch/pg", "mock ms (idle>peak)"];
  const lines = rows.map((r) => [r.target, fmt(r.reached, 1), r.started, r.finished, r.errors,
    `${fmt(r.postP50)}/${fmt(r.postP95)}`, `${fmt(r.firstP50)}/${fmt(r.firstP95)}`, fmt(r.streams),
    `${fmt(r.outboxDue)}/${fmt(r.outboxLeased)}`, fmt(r.outboxAge), fmt(r.dbConns), `${fmt(r.cpuOrch)}/${fmt(r.cpuPg)}`,
    `${fmt(baseMock, 1)}>${fmt(r.mockMs, 1)}`]);
  const widths = head.map((h, i) => Math.max(h.length, ...lines.map((l) => String(l[i]).length)));
  const row = (a) => "| " + a.map((v, i) => String(v).padEnd(widths[i])).join(" | ") + " |";
  const table = [row(head), "|" + widths.map((w) => "-".repeat(w + 2)).join("|") + "|", ...lines.map(row)].join("\n");
  console.log("\n" + table);
  console.log(`\ntotals: ${stats.started} runs started, ${stats.finished} finished, ${stats.errors} errors ${JSON.stringify(stats.errorKinds)}; run duration p50 ${fmt(pct(stats.dur, 50))} ms`);
  console.log("n/a: the sampler's tool (docker compose) or route was not reachable from here; METRICS_CMD and the README say how to point it.");
  if (cfg.out) writeFileSync(cfg.out, JSON.stringify({ config: { ...cfg, users: cfg.users.length }, rows, totals: stats.errorKinds }, null, 2));
  const rate = stats.started ? stats.errors / stats.started : 0;
  if (rate > cfg.errorBudget || stats.finished === 0) { console.error(`FAIL error rate ${(rate * 100).toFixed(1)}% over the budget ${(cfg.errorBudget * 100).toFixed(1)}%`); process.exit(1); }
  console.log("load test passed");
}
main().catch((e) => { console.error(e); process.exit(2); });
