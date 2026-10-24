# Keeping the media

Every picture and theme a work points at can be fetched once and kept, so
the catalogue reads without its providers — and keeps reading when they are
gone. Set `AMS_MEDIA_STORAGE` to `filesystem` (files under `AMS_MEDIA_DIR`,
`data/media` by default) or `s3` (a bucket, Amazon's or anything that speaks
S3 — Garage, MinIO, RustFS, Ceph — with `AMS_S3_ENDPOINT`, `AMS_S3_REGION`,
`AMS_S3_BUCKET`, `AMS_S3_ACCESS_KEY` and `AMS_S3_SECRET_KEY`). Off by
default: nothing is kept until it is asked for.

- **What is kept.** The work's posters, backdrops, banners and logos, its
  seasons', its episodes' stills, its cast's photographs and a Fan-Kai's
  theme music. Never a trailer or a video. The cast's photographs and the
  themes are each a switch on **Administration › Media**, where the store's
  figures, the failures and the tasks are.
- **When.** As a work is stored — a refresh, a sync, an import — its media
  go in line, and a worker fetches them a few at a time; the work is served
  the moment its details are, and its pictures switch to the copies kept as
  they land. **Store everything** fetches what a catalogue held before the
  store was switched on. An address that fails is tried again a few hours
  later, five times; one that answers with something else is given up on at
  once. Both are listed on the page, to be tried again.
- **Where from.** An address is whatever somebody who may edit put on a work,
  so none is fetched from this machine, from a cloud's metadata service or
  from any other address only this server can reach — however it is written,
  and whatever it redirects to. A picture mirror on a private network is
  reached all the same; `AMS_MEDIA_PRIVATE_NETWORKS=false` keeps the store
  and the NFO export out of `10/8`, `172.16/12`, `192.168/16`, `100.64/10`
  and IPv6's unique-local `fc00::/7` too (`true` by default).
- **How they are served.** Every copy is addressed by the hash of its bytes
  under `/media/`, and served immutable. The rows keep the providers'
  addresses: a work is rewritten as it is read, never as it is written, and
  asking for a copy to be forgotten puts the provider's address back.
  Sonarr, Radarr, Plex, Kodi, Jellyfin and the NFO documents are given the
  pictures as they were fetched — JPEG and PNG, whole — at the copies'
  addresses when `AMS_PUBLIC_URL` is set, and at the providers' when it is
  not, since only this server's own pages can follow a path. This server's
  own pages are given smaller copies instead: see below. A bucket's media
  go through this server, or, with **Sent to the bucket's own address**, a
  reader is redirected to a presigned address good for an hour.
- **By hand.** From a work's page, a picture or a theme can be uploaded —
  read for what it is, never for what it is called; SVG is refused, since a
  document that can carry a script must not be served from here — and a
  copy taken away. An upload's file goes with it; a provider's copy is
  fetched again at the next refresh.
- **The sweep.** Daily, and on request: what no work points at any more —
  a work removed, a picture a provider dropped, an upload taken off — is
  forgotten and its file deleted, and a file no row names is removed once it
  has had its hour.
- **Moving the store.** The rows say which copies exist, not where: pointed
  at another bucket or directory, they name files that are not there.
  Either copy the files across first, or **Forget every copy** on the Media
  page and store everything again. A database transfer carries the rows,
  not the files.

## The copies the pages are given

A poster kept is two to three thousand pixels tall and a background up to
four thousand wide: what a client that keeps one picture wants, and ten
times what a card or a page draws. So this server's own pages are given
copies, made of each picture at the widths it is drawn at, once and twice
over for a dense screen, never wider than the picture and never wider than
1920 pixels:

| A picture drawn as | Widths |
|---|---|
| a poster, a season's poster, key art | 240, 480, 960 |
| a background | 640, 1280, 1920 |
| a landscape, a banner | 500, 1000 |
| a logo | 400, 800 |
| an episode's still | 320, 640 |
| a photograph of the cast | 120, 240 |

- **WebP.** Lossy, at quality 80, through libwebp's encoder — a poster's
  colours converted with its sharper method, so that thin lettering keeps
  its edges, and a transparency kept exact beside the colours; lossless
  for drawn art, a logo, clear art, character art or a disc, whose edges
  are its transparency and whose flat colours a lossless copy keeps in
  fewer bytes; a photograph transparent somewhere, a poster with rounded
  corners, weighed seven times as much that way. On fifty posters from
  TMDB, a 480-pixel WebP weighs 53 KB on average and takes 24 ms to write,
  against 83 KB for the JPEG thumbnail earlier versions made and 699 KB for
  the original.
- **Made once, when first asked for.** A copy is named by its picture's
  hash and its width — `/media/<hash>-480.webp` — and kept beside the
  picture in the store, in a directory of its own. The copy a page asks for
  first is made as the picture is stored: a poster's 480, a background's
  1280. Any other is made the first time a page asks for it — a few hundred
  milliseconds for a large background — and served from the store after.
  A width no page asks for is refused, so a picture has a dozen copies at
  the most. The thumbnails earlier versions made are still served; none is
  made any more.
- **Read safely.** A picture is decoded in Rust, never by libwebp: a
  picture past 16,384 pixels a side or 100 million pixels is not decoded
  at all, nor one whose pixels would take more than 256 MB, and a decoder
  that panics on what it is given is a copy refused, not a server brought
  down. Such a picture is served whole.
- **The square for Plex.** A background kept is also cut into a square
  from its middle, 1080 pixels a side, as a JPEG — Plex's
  `backgroundSquare`: see [Plex](plex.md#what-is-served).

### What a page draws before the picture is there

As a picture is stored, the colour most of it is and a
[ThumbHash](https://evanw.github.io/thumbhash/) of it — a blurred sketch in
two dozen bytes, its proportions and its transparency included — are worked
out and kept with it. The native API gives both, with the picture's size,
for the pictures a card or a page leads with (`pictures`, beside a work's
`images`); the interface paints the colour behind the frame at once and the
sketch under the picture until it arrives, and gives the picture's box its
proportions before a byte of it is there.

### Preparing what was kept before

Pictures stored before this version have neither a placeholder nor their
first copy. Their copies are made all the same, as pages ask for them; the
task **Prepare the pictures** (`media.variants`, on **Tasks** and on the
Media page, which says how many are left) works out their placeholders and
makes their first copies, a few at a time. It runs only when somebody runs
it — never at start: a catalogue's pictures are hours of decoding on a small
machine. It can be stopped; each picture is noted as it is done, so the
next run takes up where the last one stopped.

### AVIF

AVIF is a third lighter than WebP, and seconds to make. With **AVIF copies
at night** on (the setting `media.avif`, off until it is set), the task
`media.avif` makes, each night at 03:00 UTC, an AVIF copy of every poster
at 480 and 960 pixels and of every background at 1280 and 1920, one after
another on one core; it can also be run by hand, and stopped. A page offers
the AVIF copies, through a `<picture>`, only where they exist and reach as
wide as the WebP ones they stand for: a browser looks at the one source it
picks, and a narrower copy would draw a dense screen's picture soft. A
background's 1920 takes about a second and a half at night. No browser is asked
what it reads: the addresses are fixed, and every browser the interface
supports reads both.

Measured on fifty posters from TMDB, on one core of an Apple M-series
processor, at quality 68 and rav1e's speed 6:

| | 480 wide | 960 wide |
|---|---|---|
| AVIF, rav1e's assembly | 316 ms, 37.7 KB | 1,044 ms, 104 KB |
| AVIF, Rust alone | 732 ms, 37.7 KB | 2,439 ms, 104 KB |
| WebP, quality 80 | 24 ms, 53.9 KB | 110 ms, 160 KB |

The container image is built with rav1e's assembly (the `avif-asm`
feature; NASM assembles it on x86_64); a build from source has it with
`cargo build --release --features avif-asm`. Making them takes about 170 MB
of memory at its peak.

The providers' terms still apply to what is kept: TMDB, TheTVDB and
Fanart.tv each say what may be done with their pictures.

The switches of the **Media** page — store, how a bucket's copies are served, the cast's
photographs, the themes, the AVIF copies — are settings: see [Media](settings.md#media) in
the settings reference.
