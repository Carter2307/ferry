// Applies the saved theme, or the system's, before the first paint (no flash).
// External file rather than an inline script, like the dashboard's.
;(function () {
  var dark = false
  try {
    var saved = localStorage.getItem('ferry-landing-theme')
    dark = saved ? saved === 'dark' : window.matchMedia('(prefers-color-scheme: dark)').matches
  } catch (e) {
    dark = window.matchMedia('(prefers-color-scheme: dark)').matches
  }
  document.documentElement.classList.toggle('dark', dark)
})()
