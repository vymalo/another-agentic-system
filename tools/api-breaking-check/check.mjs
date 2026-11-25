#!/usr/bin/env node
// Compares an OpenAPI document with the one on the base branch and lists what a client of the
// published contract would notice: a removed operation, a removed or narrowed response field, a
// new required request field or parameter, a changed type, a removed request enum value, a changed
// operationId. A new response enum value is NOT flagged: it is how the contract grows. Same idea as oasdiff, in the few rules this contract
// needs and no more (tools/api-breaking-check/README.md).
//
//   node check.mjs --base <git ref> [--file docs/api/chat-api.yaml] [--allow]
//   node check.mjs --old a.yaml --new b.yaml
//
// Exit 0: nothing breaking (or --allow). Exit 1: breaking changes, listed one per line. Exit 2: the
// check itself could not run.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { parse } from "yaml";

const METHODS = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];

export function breaking(oldDoc, newDoc) {
  const out = [];
  const add = (where, what) => out.push(`${where}: ${what}`);
  const oldPaths = oldDoc.paths ?? {};
  const newPaths = newDoc.paths ?? {};
  for (const [path, oldItem] of Object.entries(oldPaths)) {
    const newItem = newPaths[path];
    if (!newItem) {
      add(path, "path removed");
      continue;
    }
    for (const method of METHODS) {
      const oldOp = oldItem[method];
      if (!oldOp) continue;
      const where = `${method.toUpperCase()} ${path}`;
      const newOp = newItem[method];
      if (!newOp) {
        add(where, "operation removed");
        continue;
      }
      if (oldOp.operationId && oldOp.operationId !== newOp.operationId) {
        add(where, `operationId changed from ${oldOp.operationId} to ${newOp.operationId}`);
      }
      compareParameters(where, params(oldDoc, oldItem, oldOp), params(newDoc, newItem, newOp), add);
      compareRequest(where, oldDoc, oldOp, newDoc, newOp, add);
      compareResponses(where, oldDoc, oldOp, newDoc, newOp, add);
    }
  }
  return out;
}

function resolve(doc, node, seen = new Set()) {
  while (node && typeof node === "object" && typeof node.$ref === "string") {
    const ref = node.$ref;
    if (!ref.startsWith("#/") || seen.has(ref)) return {};
    seen.add(ref);
    node = ref
      .slice(2)
      .split("/")
      .reduce((n, key) => n?.[key.replace(/~1/g, "/").replace(/~0/g, "~")], doc);
  }
  return node ?? {};
}

function params(doc, item, op) {
  const map = new Map();
  for (const raw of [...(item.parameters ?? []), ...(op.parameters ?? [])]) {
    const p = resolve(doc, raw);
    if (p.name) map.set(`${p.in}:${p.name}`, { required: p.required === true || p.in === "path", schema: resolve(doc, p.schema) });
  }
  return map;
}

function compareParameters(where, oldP, newP, add) {
  for (const [key, p] of newP) {
    const before = oldP.get(key);
    if (!before && p.required) add(where, `new required parameter ${key}`);
    else if (before && !before.required && p.required) add(where, `parameter ${key} became required`);
    else if (before) compareSchema(`${where} parameter ${key}`, before.schema, p.schema, "request", add, oldP, newP);
  }
}

function content(doc, holder) {
  const out = {};
  for (const [type, media] of Object.entries(resolve(doc, holder).content ?? {})) out[type] = media;
  return out;
}

function compareRequest(where, oldDoc, oldOp, newDoc, newOp, add) {
  const oldBody = oldOp.requestBody && resolve(oldDoc, oldOp.requestBody);
  const newBody = newOp.requestBody && resolve(newDoc, newOp.requestBody);
  if (!oldBody && newBody?.required) add(where, "a request body became required");
  if (!oldBody || !newBody) return;
  if (oldBody.required !== true && newBody.required === true) add(where, "the request body became required");
  const oldC = content(oldDoc, oldBody);
  const newC = content(newDoc, newBody);
  for (const type of Object.keys(oldC)) {
    if (!newC[type]) {
      add(where, `request content type ${type} removed`);
      continue;
    }
    walk(`${where} request ${type}`, oldDoc, oldC[type].schema, newDoc, newC[type].schema, "request", add);
  }
}

function compareResponses(where, oldDoc, oldOp, newDoc, newOp, add) {
  for (const [code, oldRaw] of Object.entries(oldOp.responses ?? {})) {
    const newRaw = newOp.responses?.[code];
    if (!newRaw) {
      if (/^2/.test(code)) add(where, `response ${code} removed`);
      continue;
    }
    const oldC = content(oldDoc, oldRaw);
    const newC = content(newDoc, newRaw);
    for (const type of Object.keys(oldC)) {
      if (!newC[type]) {
        add(where, `response ${code} content type ${type} removed`);
        continue;
      }
      walk(`${where} response ${code} ${type}`, oldDoc, oldC[type].schema, newDoc, newC[type].schema, "response", add);
    }
  }
}

// Walks two schemas together. `side` is "request" (what a client sends) or "response" (what it reads).
function walk(where, oldDoc, oldRaw, newDoc, newRaw, side, add, depth = 0) {
  if (depth > 12) return;
  const o = flatten(oldDoc, oldRaw);
  const n = flatten(newDoc, newRaw);
  compareSchema(where, o, n, side, add);
  const oldProps = o.properties ?? {};
  const newProps = n.properties ?? {};
  for (const [name, oldProp] of Object.entries(oldProps)) {
    if (!(name in newProps)) {
      if (side === "response") add(where, `property ${name} removed`);
      continue;
    }
    walk(`${where}.${name}`, oldDoc, oldProp, newDoc, newProps[name], side, add, depth + 1);
  }
  const oldRequired = new Set(o.required ?? []);
  const newRequired = new Set(n.required ?? []);
  for (const name of newRequired) {
    if (side === "request" && !oldRequired.has(name)) add(where, `property ${name} became required`);
  }
  for (const name of oldRequired) {
    if (side === "response" && !newRequired.has(name) && name in oldProps) add(where, `property ${name} is no longer always present`);
  }
  if (side === "request") {
    for (const name of Object.keys(newProps)) {
      if (!(name in oldProps) && newRequired.has(name)) add(where, `new required property ${name}`);
    }
  }
  if (o.items && n.items) walk(`${where}[]`, oldDoc, o.items, newDoc, n.items, side, add, depth + 1);
}

function flatten(doc, raw, seen = new Set()) {
  const node = resolve(doc, raw);
  if (!Array.isArray(node.allOf)) return node;
  const merged = { ...node, properties: { ...(node.properties ?? {}) }, required: [...(node.required ?? [])] };
  for (const part of node.allOf) {
    const p = flatten(doc, part, seen);
    Object.assign(merged.properties, p.properties ?? {});
    merged.required.push(...(p.required ?? []));
    for (const key of ["type", "items", "enum", "format"]) if (merged[key] === undefined && p[key] !== undefined) merged[key] = p[key];
  }
  delete merged.allOf;
  return merged;
}

function compareSchema(where, o, n, side, add) {
  if (o.type && n.type) {
    const before = [].concat(o.type).map(String);
    const after = [].concat(n.type).map(String);
    // What a client sends may widen (every old type still accepted); what it reads may not change.
    const broke = side === "request" ? before.some((t) => !after.includes(t)) : before.join() !== after.join() && (after.some((t) => !before.includes(t)) || before.some((t) => !after.includes(t)));
    if (broke) add(where, `type changed from ${before.join("|")} to ${after.join("|")}`);
  }
  if (o.format && n.format && o.format !== n.format) add(where, `format changed from ${o.format} to ${n.format}`);
  if (Array.isArray(o.enum) && Array.isArray(n.enum)) {
    const oldSet = new Set(o.enum.map(String));
    const newSet = new Set(n.enum.map(String));
    if (side === "request") for (const v of oldSet) if (!newSet.has(v)) add(where, `enum value ${v} removed`);
  }
}

function cli() {
  const args = process.argv.slice(2);
  const get = (flag) => {
    const i = args.indexOf(flag);
    return i >= 0 ? args[i + 1] : undefined;
  };
  const file = get("--file") ?? "docs/api/chat-api.yaml";
  const allow = args.includes("--allow");
  let oldText;
  let newText;
  try {
    if (get("--old")) {
      oldText = readFileSync(get("--old"), "utf8");
      newText = readFileSync(get("--new"), "utf8");
    } else {
      const base = get("--base");
      if (!base) throw new Error("--base <git ref> or --old and --new");
      oldText = execFileSync("git", ["show", `${base}:${file}`], { encoding: "utf8", maxBuffer: 64 << 20 });
      newText = readFileSync(file, "utf8");
    }
  } catch (e) {
    console.error(`api-breaking-check: cannot read the documents: ${e.message}`);
    process.exit(2);
  }
  let found;
  try {
    found = breaking(parse(oldText), parse(newText));
  } catch (e) {
    console.error(`api-breaking-check: cannot compare: ${e.message}`);
    process.exit(2);
  }
  if (found.length === 0) {
    console.log("api-breaking-check: no breaking change");
    return;
  }
  console.log(`api-breaking-check: ${found.length} breaking change${found.length === 1 ? "" : "s"}${allow ? " (allowed by the PR title)" : ""}:`);
  for (const line of found) console.log(`  - ${line}`);
  if (!allow) {
    console.log("A deliberate break needs a conventional-commit title with `!` (for example `feat(api)!: ...`).");
    process.exit(1);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) cli();
