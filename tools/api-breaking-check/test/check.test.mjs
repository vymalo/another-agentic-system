import assert from "node:assert/strict";
import { test } from "node:test";
import { parse } from "yaml";
import { breaking } from "../check.mjs";

const base = parse(`
openapi: 3.1.0
paths:
  /things:
    get:
      operationId: listThings
      parameters:
        - { name: limit, in: query, schema: { type: integer } }
      responses:
        "200":
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Page" }
        "404": { description: none }
    post:
      operationId: makeThing
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/NewThing" }
      responses:
        "201": { description: made }
  /things/{id}:
    get:
      operationId: getThing
      parameters:
        - { name: id, in: path, required: true, schema: { type: string } }
      responses:
        "200":
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Thing" }
components:
  schemas:
    Thing:
      type: object
      required: [id, kind]
      properties:
        id: { type: string }
        kind: { type: string, enum: [a, b] }
        note: { type: string }
    NewThing:
      type: object
      required: [kind]
      properties:
        kind: { type: string, enum: [a, b] }
        note: { type: string }
    Page:
      type: object
      properties:
        items: { type: array, items: { $ref: "#/components/schemas/Thing" } }
`);

const edit = (fn) => {
  const copy = structuredClone(base);
  fn(copy);
  return copy;
};
const found = (doc) => breaking(base, doc).join("\n");

test("the same document breaks nothing", () => {
  assert.deepEqual(breaking(base, structuredClone(base)), []);
});

test("additions that a client ignores break nothing", () => {
  const doc = edit((d) => {
    d.paths["/new"] = { get: { responses: { 200: { description: "ok" } } } };
    d.components.schemas.Thing.properties.extra = { type: "string" };
    d.components.schemas.NewThing.properties.extra = { type: "string" };
    d.paths["/things"].get.parameters.push({ name: "q", in: "query", schema: { type: "string" } });
    delete d.paths["/things"].get.responses["404"];
  });
  assert.deepEqual(breaking(base, doc), []);
});

test("a removed operation and a removed path are breaking", () => {
  assert.match(found(edit((d) => delete d.paths["/things"].post)), /POST \/things: operation removed/);
  assert.match(found(edit((d) => delete d.paths["/things/{id}"])), /\/things\/\{id\}: path removed/);
});

test("a removed response property, through a $ref, is breaking", () => {
  const out = found(edit((d) => delete d.components.schemas.Thing.properties.note));
  assert.match(out, /property note removed/);
});

test("a new required request property or parameter is breaking", () => {
  assert.match(found(edit((d) => d.components.schemas.NewThing.required.push("note"))), /property note became required/);
  assert.match(found(edit((d) => d.paths["/things"].get.parameters.push({ name: "q", in: "query", required: true, schema: { type: "string" } }))), /new required parameter query:q/);
});

test("a narrowed request enum is breaking, a widened response enum is not", () => {
  assert.match(found(edit((d) => (d.components.schemas.NewThing.properties.kind.enum = ["a"]))), /enum value b removed/);
  assert.equal(found(edit((d) => d.components.schemas.Thing.properties.kind.enum.push("c"))), "");
});

test("a changed type, a removed success response and a renamed operationId are breaking", () => {
  assert.match(found(edit((d) => (d.components.schemas.Thing.properties.id.type = "integer"))), /type changed from string to integer/);
  assert.match(found(edit((d) => delete d.paths["/things/{id}"].get.responses["200"])), /response 200 removed/);
  assert.match(found(edit((d) => (d.paths["/things"].get.operationId = "list"))), /operationId changed/);
});

test("a request type may widen, a response type may not", () => {
  assert.equal(found(edit((d) => (d.components.schemas.NewThing.properties.note.type = ["string", "null"]))), "");
  assert.match(found(edit((d) => (d.components.schemas.Thing.properties.note.type = ["string", "null"]))), /type changed from string to string\|null/);
});

test("a response property that is no longer always present is breaking", () => {
  assert.match(found(edit((d) => (d.components.schemas.Thing.required = ["id"]))), /property kind is no longer always present/);
});
