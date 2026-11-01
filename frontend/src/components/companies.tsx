/**
 * The networks and the companies a work is credited on, as entities: each
 * with its logo, where it is from, and the way to its own page — the works
 * of this catalogue it is credited on, whatever it did for them.
 *
 * TMDB draws its logos for a white page, most of them black on nothing, so
 * each stands on a plaque of paper whatever the theme: on the site's dark
 * ink as in its light, it reads as the logo it is. It is fetched at the
 * size it is drawn at, as any of TMDB's pictures is.
 */

import { Link } from 'react-router'

import { cn } from '../lib/cn'
import { companyPath } from '../lib/facets'
import { useI18n } from '../lib/i18n'
import { countryName } from '../lib/releases'
import type { Company } from '../lib/types'
import { Artwork } from './media'

/** A company's logo on its plaque, or nothing where it has none. Decorative: its name is always beside it. */
export function CompanyLogo({ company, className }: { company: Pick<Company, 'logo' | 'name'>; className?: string }) {
  if (!company.logo) return null

  return (
    <span
      aria-hidden
      className={cn(
        'inline-grid shrink-0 place-items-center overflow-hidden rounded-card border border-rule bg-[#f4f2ed] p-1.5',
        // Under forced colours the plaque is the system's own surface.
        'forced-colors:bg-[Canvas]',
        className,
      )}
    >
      <Artwork
        url={company.logo}
        role="logo"
        alt=""
        sizes="160px"
        // Should neither the logo nor its original come, the initial stands
        // in, in the ink a logo is printed in.
        fallback={<span className="font-display text-base text-[#22201d]">{company.name.slice(0, 1)}</span>}
        className="max-h-full max-w-full object-contain"
      />
    </span>
  )
}

/**
 * A company as a line of the work's record: its logo, its name, where it is
 * from — the way to its page where it has one.
 */
export function CompanyLine({ company }: { company: Company }) {
  const { t, locale } = useI18n()
  const country = company.country ? countryName(company.country, locale) : undefined

  const face = (
    <>
      {company.logo ? (
        <CompanyLogo company={company} className="h-9 w-16" />
      ) : (
        <span aria-hidden className="grid h-9 w-16 shrink-0 place-items-center rounded-card border border-dashed border-rule-bright font-display text-sm text-bone-faint">
          {company.name.slice(0, 1)}
        </span>
      )}
      <span className="min-w-0">
        <span className="block truncate text-sm text-bone transition-colors duration-150 group-hover:text-vermillion">
          {company.name}
        </span>
        {country ? <span className="block truncate text-xs text-bone-faint">{country}</span> : null}
      </span>
    </>
  )

  return company.id ? (
    <Link
      to={companyPath(company.id)}
      title={t.work.companyPage(company.name)}
      className="group flex min-h-11 items-center gap-3 rounded-card py-1 focus-visible:outline-offset-2"
    >
      {face}
    </Link>
  ) : (
    <div className="flex min-h-11 items-center gap-3 py-1">{face}</div>
  )
}
