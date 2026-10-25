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
