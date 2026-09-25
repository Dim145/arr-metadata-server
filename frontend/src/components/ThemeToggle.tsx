/**
 * The lamp: dark or light. One button, pressed when the page is light, so a
 * screen reader hears a state and not a command.
 */

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { useTheme } from '../lib/theme'
import { Glyph } from './ui'

export function ThemeToggle({ className }: { className?: string }) {
  const { t } = useI18n()
  const [theme, setTheme] = useTheme()
  const light = theme === 'light'

  return (
    <button
      type="button"
      onClick={() => setTheme(light ? 'dark' : 'light')}
      aria-pressed={light}
      aria-label={t.nav.theme}
      title={t.nav.theme}
      className={cn(
        'flex min-h-11 min-w-11 shrink-0 cursor-pointer items-center justify-center rounded-card border border-rule',
        'text-bone-faint transition-colors duration-200 hover:text-bone',
        light && 'bg-bone text-ink hover:text-ink',
        className,
      )}
    >
      <Glyph name={light ? 'sun' : 'moon'} className="size-4" />
    </button>
  )
}
