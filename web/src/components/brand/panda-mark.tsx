import Image from "next/image";
import { cn } from "@/lib/utils";

/**
 * The house mark: the panda, three colours, one drawing (`public/brand/panda.svg`, web/DESIGN.md
 * "Brand"). A picture, not an icon: the name sits in text beside it, so it is hidden from
 * assistive technology. It is the product's mark, never an agent's (see `AgentAvatar`).
 */
export function PandaMark({ size = 28, className }: { size?: number; className?: string }) {
  return (
    <Image
      src="/brand/panda.svg"
      alt=""
      aria-hidden="true"
      width={size}
      height={size}
      unoptimized
      draggable={false}
      data-slot="panda-mark"
      className={cn("shrink-0 select-none", className)}
    />
  );
}
