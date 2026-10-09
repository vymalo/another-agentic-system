import { describe, expect, it } from "vitest";
import { isSettled } from "./reveal";

/*
 * When the transcript of an opened thread may be shown (ADR 0059, slice 1): after the replay of the log is applied,
 * not while it is. The runs of a replay come one after the other, and showing the transcript meanwhile is the page that
 * fills and chases its own bottom.
 */

const open = { loaded: true, replaying: false, connection: "open", lastSeq: 40 } as const;

describe("isSettled", () => {
  it("is held back while the log is being caught up", () => {
    expect(isSettled({ ...open, loaded: false })).toBe(false);
  });

  it("is held back while the runtime still has runs to show, even when the log is caught up", () => {
    expect(isSettled({ ...open, replaying: true })).toBe(false);
  });

  it("is settled when the log is caught up and the runtime shows every run", () => {
    expect(isSettled(open)).toBe(true);
  });

  it("is settled for a thread with nothing in it once the log is caught up", () => {
    expect(isSettled({ ...open, lastSeq: 0 })).toBe(true);
  });

  it("shows what there is when the connection is down with part of the log in", () => {
    expect(isSettled({ ...open, loaded: false, connection: "reconnecting" })).toBe(true);
  });

  it("does not show an empty transcript because the first connection failed", () => {
    expect(
      isSettled({ loaded: false, replaying: false, connection: "reconnecting", lastSeq: 0 }),
    ).toBe(false);
  });

  it("still waits for the runs of a part of the log that a dropped connection left", () => {
    expect(isSettled({ ...open, loaded: false, connection: "reconnecting", replaying: true })).toBe(
      false,
    );
  });
});
