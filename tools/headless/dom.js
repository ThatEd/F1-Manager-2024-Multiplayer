'use strict';
/*
 * dom.js - a small DOM host sufficient to run the game's Preact views headlessly.
 *
 * Why hand-rolled. The views are real Preact components that call `document.createElement`,
 * `appendChild`, `setAttribute`, `addEventListener` and `classList`. Node has no DOM, and pulling in
 * jsdom would add ~40 packages to a workspace that deliberately has no third-party JavaScript
 * dependencies. The surface the views actually touch is small and enumerable, so it is implemented
 * here and each part is asserted by a test.
 *
 * What this is NOT: a browser. There is no layout, no CSS parsing, no paint. `getBoundingClientRect`
 * returns zeros, which is correct for a headless view - code that needs real geometry (focus
 * navigation, drag) will misbehave, and that is a known limit rather than a bug.
 */

class ClassList {
  constructor(el) { this._el = el; this._set = new Set(); }
  add(...names) { for (const n of names) if (n) this._set.add(String(n)); this._sync(); }
  remove(...names) { for (const n of names) this._set.delete(String(n)); this._sync(); }
  contains(n) { return this._set.has(String(n)); }
  toggle(n, force) {
    const has = this.contains(n);
    const want = force === undefined ? !has : !!force;
    if (want) this.add(n); else this.remove(n);
    return want;
  }
  _sync() { if (this._el) this._el._className = [...this._set].join(' '); }
  get value() { return [...this._set].join(' '); }
  toString() { return this.value; }
}

/**
 * The browser returns `HTMLCollection` / `NodeList` from getElementsByClassName, getElementsByTagName,
 * querySelectorAll and `children` - array-likes with `.item(i)` and `.namedItem(name)`.
 *
 * This is not pedantry: common/components/Layer.js:13 calls `document.getElementsByClassName(
 * 'LayerCollection')?.item(0)`, so a plain Array throws and the whole TeamManagement render dies. An
 * Array subclass keeps iteration, `length`, spread and indexing working while adding the methods.
 */
class Collection extends Array {
  item(i) { const n = Number(i); return Number.isInteger(n) && n >= 0 && n < this.length ? this[n] : null; }
  namedItem(name) {
    const want = String(name);
    for (const el of this) {
      if (el && (el.id === want || (el.attributes && el.attributes.name === want))) return el;
    }
    return null;
  }
}

function collection(items) {
  const c = new Collection();
  for (const it of items) c.push(it);
  return c;
}

class Style {
  constructor() { this._props = {}; }
  setProperty(k, v) { this._props[k] = v; }
  getPropertyValue(k) { return this._props[k] || ''; }
  removeProperty(k) { delete this._props[k]; }
  get cssText() { return Object.entries(this._props).map(([k, v]) => `${k}: ${v}`).join('; '); }
  set cssText(v) { for (const part of String(v).split(';')) { const i = part.indexOf(':'); if (i > 0) this._props[part.slice(0, i).trim()] = part.slice(i + 1).trim(); } }
}

class Node {
  constructor(ownerDocument) {
    this.ownerDocument = ownerDocument || null;
    this.childNodes = [];
    this.parentNode = null;
  }
  get firstChild() { return this.childNodes[0] || null; }
  get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
  get nextSibling() {
    if (!this.parentNode) return null;
    const i = this.parentNode.childNodes.indexOf(this);
    return i >= 0 ? this.parentNode.childNodes[i + 1] || null : null;
  }
  get previousSibling() {
    if (!this.parentNode) return null;
    const i = this.parentNode.childNodes.indexOf(this);
    return i > 0 ? this.parentNode.childNodes[i - 1] : null;
  }
  appendChild(child) {
    if (!child) return child;
    if (child.parentNode) child.parentNode.removeChild(child);
    child.parentNode = this;
    child.ownerDocument = this.ownerDocument;
    this.childNodes.push(child);
    return child;
  }
  insertBefore(child, ref) {
    if (!ref) return this.appendChild(child);
    if (child.parentNode) child.parentNode.removeChild(child);
    const i = this.childNodes.indexOf(ref);
    child.parentNode = this;
    this.childNodes.splice(i < 0 ? this.childNodes.length : i, 0, child);
    return child;
  }
  removeChild(child) {
    const i = this.childNodes.indexOf(child);
    if (i >= 0) { this.childNodes.splice(i, 1); child.parentNode = null; }
    return child;
  }
  replaceChild(next, prev) {
    const i = this.childNodes.indexOf(prev);
    if (i >= 0) { this.childNodes[i] = next; next.parentNode = this; prev.parentNode = null; }
    return prev;
  }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  hasChildNodes() { return this.childNodes.length > 0; }
  cloneNode(deep) {
    const copy = this.nodeType === 3
      ? new Text(this.ownerDocument, this.data)
      : new Element(this.ownerDocument, this.nodeName);
    if (this.nodeType === 1) {
      copy._className = this._className;
      copy.classList._set = new Set(this.classList._set);
      copy.attributes = Object.assign({}, this.attributes);
      copy.id = this.id;
    }
    if (deep) for (const c of this.childNodes) copy.appendChild(c.cloneNode(true));
    return copy;
  }
  contains(other) {
    for (let n = other; n; n = n.parentNode) if (n === this) return true;
    return false;
  }
  get textContent() {
    let out = '';
    for (const c of this.childNodes) out += c.textContent != null ? c.textContent : '';
    return out;
  }
  set textContent(v) {
    this.childNodes.length = 0;
    if (v !== '' && v != null) this.appendChild(new Text(this.ownerDocument, String(v)));
  }
}

class Text extends Node {
  constructor(doc, data) { super(doc); this.data = data; this.nodeType = 3; this.nodeName = '#text'; }
  get textContent() { return this.data; }
  set textContent(v) { this.data = String(v); }
}

/**
 * A flattened view of a subtree: tag, classes, attributes and text, as plain JS.
 *
 * This is what the tests assert against. It is deliberately a *snapshot* rather than a live view, so
 * a test cannot accidentally pass because it re-read the tree after a mutation.
 */
function describe(node) {
  if (!node) return null;
  if (node.nodeType === 3) return { text: String(node.data || '').replace(/\s+/g, ' ').trim() };
  const out = {
    tag: String(node.nodeName || '').toLowerCase(),
    classes: [...(node.classList ? node.classList._set : [])].sort(),
    attrs: Object.assign({}, node.attributes),
  };
  const kids = (node.childNodes || []).map(describe).filter(Boolean);
  if (kids.length) out.children = kids;
  const text = (node.childNodes || []).filter((c) => c.nodeType === 3)
    .map((c) => String(c.data)).join('').replace(/\s+/g, ' ').trim();
  if (text) out.text = text;
  return out;
}

/** Every class anywhere in the subtree. Used to assert "did this screen actually render?". */
function allClasses(node) {
  const out = new Set();
  (function walk(n) {
    if (!n) return;
    if (n.classList) for (const c of n.classList._set) out.add(c);
    for (const c of n.childNodes || []) walk(c);
  })(node);
  return out;
}

class Element extends Node {
  constructor(doc, tagName) {
    super(doc);
    this.nodeType = 1;
    this.nodeName = String(tagName).toUpperCase();
    this.tagName = this.nodeName;
    this._className = '';
    this.classList = new ClassList(this);
    this.attributes = {};
    this.style = new Style();
    this.dataset = {};
    // NOTE the name: preact stores its own handler table at `node._listeners`, keyed by *event type*
    // and read by `eventProxy` (preact.js:219-226, written at :431). An earlier version of this file
    // used `_listeners` for a Map of addEventListener handlers, which preact then wrote an object
    // entry onto - so every onClick in the entire UI silently did nothing. The two must not share a
    // name; this one is the standard-DOM side.
    this._domListeners = new Map();
    this.value = '';
    this.checked = false;
    this.disabled = false;
    this.scrollTop = 0;
    this.scrollLeft = 0;
  }
  get className() { return this._className; }
  set className(v) {
    this._className = String(v || '');
    this.classList._set = new Set(this._className.split(/\s+/).filter(Boolean));
  }
  get id() { return this.attributes.id || ''; }
  set id(v) { this.setAttribute('id', v); }
  setAttribute(n, v) {
    const val = v == null ? '' : String(v);
    this.attributes[n] = val;
    if (n === 'class') this.className = val;
    if (n.startsWith('data-')) this.dataset[n.slice(5)] = val;
  }
  getAttribute(n) { return Object.prototype.hasOwnProperty.call(this.attributes, n) ? this.attributes[n] : null; }
  hasAttribute(n) { return Object.prototype.hasOwnProperty.call(this.attributes, n); }
  removeAttribute(n) { delete this.attributes[n]; }
  addEventListener(t, fn) {
    if (!this._domListeners.has(t)) this._domListeners.set(t, new Set());
    this._domListeners.get(t).add(fn);
  }
  removeEventListener(t, fn) { if (this._domListeners.has(t)) this._domListeners.get(t).delete(fn); }
  /** Fires listeners, then bubbles to the parent. Returns nothing; handlers are user code. */
  dispatchEvent(evt) {
    const e = evt || {};
    e.type = e.type || 'event';
    e.target = e.target || this;
    e.currentTarget = this;
    e.preventDefault = e.preventDefault || (() => {});
    e.stopPropagation = e.stopPropagation || (() => { e._stopped = true; });
    // Standard listeners only. preact installs ONE `eventProxy` per node+type through
    // addEventListener (preact.js:430) and then swaps the handler it forwards to via
    // `node._listeners[type]` (:431). Calling `_listeners[type]` directly here as well would invoke
    // the same handler twice - measured: one dispatched click incremented a counter by 2.
    for (const fn of this._domListeners.get(e.type) || []) fn.call(this, e);
    if (!e._stopped && this.parentNode && this.parentNode.dispatchEvent) this.parentNode.dispatchEvent(e);
  }
  click() { this.dispatchEvent({ type: 'click', button: 0 }); }
  focus() { if (this.ownerDocument) this.ownerDocument.activeElement = this; }
  blur() { if (this.ownerDocument && this.ownerDocument.activeElement === this) this.ownerDocument.activeElement = null; }
  getBoundingClientRect() { return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 }; }
  get children() { return collection(this.childNodes.filter((c) => c.nodeType === 1)); }
  get firstElementChild() { return this.children[0] || null; }
  querySelectorAll(sel) {
    const want = String(sel).trim();
    const match = (n) => {
      if (n.nodeType !== 1) return false;
      if (want.startsWith('.')) return n.classList.contains(want.slice(1));
      if (want.startsWith('#')) return n.id === want.slice(1);
      return n.nodeName.toLowerCase() === want.toLowerCase();
    };
    const out = [];
    (function walk(n) { for (const c of n.childNodes || []) { if (match(c)) out.push(c); walk(c); } })(this);
    return collection(out);
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  /**
   * Canvas 2D context stub.
   *
   * common/components/Canvas.js:38 calls `canvasElement.getContext('2d')` from a preact ref callback,
   * i.e. during the render walk. There is no rasteriser here and none is wanted - the goal is to know
   * whether a view mounts, not what its pixels are. Returning a context that accepts every drawing
   * call lets the component finish mounting; its `readPixels`-style probes get zeros.
   */
  getContext(kind) {
    if (String(kind) !== '2d') return null;
    if (this.__ctx2d) return this.__ctx2d;
    const noop = () => undefined;
    const grad = { addColorStop: noop };
    this.__ctx2d = {
      canvas: this,
      fillStyle: '#000', strokeStyle: '#000', lineWidth: 1, font: '10px sans-serif',
      globalAlpha: 1, globalCompositeOperation: 'source-over', textAlign: 'start', textBaseline: 'alphabetic',
      shadowBlur: 0, shadowColor: 'transparent', lineCap: 'butt', lineJoin: 'miter', miterLimit: 10,
      imageSmoothingEnabled: true, filter: 'none', direction: 'inherit',
      save: noop, restore: noop, scale: noop, rotate: noop, translate: noop, transform: noop, setTransform: noop, resetTransform: noop,
      beginPath: noop, closePath: noop, moveTo: noop, lineTo: noop, bezierCurveTo: noop, quadraticCurveTo: noop,
      arc: noop, arcTo: noop, ellipse: noop, rect: noop, roundRect: noop,
      fill: noop, stroke: noop, clip: noop, isPointInPath: () => false, isPointInStroke: () => false,
      fillRect: noop, strokeRect: noop, clearRect: noop,
      fillText: noop, strokeText: noop,
      measureText: (t) => ({ width: String(t == null ? '' : t).length * 6, actualBoundingBoxAscent: 8, actualBoundingBoxDescent: 2 }),
      drawImage: noop, putImageData: noop,
      getImageData: (x, y, w = 1, h = 1) => ({ width: w, height: h, data: new Uint8ClampedArray(Math.max(1, w * h * 4)) }),
      createImageData: (w = 1, h = 1) => ({ width: w, height: h, data: new Uint8ClampedArray(Math.max(1, w * h * 4)) }),
      createLinearGradient: () => grad, createRadialGradient: () => grad, createConicGradient: () => grad, createPattern: () => null,
      setLineDash: noop, getLineDash: () => [],
    };
    return this.__ctx2d;
  }
  getElementsByClassName(cls) {
    const out = [];
    (function walk(n) { for (const c of n.childNodes || []) { if (c.classList && c.classList.contains(cls)) out.push(c); walk(c); } })(this);
    return collection(out);
  }
  getElementsByTagName(tag) {
    const want = String(tag).toLowerCase();
    const out = [];
    (function walk(n) {
      for (const c of n.childNodes || []) {
        if (c.nodeType === 1 && (want === '*' || c.nodeName.toLowerCase() === want)) out.push(c);
        walk(c);
      }
    })(this);
    return collection(out);
  }
  /** Text-only convenience used by the tests. */
  get innerText() { return this.textContent; }
  get outerHTML() { return `<${this.nodeName.toLowerCase()} class="${this._className}">`; }
}

class Document extends Node {
  constructor() {
    super(null);
    this.ownerDocument = this;
    this.nodeType = 9;
    this.nodeName = '#document';
    this.activeElement = null;
    this.readyState = 'complete';
    this._byId = new Map();
    this.documentElement = new Element(this, 'html');
    this.head = new Element(this, 'head');
    this.body = new Element(this, 'body');
    this.appendChild(this.documentElement);
    this.documentElement.appendChild(this.head);
    this.documentElement.appendChild(this.body);
  }
  createElement(tag) { return new Element(this, tag); }
  createElementNS(_ns, tag) { return new Element(this, tag); }
  createTextNode(data) { return new Text(this, String(data)); }
  createDocumentFragment() { const f = new Element(this, '#fragment'); return f; }
  getElementById(id) {
    let found = null;
    (function walk(n) {
      if (found) return;
      if (n.nodeType === 1 && n.id === id) { found = n; return; }
      for (const c of n.childNodes || []) walk(c);
    })(this);
    return found;
  }
  getElementsByClassName(cls) { return this.documentElement.getElementsByClassName(cls); }
  getElementsByTagName(tag) {
    const want = String(tag).toLowerCase();
    if (want === 'head') return collection([this.head]);
    if (want === 'body') return collection([this.body]);
    if (want === 'html') return collection([this.documentElement]);
    return this.documentElement.getElementsByTagName(tag);
  }
  createComment(data) { const c = new Text(this, String(data)); c.nodeType = 8; c.nodeName = '#comment'; return c; }
  querySelectorAll(sel) { return this.documentElement.querySelectorAll(sel); }
  querySelector(sel) { return this.documentElement.querySelector(sel); }
  addEventListener() { /* document-level listeners are not used by the views */ }
  removeEventListener() {}
}

/** Builds a fresh window-ish global set for one view render. */
function createHost(opts = {}) {
  const document = new Document();
  const timers = new Set();
  const raf = new Set();
  // Custom properties declared on :root/html/body in the export's real stylesheets. ~20 call sites
  // read these through getComputedStyle(document.body) - every graph (DonutChart.js:34, PitGraph.js:146,
  // TrackGraph.js:22), LayoutUtil.js:16, Repeater.js:167. Serving '' would silently change behaviour,
  // and these come from the shipped CSS, not from guesses. Passed in so this module stays dependency-free.
  const rootVars = opts.rootVars || {};
  const win = {
    document,
    location: { pathname: '/Frontend.html', href: 'http://localhost/Frontend.html', search: '', hash: '' },
    // `getGamepads` is polled by common/debug/Gamepad.js:11 on a rAF loop; without it the poll throws
    // inside a timer and takes the process down. No pads attached is the honest headless answer.
    navigator: {
      userAgent: 'headless-coop',
      language: 'en-GB',
      platform: 'Win32',
      languages: ['en-GB', 'en'],
      getGamepads: () => [],
      maxTouchPoints: 0,
    },
    innerWidth: 1920,
    innerHeight: 1080,
    devicePixelRatio: 1,
    performance: { now: () => Number(process.hrtime.bigint() / 1000n) / 1000 },
    console,
    setTimeout: (fn, ms) => { const h = setTimeout(fn, ms); timers.add(h); return h; },
    clearTimeout: (h) => { clearTimeout(h); timers.delete(h); },
    setInterval: (fn, ms) => { const h = setInterval(fn, ms); timers.add(h); return h; },
    clearInterval: (h) => { clearInterval(h); timers.delete(h); },
    requestAnimationFrame: (fn) => { const h = setTimeout(() => fn(Date.now()), 16); raf.add(h); return h; },
    cancelAnimationFrame: (h) => { clearTimeout(h); raf.delete(h); },
    addEventListener: () => {},
    removeEventListener: () => {},
    getComputedStyle: (el) => {
      // Prefer an element's own inline custom properties, then the indexed :root set.
      const own = (el && el.style && el.style._props) || {};
      return {
        getPropertyValue: (k) => (Object.prototype.hasOwnProperty.call(own, k) ? own[k] : (rootVars[k] || '')),
      };
    },
    matchMedia: () => ({ matches: false, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {} }),
    localStorage: (() => {
      const m = new Map();
      return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), removeItem: (k) => m.delete(k), clear: () => m.clear() };
    })(),
  };
  win.window = win;
  win.self = win;
  win.globalThis = win;
  return {
    window: win,
    document,
    /** Drop every timer so a render cannot keep the process alive. */
    dispose() { for (const h of timers) { clearTimeout(h); clearInterval(h); } timers.clear(); for (const h of raf) clearTimeout(h); raf.clear(); },
  };
}

module.exports = { createHost, describe, allClasses, Element, Text, Document, Collection, collection };

