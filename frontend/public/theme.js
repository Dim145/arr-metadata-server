// Which theme the page wears, settled before the first paint: the one the
// reader chose, or the system's where they have not. A file of its own rather
// than an inline script, which the content security policy refuses.
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
})()
