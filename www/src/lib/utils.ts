import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

/** Required by components pulled from the shadcn/MagicUI registry. */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
