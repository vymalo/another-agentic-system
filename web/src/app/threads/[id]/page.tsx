import { notFound } from "next/navigation";
import { ChatShell } from "@/features/chat/components/chat-shell";
import { SignInGate } from "@/features/session/components/sign-in-screen";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export default async function ThreadPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  if (!UUID.test(id)) notFound();
  return (
    <SignInGate>
      <ChatShell threadId={id} />
    </SignInGate>
  );
}
