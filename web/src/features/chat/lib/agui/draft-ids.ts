import { uuidv7 } from "@/lib/uuid";

/**
 * The id of the message a draft goes out as (ADR 0082, C4): **minted by the client, for the words in
 * the box**, and reused for every send of those words — the press of Send after a refusal, a steer sent
 * again, a POST the network lost before the run started. The orchestrator keys idempotency on it
 * (`agui:<thread>:msg:<id>`), so five sends of one text are one message, one run and one charge.
 *
 * The id belongs to the text, and the server enforces that: a known id sent with **other** words is
 * refused (`422 message_id_reused`, ADR 0082 decision 5). So the binding follows the box: it is minted
 * when the text is first sent, kept while the text is unchanged, and minted again when the person edits
 * it. Once the orchestrator has accepted the message the words are the log's, and typing them again is
 * a new message, so `accepted` drops the binding.
 */
export class DraftIds {
  private bound: { text: string; id: string } | undefined;

  /** The id `text` goes out as: the same one for as long as the words are those words. */
  of(text: string): string {
    if (this.bound?.text !== text) this.bound = { text, id: uuidv7() };
    return this.bound.id;
  }

  /** The run was accepted: the text is in the log, so the same words typed again are a new message. */
  accepted(text: string): void {
    if (this.bound?.text === text) this.bound = undefined;
  }
}
