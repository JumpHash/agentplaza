// First-paint theming for the web console. A plain synchronous script, loaded in <head> before the stylesheet, so the
// first frame already has the right skin, palette, mode and density. It reads the look the console last applied on
// this device (ui/theme.ts caches the effective one, the device override included, with its resolved skin) first, the
// per-device override second, field by field, and falls back to the defaults. Nothing here is exported: the only global
// is window.__apzBoot, which records the resolved mode for the app.
(function () {
  'use strict';

  var root = document.documentElement;
  var DEVICE_KEY = 'apz.appearance.device';
  var CACHE_KEY = 'apz.appearance';
  var DEFAULTS = { theme: 'loom-clay', skin: 'loom', mode: 'system', density: 'default', css: '' };
  var SKINS = { loom: true, graphite: true };
  var MODES = { system: true, light: true, dark: true };
  var DENSITIES = { compact: true, default: true, comfortable: true };
  var THEME_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;

  // Reads one stored choice. Any failure (no storage, blocked storage, bad JSON) gives null.
  function readStored(key) {
    try {
      var raw = window.localStorage.getItem(key);
      if (!raw) return null;
      var value = JSON.parse(raw);
      return value && typeof value === 'object' ? value : null;
    } catch (e) {
      return null;
    }
  }

  // Takes the field from the cached effective look when it is valid, else from the device override, else the default.
  function pick(field, valid, cached, device) {
    if (cached && valid(cached[field])) return cached[field];
    if (device && valid(device[field])) return device[field];
    return DEFAULTS[field];
  }

  var device = readStored(DEVICE_KEY);
  var cached = readStored(CACHE_KEY);
  var isString = function (v) { return typeof v === 'string'; };
  var settings = {
    theme: pick('theme', function (v) { return isString(v) && THEME_ID.test(v); }, cached, device),
    skin: pick('skin', function (v) { return isString(v) && SKINS[v] === true; }, cached, device),
    mode: pick('mode', function (v) { return isString(v) && MODES[v] === true; }, cached, device),
    density: pick('density', function (v) { return isString(v) && DENSITIES[v] === true; }, cached, device),
    css: pick('css', isString, cached, device),
  };

  // System mode follows the OS preference; the query is kept so a change of the OS setting updates the page.
  var lightQuery = null;
  try {
    lightQuery = window.matchMedia ? window.matchMedia('(prefers-color-scheme: light)') : null;
  } catch (e) {
    lightQuery = null;
  }
  function resolve(mode) {
    if (mode === 'light' || mode === 'dark') return mode;
    if (!lightQuery) return 'light';
    return lightQuery.matches ? 'light' : 'dark';
  }

  var resolved = resolve(settings.mode);
  root.setAttribute('data-skin', settings.skin);
  root.setAttribute('data-palette', settings.theme);
  root.setAttribute('data-mode', resolved);
  // The older light/dark attribute: the terminal and the engine views still watch it, so it follows the resolved mode.
  root.setAttribute('data-theme', resolved);
  if (settings.density === 'default') root.removeAttribute('data-density');
  else root.setAttribute('data-density', settings.density);
  // The cached code size, when it is a sane number; otherwise the stylesheet's own default holds (ui/theme.ts sets the same).
  var codeSize = cached && typeof cached.code_size === 'number' ? cached.code_size : 0;
  if (codeSize >= 9 && codeSize <= 24) root.style.setProperty('--code-size', codeSize + 'px');

  window.__apzBoot = { mode: resolved };

  // A saved or imported theme's own CSS. It is appended now so the first frame has it, and moved to the end of <head>
  // once the document has parsed (the stylesheet link comes after this script), so its rules win ties against the app's.
  var customStyle = null;
  if (settings.css && settings.css.trim() !== '') {
    try {
      customStyle = document.createElement('style');
      customStyle.id = 'apz-custom-theme';
      customStyle.textContent = settings.css;
      (document.head || root).appendChild(customStyle);
      document.addEventListener('DOMContentLoaded', function () {
        if (customStyle && customStyle.parentNode) customStyle.parentNode.appendChild(customStyle);
      });
    } catch (e) {
      customStyle = null;
    }
  }

  // In system mode the page follows the OS preference as it changes.
  if (settings.mode === 'system' && lightQuery) {
    var onChange = function (event) {
      var mode = event.matches ? 'light' : 'dark';
      root.setAttribute('data-mode', mode);
      root.setAttribute('data-theme', mode);
      window.__apzBoot.mode = mode;
    };
    try {
      if (lightQuery.addEventListener) lightQuery.addEventListener('change', onChange);
      else if (lightQuery.addListener) lightQuery.addListener(onChange);
    } catch (e) {
      // No listener: the page keeps the mode it resolved at first paint.
    }
  }
})();
