/**
 * What stands in for a page that threw.
 *
 * An error while a page renders takes down everything above it that has no
 * boundary to stop at, and a page that is handed a value the server did not
 * send in the shape it expects — a proxy's HTML in place of JSON, an address
 * that reached the wrong endpoint — used to take the whole app with it: a
 * blank window, the bar gone. Caught here, the page says it could not be
 * shown and offers to ask again, and whatever surrounds it stays where it was.
 */

import { Component, type ReactNode } from 'react'
import { useLocation } from 'react-router'

import { useI18n } from '../lib/i18n'
import { Unavailable } from '../routes/NotFound'

/**
 * A screen whose code could not be fetched: the server was updated beneath a
 * tab that was already open, and the file it asks for is no longer there.
 * Asking again cannot find it, and the browser remembers the refusal; reading
 * the page again does.
 */
const STALE = /dynamically imported module|importing a module script|loading (css )?chunk/i

interface Props {
  /** Where the page is: another one is another chance. */
  at: string
  admin: boolean
  hint: string
  children: ReactNode
}

interface State {
  failed: boolean
  error: unknown
}

class Boundary extends Component<Props, State> {
  state: State = { failed: false, error: undefined }

  static getDerivedStateFromError(error: unknown): State {
    return { failed: true, error }
  }

  componentDidUpdate(before: Props) {
    // What threw was the page before this one.
    if (this.state.failed && before.at !== this.props.at) {
      this.setState({ failed: false, error: undefined })
    }
  }

  render() {
    if (!this.state.failed) return this.props.children

    const stale = this.state.error instanceof Error && STALE.test(this.state.error.message)

    return (
      <Unavailable
        admin={this.props.admin}
        hint={this.props.hint}
        onRetry={() => {
          if (stale) window.location.reload()
          else this.setState({ failed: false, error: undefined })
        }}
      />
    )
  }
}

export function PageBoundary({ children }: { children: ReactNode }) {
  const { t } = useI18n()
  const { pathname, search } = useLocation()

  return (
    <Boundary at={`${pathname}${search}`} admin={pathname.startsWith('/admin')} hint={t.common.errorHint}>
      {children}
    </Boundary>
  )
}
