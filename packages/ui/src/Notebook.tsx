import { forwardRef } from 'react';
import type { IconProps } from 'react-feather';

/**
 * A notebook (nano's glyph): a cover with ring binding on the left and two ruled lines. Feather has no
 * notebook, so this is drawn on Feather's grid (24×24, stroked, round caps and joins) and takes the same
 * props, so it sits in ICONS like any other Feather icon.
 */
export const Notebook = forwardRef<SVGSVGElement, IconProps>(function Notebook(
  { color = 'currentColor', size = 24, ...rest },
  ref,
) {
  return (
    <svg
      ref={ref}
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      {...rest}
    >
      <rect x="5" y="2" width="15" height="20" rx="2" />
      <path d="M3 6h4M3 10h4M3 14h4M3 18h4" />
      <path d="M10 8h6M10 12h6" />
    </svg>
  );
});
