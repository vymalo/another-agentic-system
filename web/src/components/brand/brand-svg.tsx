import type { ReactNode } from "react";

/*
 * The frame of a third-party mark (third-party-notices.md). It draws the brand's paths in the colour
 * of the text around it and nothing else: no colour of ours, no script, no link. A mark with no
 * `title` is decoration (whoever shows it says the name in text); with one it is an image of that name.
 */

export type BrandMarkProps = {
  className?: string;
  /** The brand's name, for a mark that stands alone. */
  title?: string;
};

export function BrandSvg({
  brand,
  viewBox,
  title,
  className,
  children,
}: BrandMarkProps & { brand: string; viewBox: string; children: ReactNode }) {
  const frame = {
    xmlns: "http://www.w3.org/2000/svg",
    viewBox,
    fill: "currentColor",
    "data-slot": "brand-mark",
    "data-brand": brand,
    className,
  };
  if (title) {
    return (
      <svg {...frame} role="img" aria-label={title}>
        <title>{title}</title>
        {children}
      </svg>
    );
  }
  return (
    <svg {...frame} aria-hidden="true">
      {children}
    </svg>
  );
}
