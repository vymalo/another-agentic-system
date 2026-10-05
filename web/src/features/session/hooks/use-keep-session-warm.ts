"use client";

import { useEffect } from "react";
import { keepSessionWarm } from "@/lib/api/session-refresh";

/**
 * Keeps the session warm while the app is open (`keepSessionWarm`): the signed-in app mounts it,
 * a shared page's public reader does not. Nothing without the edge's sign-in path.
 */
export function useKeepSessionWarm() {
  useEffect(() => keepSessionWarm(), []);
}
