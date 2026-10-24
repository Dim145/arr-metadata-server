/**
 * A ThumbHash, drawn: the blurred sketch the server worked out of a picture
 * it keeps, in two dozen bytes, made into a tiny image to lie under the
 * picture until the picture arrives.
 *
 * The decoder is Evan Wallace's (github.com/evanw/thumbhash), as the Rust
 * crate the server encodes with has it — MIT licence, © 2023 Evan Wallace —
 * written out here rather than taken as a package, so that the interface
 * carries the eighty lines it uses. The PNG it is wrapped in is this file's
 * own: stored, uncompressed, which for a picture of 32 pixels a side is a
 * few kilobytes no one downloads.
 */

/** The sketch's pixels: RGBA, 32 pixels on its longer side. */
export interface Sketch {
  w: number
  h: number
  rgba: Uint8Array
}

/** The sketch a ThumbHash holds, or nothing for one too short to hold any. */
export function thumbHashToRGBA(hash: Uint8Array): Sketch | undefined {
  if (hash.length < 5) return undefined
  const { PI, cos, max, min, round } = Math
  const header24 = hash[0]! | (hash[1]! << 8) | (hash[2]! << 16)
  const header16 = hash[3]! | (hash[4]! << 8)
  const lDc = (header24 & 63) / 63
  const pDc = ((header24 >> 6) & 63) / 31.5 - 1
  const qDc = ((header24 >> 12) & 63) / 31.5 - 1
  const lScale = ((header24 >> 18) & 31) / 31
  const hasAlpha = header24 >> 23 !== 0
  const pScale = ((header16 >> 3) & 63) / 63
  const qScale = ((header16 >> 9) & 63) / 63
  const isLandscape = header16 >> 15 !== 0
  const lMax = hasAlpha ? 5 : 7
  const lx = max(3, isLandscape ? lMax : header16 & 7)
  const ly = max(3, isLandscape ? header16 & 7 : lMax)
  const aDc = hasAlpha ? (hash[5]! & 15) / 15 : 1
  const aScale = hasAlpha ? (hash[5]! >> 4) / 15 : 1

  // The varying factors, four bits each, two to a byte: the saturation
  // boosted by a quarter for what the quantising took away.
  let at = hasAlpha ? 6 : 5
  let half = 0
  const channel = (nx: number, ny: number, scale: number): number[] => {
    const ac: number[] = []
    for (let cy = 0; cy < ny; cy++) {
      for (let cx = cy ? 0 : 1; cx * ny < nx * (ny - cy); cx++) {
        const byte = hash[at] ?? 0
        const bits = half ? byte >> 4 : byte & 15
        if (half) at++
        half ^= 1
        ac.push((bits / 7.5 - 1) * scale)
      }
    }
    return ac
  }
  const lAc = channel(lx, ly, lScale)
  const pAc = channel(3, 3, pScale * 1.25)
  const qAc = channel(3, 3, qScale * 1.25)
  const aAc = hasAlpha ? channel(5, 5, aScale) : []

  const ratio = aspectRatio(hash)
  const w = ratio > 1 ? 32 : round(32 * ratio)
  const h = ratio > 1 ? round(32 / ratio) : 32
  const rgba = new Uint8Array(w * h * 4)
  const fx: number[] = []
  const fy: number[] = []
  for (let y = 0, i = 0; y < h; y++) {
    for (let x = 0; x < w; x++, i += 4) {
      let l = lDc
      let p = pDc
      let q = qDc
      let a = aDc
      for (let cx = 0, n = max(lx, hasAlpha ? 5 : 3); cx < n; cx++) fx[cx] = cos((PI / w) * (x + 0.5) * cx)
      for (let cy = 0, n = max(ly, hasAlpha ? 5 : 3); cy < n; cy++) fy[cy] = cos((PI / h) * (y + 0.5) * cy)

      for (let cy = 0, j = 0; cy < ly; cy++) {
        const fy2 = fy[cy]! * 2
        for (let cx = cy ? 0 : 1; cx * ly < lx * (ly - cy); cx++, j++) l += lAc[j]! * fx[cx]! * fy2
      }
      for (let cy = 0, j = 0; cy < 3; cy++) {
        const fy2 = fy[cy]! * 2
        for (let cx = cy ? 0 : 1; cx < 3 - cy; cx++, j++) {
          const f = fx[cx]! * fy2
          p += pAc[j]! * f
          q += qAc[j]! * f
        }
      }
      if (hasAlpha) {
        for (let cy = 0, j = 0; cy < 5; cy++) {
          const fy2 = fy[cy]! * 2
          for (let cx = cy ? 0 : 1; cx < 5 - cy; cx++, j++) a += aAc[j]! * fx[cx]! * fy2
        }
      }

      const b = l - (2 / 3) * p
      const r = (3 * l - b + q) / 2
      const g = r - q
      rgba[i] = max(0, 255 * min(1, r))
      rgba[i + 1] = max(0, 255 * min(1, g))
      rgba[i + 2] = max(0, 255 * min(1, b))
      rgba[i + 3] = max(0, 255 * min(1, a))
    }
  }
  return { w, h, rgba }
}

/** The proportions of the picture a ThumbHash was made of, roughly. */
export function aspectRatio(hash: Uint8Array): number {
  const hasAlpha = (hash[2]! & 0x80) !== 0
  const lMax = hasAlpha ? 5 : 7
  const lMin = hash[3]! & 7
  const isLandscape = (hash[4]! & 0x80) !== 0
  return isLandscape ? lMax / lMin : lMin / lMax
}

/** Whether the picture a ThumbHash was made of is transparent somewhere. */
export function hasAlpha(hash: Uint8Array): boolean {
  return hash.length > 2 && (hash[2]! & 0x80) !== 0
}

/** The CRC-32 a PNG chunk ends with, IEEE's polynomial. */
const CRC = (() => {
  const table = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    table[n] = c >>> 0
  }
  return table
})()

function crc32(bytes: Uint8Array, from: number, to: number): number {
  let c = 0xffffffff
  for (let i = from; i < to; i++) c = CRC[(c ^ bytes[i]!) & 0xff]! ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

/**
 * Pixels as a PNG in a `data:` address: RGBA, each row its own stored
 * deflate block, which no inflater needs more than the length of to read.
 */
export function rgbaToDataURL(w: number, h: number, rgba: Uint8Array): string {
  const row = w * 4 + 1
  // zlib's two bytes, a five-byte header for each row's block, the rows,
  // and the Adler-32 at the end.
  const idat = 2 + h * (5 + row) + 4
  const bytes = new Uint8Array(8 + 25 + 12 + idat + 12)
  const view = new DataView(bytes.buffer)
  let at = 0
  const put = (...values: number[]) => {
    for (const value of values) bytes[at++] = value
  }
  const chunk = (type: string, length: number, write: () => void) => {
    view.setUint32(at, length)
    at += 4
    const start = at
    put(...[...type].map((c) => c.charCodeAt(0)))
    write()
    view.setUint32(at, crc32(bytes, start, at))
    at += 4
  }

  put(137, 80, 78, 71, 13, 10, 26, 10)
  chunk('IHDR', 13, () => {
    view.setUint32(at, w)
    view.setUint32(at + 4, h)
    at += 8
    // Eight bits a channel, RGBA, the standard compression, filter and no
    // interlacing.
    put(8, 6, 0, 0, 0)
  })
  chunk('IDAT', idat, () => {
    put(0x78, 0x01)
    let a = 1
    let b = 0
    for (let y = 0; y < h; y++) {
      put(y + 1 < h ? 0 : 1, row & 0xff, row >> 8, ~row & 0xff, (~row >> 8) & 0xff)
      // Each row behind its filter byte, none.
      const line = [0, ...rgba.subarray(y * w * 4, (y + 1) * w * 4)]
      for (const value of line) {
        put(value)
        a = (a + value) % 65521
        b = (b + a) % 65521
      }
    }
    view.setUint32(at, ((b << 16) | a) >>> 0)
    at += 4
  })
  chunk('IEND', 0, () => {})

  let text = ''
  for (const byte of bytes) text += String.fromCharCode(byte)
  return `data:image/png;base64,${btoa(text)}`
}

/** A ThumbHash as the native API sends it, base64, drawn. */
export function thumbHashToDataURL(base64: string): string | undefined {
  let hash: Uint8Array
  try {
    hash = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0))
  } catch {
    return undefined
  }
  const sketch = thumbHashToRGBA(hash)
  return sketch ? rgbaToDataURL(sketch.w, sketch.h, sketch.rgba) : undefined
}
