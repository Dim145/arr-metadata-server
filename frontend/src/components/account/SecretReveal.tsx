/**
 * A secret shown once: a new key, a generated password.
 *
 * Selected on focus and copied with one press, because it will not be shown
 * again; a browser that refuses the clipboard still leaves it selectable.
 */

import { useState } from 'react'

import { cn } from '../../lib/cn'
import { useI18n } from '../../lib/i18n'
import { Button, Glyph } from '../ui'

export function SecretReveal({
  title,
  secret,
  onDismiss,
  note,
}: {
  title: string
  secret: string
  onDismiss?: () => void
  note?: string
}) {
  const { t } = useI18n()
  const [copied, setCopied] = useState<'yes' | 'failed' | null>(null)
  const id = `secret-${secret.slice(0, 8)}`

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(secret)
      setCopied('yes')
    } catch {
      setCopied('failed')
    }
  }

  return (
    <div role="status" className="rise rounded-panel border border-moss/50 bg-moss/[0.07] p-4">
      <label htmlFor={id} className="label flex items-center gap-2 text-moss">
        <Glyph name="key" className="size-3.5" />
        {title}
      </label>
      {/* The whole secret on its own line: a key is forty-odd characters, and
          one cut short in a narrow box is one copied by eye with a slip. */}
      <input
        id={id}
        readOnly
        value={secret}
        onFocus={(event) => event.currentTarget.select()}
        className={cn(
          'mt-2 min-h-11 w-full rounded-card border border-moss/40 bg-ink px-3',
          'font-mono text-[0.8125rem] text-bone selection:bg-moss selection:text-ink',
        )}
      />
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={() => void copy()}>
          <Glyph name={copied === 'yes' ? 'check' : 'copy'} className="size-4" />
          {copied === 'yes' ? t.account.copied : t.account.copy}
        </Button>
        {onDismiss ? <Button onClick={onDismiss}>{t.account.dismiss}</Button> : null}
      </div>
      <p className="mt-2 text-xs text-bone-faint">{note ?? t.account.secretOnce}</p>
      {copied === 'failed' ? (
        <p role="alert" className="mt-1 text-xs text-vermillion">
          {t.account.copyFailed}
        </p>
      ) : null}
    </div>
  )
}
