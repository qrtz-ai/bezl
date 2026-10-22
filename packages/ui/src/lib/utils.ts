import { clsx, type ClassValue } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

// tailwind-merge needs the `tw` prefix (components.json) to dedupe our classes.
const twMerge = extendTailwindMerge({ prefix: 'tw' });

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
