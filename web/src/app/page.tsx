import { ChatShell } from "@/features/chat/components/chat-shell";
import { SignInGate } from "@/features/session/components/sign-in-screen";

export default function Home() {
  return (
    <SignInGate>
      <ChatShell threadId={null} />
    </SignInGate>
  );
}
