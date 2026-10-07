/**
 * Runs `task` while holding the named Web Lock: one tab at a time across the whole browser. A
 * browser without `navigator.locks` gets a queue of this page only, which is as much as it can have.
 */
const queues = new Map<string, Promise<unknown>>();

export function withLock<T>(name: string, task: () => Promise<T>): Promise<T> {
  const locks = typeof navigator === "undefined" ? undefined : navigator.locks;
  if (locks) return locks.request(name, task) as Promise<T>;
  const run: Promise<T> = (queues.get(name) ?? Promise.resolve()).then(task, task);
  queues.set(
    name,
    run.catch(() => undefined),
  );
  return run;
}
