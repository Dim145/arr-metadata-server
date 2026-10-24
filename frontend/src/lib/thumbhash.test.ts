import { crc32, inflateSync } from 'node:zlib'

import { describe, expect, it } from 'vitest'

import { aspectRatio, hasAlpha, rgbaToDataURL, thumbHashToDataURL, thumbHashToRGBA } from './thumbhash'

/** What the server's ThumbHash crate made of two pictures, and drew them back as. */
const POSTER = 'UygOdRKIiICIuIiIiIaYeIiAiwi4' // 40 × 60: two thirds dark blue, the rest orange
const LOGO = 'PwiCAoAEh4d3h48AAAAAAI+PiHiIh3c=' // 80 × 31: white on nothing

const bytes = (base64: string) => Uint8Array.from(atob(base64), (c) => c.charCodeAt(0))
const near = (got: ArrayLike<number>, want: number[]) =>
  want.every((value, i) => Math.abs((got[i] ?? -999) - value) <= 1)

describe('a ThumbHash, drawn', () => {
  it('is the sketch the server’s own decoder draws, to a shade', () => {
    const sketch = thumbHashToRGBA(bytes(POSTER))!
    expect([sketch.w, sketch.h]).toEqual([23, 32])
    expect(near(sketch.rgba.subarray(0, 4), [85, 95, 125, 255])).toBe(true)
    expect(near(sketch.rgba.subarray(-4), [255, 119, 0, 255])).toBe(true)
    expect(aspectRatio(bytes(POSTER))).toBeCloseTo(5 / 7)
    expect(hasAlpha(bytes(POSTER))).toBe(false)
  })

  it('keeps a transparent picture’s transparency', () => {
    const sketch = thumbHashToRGBA(bytes(LOGO))!
    expect([sketch.w, sketch.h]).toEqual([32, 13])
    expect(near(sketch.rgba.subarray(0, 4), [255, 254, 252, 68])).toBe(true)
    expect(hasAlpha(bytes(LOGO))).toBe(true)
  })

  it('says nothing for a hash too short to hold a sketch, or not base64', () => {
    expect(thumbHashToRGBA(new Uint8Array([1, 2]))).toBeUndefined()
    expect(thumbHashToDataURL('not base64!')).toBeUndefined()
  })
})

describe('pixels as a PNG', () => {
  /** The chunks of a PNG: their types, data, and whether each checksum holds. */
  function chunks(png: Uint8Array) {
    const view = new DataView(png.buffer, png.byteOffset, png.byteLength)
    const out: { type: string; data: Uint8Array; checked: boolean }[] = []
    for (let at = 8; at < png.length; ) {
      const length = view.getUint32(at)
      const type = String.fromCharCode(...png.subarray(at + 4, at + 8))
      const data = png.subarray(at + 8, at + 8 + length)
      const crc = view.getUint32(at + 8 + length)
      out.push({ type, data, checked: crc32(png.subarray(at + 4, at + 8 + length)) === crc })
      at += 12 + length
    }
    return out
  }

  it('is a PNG any reader takes: its signature, its checksums, its rows', () => {
    const rgba = new Uint8Array([255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 9, 9, 9, 1, 2, 3, 4, 5, 6, 7, 8])
    const url = rgbaToDataURL(3, 2, rgba)
    expect(url.startsWith('data:image/png;base64,')).toBe(true)
    const png = bytes(url.slice('data:image/png;base64,'.length))
    expect([...png.subarray(0, 8)]).toEqual([137, 80, 78, 71, 13, 10, 26, 10])

    const found = chunks(png)
    expect(found.map((chunk) => chunk.type)).toEqual(['IHDR', 'IDAT', 'IEND'])
    expect(found.every((chunk) => chunk.checked)).toBe(true)
    expect([...found[0]!.data]).toEqual([0, 0, 0, 3, 0, 0, 0, 2, 8, 6, 0, 0, 0])
    // Each row behind its filter byte, none, and zlib's own checksum good.
    expect([...inflateSync(found[1]!.data)]).toEqual([0, ...rgba.subarray(0, 12), 0, ...rgba.subarray(12)])
  })

  it('draws a ThumbHash whole', () => {
    const url = thumbHashToDataURL(POSTER)!
    const png = bytes(url.slice('data:image/png;base64,'.length))
    const [header, data] = chunks(png)
    expect([...header!.data.subarray(0, 8)]).toEqual([0, 0, 0, 23, 0, 0, 0, 32])
    expect(inflateSync(data!.data).length).toBe(32 * (23 * 4 + 1))
  })
})
