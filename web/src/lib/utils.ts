import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** Class names, later Tailwind utilities winning over earlier ones. */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
