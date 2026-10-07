"use client";

import { CheckIcon, CopyIcon, LinkIcon, RefreshCwIcon, UnlinkIcon } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { Sharing } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import type { ThreadSharer } from "../hooks/use-share-thread";
import { absoluteLink, levelsFor, type ShareLevel } from "../lib/sharing";

/** What the thread is shared as, for the radios: private while there is no share. */
const currentLevel = (sharer: ThreadSharer): ShareLevel => sharer.share?.visibility ?? "private";

/**
 * Why a share the person made is not what is served now (the cap was lowered since, or the
 * deployment turned sharing off): the link is paused, not gone.
 */
function pausedNote(share: NonNullable<ThreadSharer["share"]>): string | null {
  if (share.effective === share.visibility) return null;
  return share.effective === "private"
    ? "Sharing is turned off here for now, so this link does not work. It comes back when it is turned on."
    : "This deployment shares only with signed-in people for now, so the link works for them only.";
}

/**
 * The share dialog of a thread (ADR 0040, section 12): who may read it (Private, Signed-in people with
 * the link, Anyone with the link: each above the deployment's cap is disabled, and says why), the link
 * with **Copy**, **New link** (the old one stops working) and **Stop sharing**. A choice among the radios
 * is only picked until **Save** makes it the thread's: the arrow keys move between radios and select
 * each as they go, and walking past "Anyone with the link" must not make the thread public. A refusal
 * is the server's words under the choices. It is a dialog: it takes the focus, Escape closes it, and
 * `onCloseAutoFocus` says where the focus goes. The button that was pressed is disabled while its
 * request is out, and a browser drops the focus of a disabled button to the page (`Stop sharing` is
 * gone altogether): when the request ends, the focus goes to **Done**, which is never disabled, so a
 * person at the keyboard keeps their place in the dialog.
 */
export function ShareDialog({
  open,
  onOpenChange,
  sharer,
  cap,
  onCloseAutoFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sharer: ThreadSharer;
  /** What the person may share as (`GET /api/me`'s `sharing`). Stopping needs no cap. */
  cap: Sharing;
  onCloseAutoFocus?: (event: Event) => void;
}) {
  const ids = useId();
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const { share, busy } = sharer;
  const doneRef = useRef<HTMLButtonElement>(null);
  /** A request ended: the focus of the pressed button is lost with it, so it goes to Done. */
  const settled = () => doneRef.current?.focus();
  // what is picked and not yet saved; nothing picked is what the thread is
  const [picked, setPicked] = useState<ShareLevel | null>(null);
  const current = currentLevel(sharer);
  const selected = picked ?? current;
  useEffect(() => {
    if (!open) setPicked(null);
  }, [open]);
  const link = share?.url !== undefined ? absoluteLink(share.url, window.location.origin) : null;
  const paused = share ? pausedNote(share) : null;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent {...(onCloseAutoFocus ? { onCloseAutoFocus } : {})} data-slot="share-dialog">
        <DialogHeader>
          <DialogTitle>Share this conversation</DialogTitle>
          <DialogDescription>
            People with the link read it as it goes on. They cannot write in it.
          </DialogDescription>
        </DialogHeader>
        <RadioGroup
          aria-label="Who can read this conversation"
          value={selected}
          onValueChange={(level) => setPicked(level === current ? null : (level as ShareLevel))}
          disabled={busy}
        >
          {levelsFor(cap).map((choice) => {
            const id = `${ids}-${choice.level}`;
            return (
              <div
                key={choice.level}
                className="flex items-start gap-3 rounded-lg px-2 py-2 has-[button[data-state=checked]]:bg-muted"
              >
                <RadioGroupItem
                  id={id}
                  value={choice.level}
                  disabled={busy || !choice.allowed}
                  aria-describedby={`${id}-hint`}
                  className="mt-0.5"
                />
                <div className="min-w-0 flex-1">
                  <Label htmlFor={id} className={cn(!choice.allowed && "text-muted-foreground")}>
                    {choice.label}
                  </Label>
                  <p
                    id={`${id}-hint`}
                    className={cn(
                      "mt-1 text-[0.8125rem] leading-5",
                      choice.warning && choice.allowed ? "text-warning" : "text-muted-foreground",
                    )}
                  >
                    {choice.reason ?? choice.hint}
                  </p>
                </div>
              </div>
            );
          })}
        </RadioGroup>
        {paused ? (
          <InlineStatus tone="warning" role="status">
            {paused}
          </InlineStatus>
        ) : null}
        {share ? (
          <div data-slot="share-link" className="grid gap-3 border-t pt-4">
            {link ? (
              <div className="grid gap-1.5">
                <Label htmlFor={`${ids}-link`}>Link</Label>
                <div className="flex gap-2">
                  <Input
                    id={`${ids}-link`}
                    readOnly
                    value={link}
                    onFocus={(event) => event.currentTarget.select()}
                    className="min-w-0 font-mono text-xs"
                  />
                  <Button type="button" variant="outline" onClick={() => copyToClipboard(link)}>
                    {isCopied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}
                    {isCopied ? "Copied" : "Copy"}
                  </Button>
                </div>
              </div>
            ) : (
              <p className="flex items-center gap-2 text-[0.8125rem] text-muted-foreground">
                <LinkIcon aria-hidden="true" className="size-3.5 shrink-0" />
                There is no link while sharing is turned off here.
              </p>
            )}
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={busy || !link || cap === "disabled"}
                onClick={() => void sharer.newLink().then(settled)}
                title="Makes a new link: the old one stops working"
              >
                <RefreshCwIcon aria-hidden="true" />
                New link
              </Button>
              <Button
                type="button"
                variant="destructive"
                size="sm"
                disabled={busy}
                onClick={() => void sharer.stop().then(settled)}
              >
                <UnlinkIcon aria-hidden="true" />
                Stop sharing
              </Button>
            </div>
          </div>
        ) : null}
        {sharer.error ? (
          <InlineStatus tone="error" role="alert">
            {sharer.error}
          </InlineStatus>
        ) : null}
        <DialogFooter>
          <DialogClose asChild>
            <Button ref={doneRef} type="button" variant="outline">
              {picked === null ? "Done" : "Cancel"}
            </Button>
          </DialogClose>
          <Button
            type="button"
            disabled={busy || picked === null || picked === current}
            onClick={() => {
              if (picked === null) return;
              void sharer.choose(picked).then((done) => {
                if (done) setPicked(null);
                settled();
              });
            }}
          >
            Save
          </Button>
        </DialogFooter>
        <p role="status" className="sr-only">
          {isCopied ? "Link copied." : (sharer.notice ?? "")}
        </p>
      </DialogContent>
    </Dialog>
  );
}
