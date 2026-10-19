/**
 * A season's pictures as the editor lists them: those the work serves, then
 * those a person hid, each saying which.
 *
 * A hidden picture is kept apart on the work, in `hidden.images`, so that no
 * answer carries it; the editor is the one place it is listed, struck
 * through, to be shown again. Without this a season's picture hidden
 * through the API was gone from every screen, and nothing could bring it
 * back.
 */

import { seasonImages } from '../../../lib/media'
import type { Image, MediaItem } from '../../../lib/types'

export interface SeasonPicture {
  image: Image
  hidden: boolean
}

export function seasonPictures(work: Pick<MediaItem, 'images' | 'seasons' | 'hidden'>, season: number): SeasonPicture[] {
  const shown = seasonImages(work, season).map((image) => ({ image, hidden: false }))
  const hidden = (work.hidden?.images ?? [])
    .filter((image) => image.seasonNumber === season)
    .map((image) => ({ image, hidden: true }))
  return [...shown, ...hidden]
}
