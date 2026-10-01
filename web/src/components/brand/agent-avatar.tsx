import type { CSSProperties } from "react";
import { cn } from "@/lib/utils";

/**
 * Agents are not the panda (the panda is the house). An agent is a 28 px circle with the first
 * letter of its name on one of six sober tints, chosen by a stable hash of its id, so the same
 * agent has the same colour in every thread and on every device. Light and dark pairs; every
 * text/background pair is at least 4.5:1 (agent-avatar.test.tsx checks the table).
 */
export const AGENT_TINTS = [
  { name: "sage", light: ["#e4ede2", "#2f4b31"], dark: ["#2c3a2c", "#cfe6cd"] },
  { name: "sand", light: ["#efe8d6", "#5a4a1f"], dark: ["#3b3524", "#ead9a8"] },
  { name: "slate", light: ["#e1e7ee", "#34465c"], dark: ["#2a3442", "#c5d3e6"] },
  { name: "clay", light: ["#f1e1da", "#6b3a2a"], dark: ["#432e26", "#efc6b6"] },
  { name: "plum", light: ["#ebe0ec", "#5a3a5e"], dark: ["#3a2b3d", "#e3c8e6"] },
  { name: "teal", light: ["#d9ebea", "#1f5553"], dark: ["#1f3a39", "#bfe3e1"] },
] as const;

/** FNV-1a over the UTF-16 code units: small, stable across runs and platforms. */
function hash(value: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < value.length; i++) {
    h ^= value.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

export type Tint = (typeof AGENT_TINTS)[number];

/** The tint of an agent: the same id always gets the same one. */
export function tintOf(agentId: string): Tint {
  return AGENT_TINTS[hash(agentId) % AGENT_TINTS.length] ?? AGENT_TINTS[0];
}

/** The first letter (or digit) of a name, in capitals; the first character when it has neither. */
export function initialOf(name: string): string {
  const chars = Array.from(name.trim());
  const first = chars.find((c) => /[\p{L}\p{N}]/u.test(c)) ?? chars[0];
  return first ? first.toLocaleUpperCase() : "?";
}

type Props = {
  /** What the tint comes from: the agent's id, stable across renames. */
  agentId: string;
  /** What the letter comes from: the agent's name as the person reads it. */
  name: string;
  size?: number;
  className?: string;
};

/** Decorative: whoever shows an avatar says the name in text beside it. */
export function AgentAvatar({ agentId, name, size = 28, className }: Props) {
  const tint = tintOf(agentId);
  const style = {
    width: size,
    height: size,
    fontSize: Math.round(size * 0.46),
    "--avatar-bg": tint.light[0],
    "--avatar-fg": tint.light[1],
    "--avatar-bg-dark": tint.dark[0],
    "--avatar-fg-dark": tint.dark[1],
  } as CSSProperties;
  return (
    <span
      aria-hidden="true"
      data-slot="agent-avatar"
      data-tint={tint.name}
      style={style}
      className={cn(
        "inline-flex shrink-0 select-none items-center justify-center rounded-full bg-(--avatar-bg) font-semibold leading-none text-(--avatar-fg) dark:bg-(--avatar-bg-dark) dark:text-(--avatar-fg-dark)",
        className,
      )}
    >
      {initialOf(name)}
    </span>
  );
}
