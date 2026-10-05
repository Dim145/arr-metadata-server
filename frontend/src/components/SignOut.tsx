/**
 * The way out, for anybody signed in, from either side of the site.
 *
 * It was the administration's alone, and a member — a plain account, which is
 * what everybody who registers or is invited becomes — had no way to end their
 * session from the interface: on a shared machine it stayed open until it
 * lapsed, with their account page, their keys and the shelf of what they had
 * opened lately for whoever sat down next.
 */

import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from 'react-router'

import { api } from '../lib/api'
import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { forget } from '../lib/recent'
import { Glyph } from './ui'

export function SignOut({ className, iconOnly }: { className?: string; iconOnly?: boolean }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  return (
    <button
      type="button"
      aria-label={iconOnly ? t.nav.signOut : undefined}
      className={cn('cursor-pointer', className)}
      onClick={async () => {
        // Whatever the server says, the session is over here: everything cached
        // was answered for somebody who is now gone, and so was the shelf of
        // the works they opened. A server that cannot be reached is caught
        // rather than left to reject — signing out still works, and an
        // unhandled rejection would be the only trace of it.
        try {
          await api.post('/auth/logout')
        } catch {
          // Nothing to tell them: they are being signed out either way.
        } finally {
          forget()
          queryClient.clear()
          navigate('/login', { replace: true })
        }
      }}
    >
      <Glyph name="signOut" className={iconOnly ? 'size-5' : 'size-4'} />
      {iconOnly ? null : t.nav.signOut}
    </button>
  )
}
