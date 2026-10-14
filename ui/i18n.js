'use strict';

// The dashboard in other languages. Pages are written in English; this layer replaces
// text and labels as they appear, so the rest of the code stays in one language. A
// language's dictionary (ui/i18n-<lang>.js) loads only when that language is chosen:
// by the browser's language, or from the command palette (saved per browser).
(function i18n() {
  const KEY = 'fusebox.lang';
  const LANGS = { en: 'English', zh: '中文' };
  const saved = (() => { try { return localStorage.getItem(KEY); } catch { return null; } })();
  const preferred = (navigator.languages || [navigator.language || '']).some((l) => /^zh\b/i.test(l)) ? 'zh' : 'en';
  const lang = LANGS[saved] ? saved : preferred;
  // Attributes people read, and where text is data rather than words.
  const ATTRS = ['title', 'placeholder', 'aria-label', 'aria-valuetext', 'data-toast'];
  const SKIP = 'script, style, code, pre, textarea, kbd, .mono, .code, [data-no-i18n]';
  let dict = null;

  // Exact text first (entries scoped to an element first of all), then patterns, then
  // a line of " · "-separated parts, part by part.
  function lookup(key, el) {
    if (el) {
      for (const [selector, map] of dict.scoped || []) {
        if (map[key] != null && el.closest(selector)) return map[key];
      }
    }
    if (dict.strings[key] != null) return dict.strings[key];
    for (const [pattern, replacement] of dict.patterns) {
      if (pattern.test(key)) return key.replace(pattern, replacement);
    }
    if (key.includes(' · ')) {
      const parts = key.split(' · ');
      const done = parts.map((p) => (p.trim() && /[A-Za-z]/.test(p) ? lookup(p.trim(), el) : p));
      if (done.some((p, i) => p != null && p !== parts[i])) return done.map((p, i) => p ?? parts[i]).join(' · ');
    }
    // Several sentences: each on its own, written together as Chinese is.
    const sentences = key.split(/(?<=[.?!])\s+(?=[A-Z])/);
    if (sentences.length > 1) {
      const done = sentences.map((s) => lookup(s, el));
      if (done.every((s) => s != null)) return done.join(dict.sentenceJoin ?? ' ');
    }
    return null;
  }

  function translate(text, el) {
    const key = text.trim();
    if (!key || key.length > 600 || !/[A-Za-z]/.test(key)) return null;
    const out = lookup(key, el);
    return out == null ? null : text.replace(key, out);
  }

  function translateText(node) {
    const parent = node.parentElement;
    if (!parent || parent.closest(SKIP)) return;
    const out = translate(node.nodeValue, parent);
    if (out != null && out !== node.nodeValue) node.nodeValue = out;
  }

  function translateAttrs(el) {
    if (el.closest(SKIP) && !el.matches('textarea, input')) return;
    for (const name of ATTRS) {
      const value = el.getAttribute(name);
      if (value == null) continue;
      const out = translate(value, el);
      if (out != null && out !== value) el.setAttribute(name, out);
    }
  }

  function translateTree(root) {
    if (root.nodeType === Node.TEXT_NODE) return translateText(root);
    if (root.nodeType !== Node.ELEMENT_NODE) return;
    translateAttrs(root);
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.nodeType === Node.TEXT_NODE) translateText(node); else translateAttrs(node);
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
  script.src = `/ui/i18n-${lang}.js`;
  script.onload = () => {
    dict = window.FUSEBOX_I18N?.[lang];
    if (!dict) return;
    // Patterns can translate the parts they capture (a status inside a sentence).
    window.FUSEBOX_I18N.tr = (text) => (text ? translate(text) ?? text : text);
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start); else start();
  };
  document.head.append(script);
})();
