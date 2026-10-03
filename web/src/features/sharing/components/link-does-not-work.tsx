import { PandaMark } from "@/components/brand/panda-mark";

/**
 * What a link that does not work says, for every way it can fail (ADR 0040: one 404, one body): an
 * unknown or mistyped link, a thread that is private again, a link that was replaced, a deployment that
 * stopped sharing. It never says which, and never whether a thread exists.
 */
export function LinkDoesNotWork() {
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-5 px-6 py-12 text-center">
      <PandaMark size={72} />
      <h1 className="text-[1.75rem] leading-tight font-medium tracking-tight text-balance">
        This link does not work
      </h1>
      <p className="max-w-md text-[0.9375rem] text-muted-foreground text-balance">
        It may have been copied wrongly, or the person who made it may have turned it off. Ask them
        for a new link.
      </p>
    </main>
  );
}
