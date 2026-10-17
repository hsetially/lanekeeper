// Sets the theme class on <html> before the first paint, so a dark-mode user never sees a white flash. A classic script
// from the same origin (the CSP allows script-src 'self'; there is no inline script). It reads the `lk_theme` cookie
// that src/lib/prefs/theme.ts writes: `light`, `dark` or `system` (the default). No storage, no network, no eval.
(function () {
  var choice = "system";
  var cookies = document.cookie.split(";");
  for (var i = 0; i < cookies.length; i++) {
    var pair = cookies[i].trim();
    if (pair.indexOf("lk_theme=") === 0) {
      var value = pair.slice("lk_theme=".length);
      if (value === "light" || value === "dark" || value === "system") choice = value;
    }
  }
  var dark = choice === "dark" || (choice === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  if (dark) document.documentElement.classList.add("dark");
})();
