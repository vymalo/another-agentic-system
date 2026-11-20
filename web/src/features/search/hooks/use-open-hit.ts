import { usePathname, useRouter } from "next/navigation";
import { useCallback } from "react";
import type { SearchOption } from "@/features/search/lib/options";

/**
 * Opens a hit: `/threads/<id>#m-<seq>` for a message, `/threads/<id>` for a title or a description. When that thread is already
 * open only the hash changes, and the page is told (`hashchange`) so that it scrolls to the message again, also when the same hash is
 * chosen twice. `done` runs after, for what the list has to do (a phone's sheet closes).
 */
export function useOpenHit(done?: () => void): (option: SearchOption) => void {
  const router = useRouter();
  const pathname = usePathname();
  return useCallback(
    (option: SearchOption) => {
      const [path = "", hash] = option.href.split("#");
      if (pathname === path) {
        if (hash !== undefined) {
          const unchanged = window.location.hash === `#${hash}`;
          window.location.hash = hash;
          if (unchanged) window.dispatchEvent(new HashChangeEvent("hashchange"));
        }
      } else {
        router.push(option.href);
      }
      done?.();
    },
    [pathname, router, done],
  );
}
