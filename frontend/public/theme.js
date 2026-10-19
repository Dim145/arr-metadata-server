// Which theme the page wears, and which language it is read in, settled
// before the first paint: the ones the reader chose, or the system's where
// they have not. A file of its own rather than an inline script, which the
// content security policy refuses.
;(function () {
  var stored = null
  try {
    stored = localStorage.getItem('ams.theme')
  } catch (e) {
    // Private browsing, or storage disabled: the system decides.
  }
  var light =
    stored === 'light' ||
    (stored !== 'dark' && window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches)
  document.documentElement.dataset.theme = light ? 'light' : 'dark'
  var meta = document.querySelector('meta[name="theme-color"]')
  if (meta) meta.setAttribute('content', light ? '#f6f3ee' : '#0b0b0c')

  // The language the reader chose, under the key the interface keeps it in
  // (src/lib/i18n.tsx), so that the page is announced and hyphenated in it
  // from its first paint rather than once the interface has loaded and set
  // it. Only the shape of a language tag is asked of the value — the
  // interface stores nothing else there, and corrects a tag it does not
  // speak as soon as it runs. Nothing stored leaves the page's own `en`.
  var lang = null
  try {
    lang = localStorage.getItem('ams.lang')
  } catch (e) {
    // As above: English, which the page says already.
  }
  if (lang && /^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(lang)) document.documentElement.lang = lang
})()
