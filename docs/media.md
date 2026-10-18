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
  under `/media/`, and served immutable — with a thumbnail beside each
  picture, so a page of cards weighs what it did. The rows keep the
  providers' addresses: a work is rewritten as it is read, never as it is
  written, and asking for a copy to be forgotten puts the provider's address
  back. Sonarr, Radarr and the NFO documents are given the copies' addresses
  when `AMS_PUBLIC_URL` is set, and the providers' when it is not, since
  only this server's own pages can follow a path. A bucket's media go
  through this server, or, with **Sent to the bucket's own address**, a
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

The providers' terms still apply to what is kept: TMDB, TheTVDB and
Fanart.tv each say what may be done with their pictures.

The switches of the **Media** page — store, how a bucket's copies are served, the cast's
photographs, the themes — are settings: see [Media](settings.md#media) in the settings
reference.
