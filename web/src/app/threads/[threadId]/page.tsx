import { notFound } from "next/navigation";
import { ChatShell } from "@/components/ChatShell";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export default async function ThreadPage({ params }: { params: Promise<{ threadId: string }> }) {
  const { threadId } = await params;
  if (!UUID.test(threadId)) notFound();
  return <ChatShell threadId={threadId} />;
}
