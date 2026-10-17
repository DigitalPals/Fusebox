'use strict';

// The dashboard in other languages. Pages are written in English; this layer replaces
// text and labels as they appear, so the rest of the code stays in one language. A
// language's dictionary (ui/i18n-<lang>.js) loads only when that language is chosen:
// by the browser's language, or from the command palette (saved per browser).
(function i18n() {
  const KEY = 'fusebox.lang';
  const LANGS = { en: 'English', zh: '中文' };
  // Written out whole so the server can version each dictionary's URL.
  const DICTS = { zh: '/ui/i18n-zh.js' };
  const saved = (() => { try { return localStorage.getItem(KEY); } catch { return null; } })();
  const preferred = (navigator.languages || [navigator.language || '']).some((l) => /^zh\b/i.test(l)) ? 'zh' : 'en';
  const lang = LANGS[saved] ? saved : preferred;
  // Attributes people read, and where text is data rather than words.
  const ATTRS = ['title', 'placeholder', 'aria-label', 'aria-valuetext', 'data-toast'];
  const SKIP = 'script, style, code, pre, textarea, kbd, .mono, .code, [data-no-i18n]';
  let dict = null;

  // Live pages repeat the same text (times, counts, labels) all the time, and text with no
  // translation is the costly case: every pattern is tried. So answers are remembered,
  // those that never depend on where the text is, up to a bound.
  const cache = new Map();
  const CACHE_MAX = 4000;
  let scoped = false; // the current lookup met an entry scoped to an element
  function remember(key, out) {
    if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value);
    cache.set(key, out);
  }
  function cached(key, el) {
    if (cache.has(key)) return cache.get(key);
    const outer = scoped;
    scoped = false;
    const out = lookup(key, el);
    if (!scoped) remember(key, out);
    scoped = outer || scoped;
    return out;
  }

  // Exact text first (entries scoped to an element first of all), then patterns, then
  // a line of " · "-separated parts, part by part.
  function lookup(key, el) {
    for (const [selector, map] of dict.scoped || []) {
      if (map[key] == null) continue;
      scoped = true;
      if (el && el.closest(selector)) return map[key];
    }
    if (dict.strings[key] != null) return dict.strings[key];
    for (const [pattern, replacement] of dict.patterns) {
      if (pattern.test(key)) return key.replace(pattern, replacement);
    }
    if (key.includes(' · ')) {
      const parts = key.split(' · ');
      const done = parts.map((p) => (p.trim() && /[A-Za-z]/.test(p) ? cached(p.trim(), el) : p));
      if (done.some((p, i) => p != null && p !== parts[i])) return done.map((p, i) => p ?? parts[i]).join(' · ');
    }
    // Several sentences: each on its own, written together as Chinese is.
    const sentences = key.split(/(?<=[.?!])\s+(?=[A-Z])/);
    if (sentences.length > 1) {
      const done = sentences.map((s) => cached(s, el));
      if (done.every((s) => s != null)) return done.join(dict.sentenceJoin ?? ' ');
    }
    return null;
  }

  function translate(text, el) {
    const key = text.trim();
    if (!key || key.length > 600 || !/[A-Za-z]/.test(key)) return null;
    const out = cached(key, el);
    return out == null ? null : text.replace(key, out);
  }

  // What this layer wrote last, so its own changes are not translated a second time.
  const wroteText = new WeakMap();
  const wroteAttrs = new WeakMap();

  function setText(node) {
    const out = translate(node.nodeValue, node.parentElement);
    if (out != null && out !== node.nodeValue) {
      node.nodeValue = out;
      wroteText.set(node, out);
    }
  }

  function translateText(node) {
    const parent = node.parentElement;
    if (!parent || wroteText.get(node) === node.nodeValue || parent.closest(SKIP)) return;
    setText(node);
  }

  function setAttrs(el) {
    let wrote = wroteAttrs.get(el);
    for (const name of ATTRS) {
      const value = el.getAttribute(name);
      if (value == null || (wrote && wrote[name] === value)) continue;
      const out = translate(value, el);
      if (out != null && out !== value) {
        el.setAttribute(name, out);
        if (!wrote) wroteAttrs.set(el, (wrote = {}));
        wrote[name] = out;
      }
    }
  }

  function translateAttrs(el) {
    if (el.closest(SKIP) && !el.matches('textarea, input')) return;
    setAttrs(el);
  }

  // Text is data inside SKIP elements; only the attributes of fields there are read.
  function skipped(el) {
    if (el.matches('textarea, input')) setAttrs(el);
    for (const field of el.querySelectorAll('textarea, input')) setAttrs(field);
  }

  // A new subtree, walked once. Elements matching SKIP are passed over whole, so their
  // descendants need no ancestor check of their own.
  const FILTER = {
    acceptNode(node) {
      if (node.nodeType === Node.TEXT_NODE || !node.matches(SKIP)) return NodeFilter.FILTER_ACCEPT;
      skipped(node);
      return NodeFilter.FILTER_REJECT;
    },
  };
  function translateTree(root) {
    if (root.nodeType === Node.TEXT_NODE) return translateText(root);
    if (root.nodeType !== Node.ELEMENT_NODE) return;
    if (root.closest(SKIP)) return skipped(root);
    setAttrs(root);
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, FILTER);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.nodeType === Node.TEXT_NODE) setText(node); else setAttrs(node);
    }
  }

  function start() {
    document.documentElement.lang = 'zh-CN';
    translateTree(document.body);
    new MutationObserver((records) => {
      for (const r of records) {
        if (r.type === 'characterData') translateText(r.target);
        else if (r.type === 'attributes') translateAttrs(r.target);
        else r.addedNodes.forEach(translateTree);
      }
    }).observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  }

  window.fuseboxLanguage = {
    current: lang,
    names: LANGS,
    // Changing language reloads the page, so nothing is left half-translated.
    set(next) {
      try { localStorage.setItem(KEY, next); } catch {}
      location.reload();
    },
  };

  if (lang === 'en') return;
  const script = document.createElement('script');
  script.src = DICTS[lang];
  script.onload = () => {
    dict = window.FUSEBOX_I18N?.[lang];
    if (!dict) return;
    // Patterns can translate the parts they capture (a status inside a sentence).
    window.FUSEBOX_I18N.tr = (text) => (text ? translate(text) ?? text : text);
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start); else start();
  };
  document.head.append(script);
})();
