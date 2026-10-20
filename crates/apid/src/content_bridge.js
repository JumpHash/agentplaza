// The bridge between an HTML artifact's page and the console or the desktop app that frames it. The content
// origin serves it first in every page, so the page can report its height, open links through the host, hand
// files out for saving, and take the host's theme. Plain script, no dependencies.
(function () {
  'use strict';

  var inIpc = !!(window.ipc && typeof window.ipc.postMessage === 'function');

  // Sends one message to the host: the desktop webview's IPC when it exists, else the embedding page.
  function send(message) {
    message.apz = 1;
    if (inIpc) {
      window.ipc.postMessage(JSON.stringify(message));
    } else if (window.parent && window.parent !== window) {
      window.parent.postMessage(message, '*');
    }
  }

  // Height: reported on the next animation frame after the document resizes, and only when it changes.
  var lastHeight = -1;
  var pending = false;

  function reportHeight() {
    pending = false;
    var height = Math.ceil(document.documentElement.scrollHeight);
    if (height !== lastHeight) {
      lastHeight = height;
      send({ type: 'height', height: height });
    }
  }

  function scheduleHeight() {
    if (!pending) {
      pending = true;
      window.requestAnimationFrame(reportHeight);
    }
  }

  if (typeof window.ResizeObserver === 'function') {
    new window.ResizeObserver(scheduleHeight).observe(document.documentElement);
  }

  function ready() {
    send({ type: 'ready' });
    scheduleHeight();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', ready);
  } else {
    ready();
  }

  // Links that leave the page go to the host, which opens them outside the sandbox. Only web and mail links go.
  var OPENABLE = ['http:', 'https:', 'mailto:'];

  function resolve(href) {
    try {
      return new URL(href, document.baseURI);
    } catch (err) {
      return null;
    }
  }

  function leavesPage(url) {
    var here = window.location.href.split('#')[0];
    return url.href.split('#')[0] !== here;
  }

  // Every click on a link that leaves the page, with any modifier key, with the middle button, or into a new window,
  // goes to the host and nothing opens here. Capture phase, so the page's own handlers cannot stop the check.
  function onLink(event) {
    if (event.type === 'click' && event.button !== 0) {
      return;
    }
    if (event.type === 'auxclick' && event.button !== 1) {
      return;
    }
    var target = event.target;
    var anchor = target && target.closest ? target.closest('a[href]') : null;
    if (!anchor) {
      return;
    }
    var url = resolve(anchor.getAttribute('href'));
    var windowName = (anchor.getAttribute('target') || '').toLowerCase();
    var newWindow = windowName !== '' && windowName !== '_self';
    if (!url || (!newWindow && !leavesPage(url))) {
      return;
    }
    event.preventDefault();
    if (OPENABLE.indexOf(url.protocol) !== -1) {
      send({ type: 'open', url: url.href });
    }
  }

  document.addEventListener('click', onLink, true);
  document.addEventListener('auxclick', onLink, true);

  // Forms never submit: the CSP refuses every form action, and this takes away the default action too, so no form
  // `target` can open a window. The page's own submit handlers still run.
  document.addEventListener('submit', function (event) {
    event.preventDefault();
  }, true);

  window.open = function (href) {
    var url = href === undefined ? null : resolve(String(href));
    if (url && OPENABLE.indexOf(url.protocol) !== -1) {
      send({ type: 'open', url: url.href });
    }
    return null;
  };

  // A file the page makes (a CSV, an image as text) goes to the host to save: `apzDownload(name, mime, text)`.
  window.apzDownload = function (name, mime, text) {
    send({
      type: 'download',
      name: String(name),
      mime: String(mime || 'application/octet-stream'),
      data: String(text),
    });
  };

  // The host's theme: a map of custom properties and the mode (`light` or `dark`).
  window.__apzTheme = function (vars, mode) {
    var root = document.documentElement;
    if (vars && typeof vars === 'object') {
      Object.keys(vars).forEach(function (name) {
        if (/^--[A-Za-z0-9_-]+$/.test(name)) {
          root.style.setProperty(name, String(vars[name]));
        }
      });
    }
    if (mode === 'light' || mode === 'dark') {
      root.setAttribute('data-mode', mode);
      root.style.colorScheme = mode;
    }
  };

  window.addEventListener('message', function (event) {
    var message = event.data;
    if (event.source !== window.parent || !message || message.apz !== 1) {
      return;
    }
    if (message.type === 'theme') {
      window.__apzTheme(message.vars, message.mode);
    }
  });
})();
