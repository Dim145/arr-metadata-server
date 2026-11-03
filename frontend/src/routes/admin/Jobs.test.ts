import { describe, expect, it } from 'vitest'

import { en } from '../../lib/lang/en'
import { fr } from '../../lib/lang/fr'
import { describeDetail } from './Jobs'

describe('what a run of IMDb’s lists says of itself, in words', () => {
  it('says how far a reading has got, and what it has kept so far', () => {
    expect(describeDetail('212 of 519: megabytes read, 1740 titles kept', en, 'en')).toBe(
      '212 of 519 MB read, 1740 titles kept',
    )
    expect(describeDetail('12 megabytes read, 1 episodes kept', en, 'en')).toBe('12 MB read, 1 episode kept')
    expect(describeDetail('212 of 519: megabytes read, 1740 titles kept', fr, 'fr')).toBe(
      '212 Mo lus sur 519, 1740 titres gardés',
    )
  })

  it('says what a run kept, and how far one that was stopped got', () => {
    expect(describeDetail('5 titles for 1 of 2 works', en, 'en')).toBe('5 titles for 1 of 2 works')
    expect(describeDetail('stopped: 0 titles for 0 works', en, 'en')).toBe(
      'Stopped: 0 titles written for 0 works; the others keep theirs',
    )
    expect(describeDetail('4 episodes of 1 of 1 series', fr, 'fr')).toBe('4 épisodes de 1 série sur 1')
    expect(describeDetail('stopped: 2000 episodes for 12 series', fr, 'fr')).toBe(
      'Arrêtée : 2000 épisodes écrits pour 12 séries ; les autres gardent les leurs',
    )
  })

  it('says how many works the ratings rated, and how many episodes', () => {
    expect(describeDetail('2 of 2 works rated, 1 of 4 episodes', en, 'en')).toBe('2 of 2 works rated, 1 of 4 episodes')
    expect(describeDetail('26 of 27 works rated', fr, 'fr')).toBe('26 œuvres notées sur 27')
  })
})

describe('what a pass over the answers kept says of itself, in words', () => {
  it('says how many works it filled in, and how many it passed over', () => {
    expect(describeDetail('12 filled in, 3 passed over', en, 'en')).toBe('12 works filled in, 3 passed over')
    expect(describeDetail('1 filled in, 0 passed over', en, 'en')).toBe('1 work filled in, 0 passed over')
    expect(describeDetail('12 filled in, 1 passed over', fr, 'fr')).toBe('12 œuvres complétées, 1 laissée de côté')
    expect(describeDetail('0 filled in, 2 passed over', fr, 'fr')).toBe('0 œuvre complétée, 2 laissées de côté')
  })
})

describe('what a run of TVmaze’s index, Wikidata’s links and TheTVDB’s deletions says of itself, in words', () => {
  it('says how far a reading of TVmaze’s index has got, and what it kept', () => {
    expect(describeDetail('12 pages read, 3000 series kept', en, 'en')).toBe('12 pages read, 3,000 series kept')
    expect(describeDetail('1 pages read, 245 series kept', fr, 'fr')).toBe('1 page lue, 245 séries gardées')
    expect(describeDetail('91234 series from 382 pages, 3 no longer listed, 1 unreadable', en, 'en')).toBe(
      '91,234 series from 382 pages, 3 no longer listed, 1 entry unreadable',
    )
    expect(describeDetail('91234 series from 382 pages', fr, 'fr')).toBe('91 234 séries sur 382 pages')
    expect(describeDetail('stopped: 500 series from 2 pages', en, 'en')).toBe('Stopped: 500 series kept from 2 pages')
  })

  it('says how many of the series TVmaze changed were read again', () => {
    expect(describeDetail('35 of 1700: series read again', en, 'en')).toBe('35 of 1,700 changed series read again')
    expect(describeDetail('1600 of 1695 series changed in the week read again, 1 gone: 91234 in all', en, 'en')).toBe(
      '1,600 of the 1,695 series changed in the past week read again, 1 gone: 91,234 in all',
    )
    expect(describeDetail('2 of 3 series changed in the day read again, 0 gone: 10 in all', fr, 'fr')).toBe(
      '2 des 3 séries changées depuis la veille relues, 0 disparue : 10 en tout',
    )
    expect(describeDetail('stopped: 12 of 1700 changed series read again, 0 gone', fr, 'fr')).toBe(
      'Arrêtée : 12 sur 1 700 séries changées relues, 0 disparue',
    )
  })

  it('says how many links Wikidata gave, on how many items', () => {
    expect(describeDetail('1016710 links on 359851 items: 1016710 new, 0 gone', en, 'en')).toBe(
      '1,016,710 links on 359,851 items: 1,016,710 new, 0 gone',
    )
    expect(describeDetail('200 links on 100 items: 1 new, 2 gone', fr, 'fr')).toBe(
      '200 liens sur 100 éléments : 1 nouveau, 2 disparus',
    )
  })

  it('says what TheTVDB deleted, and what became of the series held here', () => {
    expect(describeDetail('TheTVDB deleted 104 series since 2026-10-27 00:00 UTC; none held here', en, 'en')).toBe(
      'TheTVDB deleted 104 series since Oct 27, 2026, 12:00 AM UTC; none held here',
    )
    expect(
      describeDetail(
        'TheTVDB deleted 2 series since 2026-10-27 00:00 UTC; numbered as they were served: “Gone” (merged into 81189), “Away”; 1 held here numbered so already',
        fr,
        'fr',
      ),
    ).toBe(
      'TheTVDB a supprimé 2 séries depuis le 27 oct. 2026, 00:00 UTC ; numérotées comme elles étaient servies : “Gone” (fusionnée dans 81189), “Away” ; 1 présente ici l’était déjà',
    )
  })
})
