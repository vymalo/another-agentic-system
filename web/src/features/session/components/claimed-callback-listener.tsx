"use client";

import { useEffect } from "react";
import { startClaimedCallbackListener } from "@/lib/auth/sign-in";

/** Receives a claimed Android App Link or iOS Universal Link after the native shell has started. */
export function ClaimedCallbackListener() {
  useEffect(() => {
    void startClaimedCallbackListener();
  }, []);
  return null;
}
