import { ThreadRoute } from "@/features/routing/thread-route";

/**
 * Every thread is this one exported page, `/threads/_` (ADR 0047): the static server answers it for any
 * `/threads/<id>`, and the page reads the id from the address.
 */
export function generateStaticParams() {
  return [{ id: "_" }];
}
export const dynamicParams = false;

export default function ThreadPage() {
  return <ThreadRoute />;
}
