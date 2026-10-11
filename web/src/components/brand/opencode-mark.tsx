import { type BrandMarkProps, BrandSvg } from "./brand-svg";

/*
 * OpenCode's mark, the square logo of its brand assets
 * (`packages/console/app/src/asset/brand/opencode-logo-*-square.svg` in github.com/anomalyco/opencode,
 * formerly sst/opencode, offered at https://opencode.ai/brand): the same two shapes, in a 16 x 20 box
 * (the asset's 240 x 300 over 15), the inner block lighter as in the asset. The repository is MIT,
 * (c) 2025 opencode, and the brand page states no further terms: verified 2026-10-11 (ADR 0049,
 * amended); third-party-notices.md. Drawn small, beside the step that hands work to OpenCode.
 */

export function OpenCodeMark(props: BrandMarkProps) {
  return (
    <BrandSvg brand="opencode" viewBox="0 0 16 20" {...props}>
      <path d="M12 16H4V8H12V16Z" opacity=".4" />
      <path d="M12 4H4V16H12V4ZM16 20H0V0H16V20Z" />
    </BrandSvg>
  );
}
