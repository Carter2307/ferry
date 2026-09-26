// Applies the saved theme before first paint (no flash of the wrong theme).
// Loaded as a classic, render-blocking script from index.html; keep in sync
// with src/stores/ui.ts (key "ferry.ui", zustand persist shape).
;(function () {
  var pref = 'system'
  try {
    var raw = window.localStorage.getItem('ferry.ui')
    if (raw) {
      var parsed = JSON.parse(raw)
      if (parsed && parsed.state && typeof parsed.state.theme === 'string') pref = parsed.state.theme
    }
  } catch (e) {
    /* storage unavailable */
  }
  var dark = pref === 'dark' || (pref !== 'light' && window.matchMedia('(prefers-color-scheme: dark)').matches)
  var root = document.documentElement
  root.classList.toggle('dark', dark)
  root.style.colorScheme = dark ? 'dark' : 'light'
})()
