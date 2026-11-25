import { afterEach, describe, expect, it, vi } from "vitest";
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

afterEach(() => {
  vi.useRealTimers();
});

describe("readSse", () => {
  // The name used to say comments were ignored. They are read, and they are the heartbeat: a comment
  // is activity that re-arms the silence watchdog below, and it is still not a frame (ADR 0082, C3).
  it("reads id and data; a comment is a keepalive, not a frame; event and retry are ignored", async () => {
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

  // ADR 0082, C3: a socket held open by a proxy or a dead NAT entry says nothing and never closes. The
  // reader must let go of it, so the caller can reopen from its last `id:`.
  it("lets go of a body that says nothing for the silence window, and not a moment sooner", async () => {
    vi.useFakeTimers();
    const never = new ReadableStream<Uint8Array>({ start() {} });
    let ended = false;
    const done = (async () => {
      for await (const _ of readSse(never, undefined, { silenceMs: 100 })) {
        // nothing to read
      }
      ended = true;
    })();
    await vi.advanceTimersByTimeAsync(99);
    expect(ended).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await done;
    expect(ended).toBe(true);
  });

  it("is kept alive by a keepalive comment: only real silence is a dead stream", async () => {
    vi.useFakeTimers();
    let beat!: ReadableStreamDefaultController<Uint8Array>;
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        beat = c;
      },
    });
    // the orchestrator's heartbeat, one window apart at the most
    const beats = setInterval(() => beat.enqueue(enc.encode(": keepalive\n\n")), 90);
    let ended = false;
    const done = (async () => {
      for await (const _ of readSse(stream, undefined, { silenceMs: 100 })) {
        // nothing to read
      }
      ended = true;
    })();
    for (let i = 0; i < 10; i++) {
      await vi.advanceTimersByTimeAsync(90);
      expect(ended).toBe(false);
    }
    clearInterval(beats);
    beat.close();
    await done;
    expect(ended).toBe(true);
  });

  // A hidden tab: browsers throttle timers and may pause the connection, so silence there is not real.
  describe("while the tab is hidden", () => {
    const fakeDocument = (state: "visible" | "hidden") => {
      const target = new EventTarget();
      const doc = Object.assign(target, { visibilityState: state });
      vi.stubGlobal("document", doc);
      return {
        set(next: "visible" | "hidden") {
          doc.visibilityState = next;
          target.dispatchEvent(new Event("visibilitychange"));
        },
      };
    };
    const silent = (silenceMs: number) => {
      const never = new ReadableStream<Uint8Array>({ start() {} });
      const state = { ended: false };
      const done = (async () => {
        for await (const _ of readSse(never, undefined, { silenceMs })) {
          // nothing to read
        }
        state.ended = true;
      })();
      return { state, done };
    };

    afterEach(() => vi.unstubAllGlobals());

    it("does not let go of a silent body that is hidden for longer than the window", async () => {
      vi.useFakeTimers();
      fakeDocument("hidden");
      const { state } = silent(100);
      await vi.advanceTimersByTimeAsync(1000);
      expect(state.ended).toBe(false);
    });

    it("restarts its clock when the tab becomes visible, and lets go after one full window", async () => {
      vi.useFakeTimers();
      const tab = fakeDocument("hidden");
      const { state, done } = silent(100);
      await vi.advanceTimersByTimeAsync(1000);
      tab.set("visible");
      await vi.advanceTimersByTimeAsync(99);
      expect(state.ended).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await done;
      expect(state.ended).toBe(true);
    });

    it("counts a visible tab as it always did", async () => {
      vi.useFakeTimers();
      fakeDocument("visible");
      const { state, done } = silent(100);
      await vi.advanceTimersByTimeAsync(99);
      expect(state.ended).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await done;
      expect(state.ended).toBe(true);
    });

    it("stops counting when a visible tab becomes hidden", async () => {
      vi.useFakeTimers();
      const tab = fakeDocument("visible");
      const { state } = silent(100);
      await vi.advanceTimersByTimeAsync(50);
      tab.set("hidden");
      await vi.advanceTimersByTimeAsync(1000);
      expect(state.ended).toBe(false);
    });
  });
});
