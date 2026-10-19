import { clsx, type ClassValue } from 'clsx'
import { extendTailwindMerge } from 'tailwind-merge'

/**
 * The named steps of type in `styles.css` (`--text-caption` and the rest).
 *
 * Told to the merge because it cannot read the theme: to it, any `text-` it
 * does not know is a colour, so `text-caption` followed by `text-bone` lost
 * the size to the colour and the caption fell back to the body's.
 */
const twMerge = extendTailwindMerge({
  extend: { theme: { text: ['label', 'caption', 'body-sm', 'body', 'lead', 'plate'] } },
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
