import { describe, expect, it } from "vitest";
import { readSse, type SseFrame } from "./sse";

const enc = new TextEncoder();

/** A body that delivers `chunks` as separate reads. */
const body = (chunks: string[]) =>
  new ReadableStream<Uint8Array>({
    start(c) {
      for (const chunk of chunks) c.enqueue(enc.encode(chunk));
      c.close();
    },
  });

async function read(chunks: string[]): Promise<SseFrame[]> {
  const out: SseFrame[] = [];
  for await (const f of readSse(body(chunks))) out.push(f);
  return out;
}

describe("readSse", () => {
  it("reads id and data, ignores comments, event and retry", async () => {
    expect(
      await read([
        ": keepalive\n\n",
        "retry: 1000\n\n",
        'id: 4\nevent: x\ndata: {"a":1}\n\n',
        'data: {"b":2}\n\n',
      ]),
    ).toEqual([{ id: "4", data: '{"a":1}' }, { data: '{"b":2}' }]);
  });

  it("the id belongs to its own block only", async () => {
    const frames = await read(["id: 1\ndata: a\n\ndata: b\n\n"]);
    expect(frames).toEqual([{ id: "1", data: "a" }, { data: "b" }]);
  });

  it("joins several data lines with LF", async () => {
    expect(await read(["data: one\ndata: two\n\n"])).toEqual([{ data: "one\ntwo" }]);
  });

  it("copes with CRLF, CR and chunks that cut anywhere", async () => {
    const text = 'id: 7\r\ndata: {"x":"é"}\r\n\r\nid: 8\rdata: y\r\r';
    const whole = await read([text]);
    expect(whole).toEqual([
      { id: "7", data: '{"x":"é"}' },
      { id: "8", data: "y" },
    ]);
    // one byte per read, including the two halves of the multi-byte character
    const bytes = enc.encode(text);
    const oneByOne = new ReadableStream<Uint8Array>({
      start(c) {
        for (const b of bytes) c.enqueue(Uint8Array.of(b));
        c.close();
      },
    });
    const out: SseFrame[] = [];
    for await (const f of readSse(oneByOne)) out.push(f);
    expect(out).toEqual(whole);
  });

  it("drops a block the stream ended in the middle of", async () => {
    expect(await read(["id: 1\ndata: a\n\nid: 2\ndata: b"])).toEqual([{ id: "1", data: "a" }]);
  });

  it("stops reading and cancels the body when the consumer leaves", async () => {
    let cancelled = false;
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(enc.encode("data: a\n\ndata: b\n\n"));
      },
      cancel() {
        cancelled = true;
      },
    });
    for await (const f of readSse(stream)) {
      expect(f.data).toBe("a");
      break;
    }
    expect(cancelled).toBe(true);
  });

  it("ends when the signal aborts", async () => {
    const ac = new AbortController();
    const never = new ReadableStream<Uint8Array>({ start() {} });
    const done = (async () => {
      const out: SseFrame[] = [];
      for await (const f of readSse(never, ac.signal)) out.push(f);
      return out;
    })();
    ac.abort();
    expect(await done).toEqual([]);
  });
});
