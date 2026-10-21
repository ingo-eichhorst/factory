// Before the first paint, and not a module: a module is deferred, and a page
// that renders dark then flips to light has already been seen doing it. Its
// own file rather than inline in index.html, so the UI's CSP can say
// `script-src 'self'` with no `'unsafe-inline'` (#343).
try {
  var saved = localStorage.getItem("factory-theme");
} catch (e) { }
document.documentElement.dataset.theme =
  saved === "foundry-light" ? "foundry-light" : "foundry-dark";
