/**
 * The interface's language: a button for each one it speaks, pressed for the
 * one in use, with its name in itself as its title.
 *
 * One control in both shells, where there were two — the same logic dressed
 * twice. Segmented, each half a full 44px target: on a phone it is hit with
 * a thumb, in the public sheet and the administration's bar alike.
 */

import { cn } from '../lib/cn'
import { LANGS, LANGUAGES, useI18n } from '../lib/i18n'

export function LanguageToggle() {
  const { lang, setLang, t } = useI18n()

  return (
    <div
      role="group"
      aria-label={t.nav.language}
      className="flex shrink-0 items-center overflow-hidden rounded-card border border-rule"
    >
      {LANGS.map((code) => (
        <button
          key={code}
          type="button"
          onClick={() => setLang(code)}
          aria-pressed={lang === code}
          title={LANGUAGES[code].name}
          lang={code}
          className={cn(
            'flex min-h-11 min-w-11 cursor-pointer items-center justify-center px-2.5',
            'font-mono text-label font-medium tracking-wider uppercase',
            'transition-colors duration-150',
            lang === code ? 'bg-bone text-ink' : 'text-bone-faint hover:text-bone',
          )}
        >
          {code}
        </button>
      ))}
    </div>
  )
}
