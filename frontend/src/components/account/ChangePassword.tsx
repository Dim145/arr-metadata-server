/**
 * Changing one's password.
 *
 * The server ends every session when it succeeds — they were all authorised
 * under the old password — so this form's success state is the sign-in page.
 * Saying so beforehand is the difference between that and looking like a crash.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useNavigate } from 'react-router'

import { ApiError, api } from '../../lib/api'
import { useI18n } from '../../lib/i18n'
import { Button, FormField, Glyph, Input, Spinner } from '../ui'

export function ChangePassword() {
  const { t } = useI18n()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [repeat, setRepeat] = useState('')

  const change = useMutation({
    mutationFn: () =>
      api.post('/auth/password', { currentPassword: current, newPassword: next }),
    onSuccess: () => {
      queryClient.clear()
      navigate('/login', { replace: true })
    },
  })

  const mismatch = repeat !== '' && next !== repeat
  const wrongCurrent = change.error instanceof ApiError && change.error.isUnauthorized

  return (
    <form
      className="grid gap-4 p-5 sm:grid-cols-2"
      onSubmit={(event) => {
        event.preventDefault()
        if (!mismatch) change.mutate()
      }}
    >
      <p className="max-w-prose text-xs leading-relaxed text-bone-faint sm:col-span-2">
        {t.admin.config.passwordTitle}. {t.admin.config.passwordBody}
      </p>

      <div className="sm:col-span-2">
        <FormField
          label={t.admin.config.currentPassword}
          htmlFor="current-password"
          error={
            wrongCurrent
              ? t.admin.config.passwordWrong
              : change.isError
                ? change.error.message
                : undefined
          }
        >
          <Input
            id="current-password"
            type="password"
            autoComplete="current-password"
            required
            value={current}
            onChange={(event) => setCurrent(event.target.value)}
          />
        </FormField>
      </div>

      <FormField
        label={t.admin.config.newPassword}
        htmlFor="new-password"
        hint={t.admin.config.passwordRule}
      >
        <Input
          id="new-password"
          type="password"
          autoComplete="new-password"
          required
          minLength={12}
          value={next}
          onChange={(event) => setNext(event.target.value)}
        />
      </FormField>

      <FormField
        label={t.admin.config.repeatPassword}
        htmlFor="repeat-password"
        error={mismatch ? t.admin.config.passwordMismatch : undefined}
      >
        <Input
          id="repeat-password"
          type="password"
          autoComplete="new-password"
          required
          value={repeat}
          onChange={(event) => setRepeat(event.target.value)}
        />
      </FormField>

      <div className="sm:col-span-2">
        <Button
          type="submit"
          variant="primary"
          disabled={change.isPending || mismatch || !current || next.length < 12}
        >
          {change.isPending ? <Spinner className="size-4" /> : <Glyph name="lock" className="size-4" />}
          {t.admin.config.passwordSubmit}
        </Button>
      </div>
    </form>
  )
}
