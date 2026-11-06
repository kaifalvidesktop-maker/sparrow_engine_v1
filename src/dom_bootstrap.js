// Sparrow DOM layer: runs inside boa. Rust injects __DOM/__URL/__TITLE/__VW/__VH first.
// Mutations are tracked with a dirty flag; Rust re-parses __serialize() output afterwards.
(function (g) {
  var VOID = { area: 1, base: 1, br: 1, col: 1, embed: 1, hr: 1, img: 1, input: 1, link: 1, meta: 1, source: 1, track: 1, wbr: 1 };
  var RAW = { script: 1, style: 1 };
  var dirty = false, nav = '', timers = [], tid = 1, now = 0, nid = 0;
  function mark() { dirty = true; }
  function err(x) { if (g.__log) g.__log('JS error: ' + (x && x.message ? x.message : x)); }
  function def(o, n, get, set) { Object.defineProperty(o, n, { get: get, set: set, configurable: true }); }
  function esc(s, a) {
    s = String(s).split('&').join('&amp;').split('<').join('&lt;').split('>').join('&gt;');
    return a ? s.split('"').join('&quot;') : s;
  }
  function unesc(s) {
    return s.split('&lt;').join('<').split('&gt;').join('>').split('&quot;').join('"').split('&#39;').join("'").split('&nbsp;').join('\u00a0').split('&amp;').join('&');
  }
  function kebab(k) { var o = ''; for (var i = 0; i < k.length; i++) { var c = k.charAt(i); o += (c >= 'A' && c <= 'Z') ? '-' + c.toLowerCase() : c; } return o; }
  function camel(k) { var o = '', up = false; for (var i = 0; i < k.length; i++) { var c = k.charAt(i); if (c === '-') up = true; else { o += up ? c.toUpperCase() : c; up = false; } } return o; }

  // ---------- Nodes ----------
  function Node(t) { this.nodeType = t; this.parentNode = null; this.childNodes = []; this._ls = {}; this._id = nid++; }
  var NP = Node.prototype;
  NP.removeChild = function (c) {
    var i = this.childNodes.indexOf(c);
    if (i < 0) throw new Error('NotFoundError: node is not a child');
    this.childNodes.splice(i, 1); c.parentNode = null; mark(); return c;
  };
  NP.insertBefore = function (c, r) {
    if (c.nodeType === 11) { var k = c.childNodes.slice(); for (var j = 0; j < k.length; j++) this.insertBefore(k[j], r); return c; }
    if (c.parentNode) c.parentNode.removeChild(c);
    var i = r ? this.childNodes.indexOf(r) : -1;
    if (i < 0) this.childNodes.push(c); else this.childNodes.splice(i, 0, c);
    c.parentNode = this; mark(); return c;
  };
  NP.appendChild = function (c) { return this.insertBefore(c, null); };
  NP.append = function () { for (var i = 0; i < arguments.length; i++) { var a = arguments[i]; this.appendChild(typeof a === 'object' ? a : new TextNode(String(a))); } };
  NP.prepend = function () { var f = this.childNodes[0] || null; for (var i = 0; i < arguments.length; i++) { var a = arguments[i]; this.insertBefore(typeof a === 'object' ? a : new TextNode(String(a)), f); } };
  NP.remove = function () { if (this.parentNode) this.parentNode.removeChild(this); };
  NP.replaceChild = function (n, o) { this.insertBefore(n, o); this.removeChild(o); return o; };
  NP.contains = function (o) { while (o) { if (o === this) return true; o = o.parentNode; } return false; };
  NP.hasChildNodes = function () { return this.childNodes.length > 0; };
  NP.cloneNode = function (deep) {
    var c;
    if (this.nodeType === 3) c = new TextNode(this._t);
    else if (this.nodeType === 8) c = new Comment(this._t);
    else if (this.nodeType === 1) {
      c = new Element(this.localName);
      for (var i = 0; i < this._k.length; i++) { c._k.push(this._k[i]); c._a[this._k[i]] = this._a[this._k[i]]; }
      var s = this._cssText(); if (s) parseStyle(c._st, s);
    } else c = new Frag();
    if (deep) for (var j = 0; j < this.childNodes.length; j++) c.appendChild(this.childNodes[j].cloneNode(true));
    return c;
  };
  def(NP, 'firstChild', function () { return this.childNodes[0] || null; });
  def(NP, 'lastChild', function () { return this.childNodes[this.childNodes.length - 1] || null; });
  def(NP, 'parentElement', function () { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; });
  def(NP, 'nextSibling', function () { var p = this.parentNode; if (!p) return null; return p.childNodes[p.childNodes.indexOf(this) + 1] || null; });
  def(NP, 'previousSibling', function () { var p = this.parentNode; if (!p) return null; return p.childNodes[p.childNodes.indexOf(this) - 1] || null; });
  def(NP, 'children', function () { return this.childNodes.filter(function (n) { return n.nodeType === 1; }); });
  def(NP, 'firstElementChild', function () { return this.children[0] || null; });
  def(NP, 'lastElementChild', function () { var c = this.children; return c[c.length - 1] || null; });
  def(NP, 'childElementCount', function () { return this.children.length; });
  def(NP, 'nextElementSibling', function () { var s = this.nextSibling; while (s && s.nodeType !== 1) s = s.nextSibling; return s; });
  def(NP, 'previousElementSibling', function () { return prevEl(this); });
  def(NP, 'ownerDocument', function () { return document; });
  def(NP, 'isConnected', function () { var n = this; while (n.parentNode) n = n.parentNode; return n === document; });
  function prevEl(n) { var s = n.previousSibling; while (s && s.nodeType !== 1) s = s.previousSibling; return s; }
  function tc(n) {
    if (n.nodeType === 3) return n._t;
    if (n.nodeType === 8) return '';
    var s = ''; for (var i = 0; i < n.childNodes.length; i++) s += tc(n.childNodes[i]); return s;
  }
  def(NP, 'textContent', function () { return tc(this); }, function (v) {
    for (var i = 0; i < this.childNodes.length; i++) this.childNodes[i].parentNode = null;
    this.childNodes = [];
    if (v !== '' && v != null) this.appendChild(new TextNode(String(v)));
    mark();
  });
  def(NP, 'innerText', function () { return tc(this); }, function (v) { this.textContent = v; });

  // events
  function Ev(type, init) {
    this.type = type; this.bubbles = !(init && init.bubbles === false); this.cancelable = true;
    this.defaultPrevented = false; this._stop = false; this.target = null; this.currentTarget = null; this.timeStamp = now;
    if (init && init.detail !== undefined) this.detail = init.detail;
  }
  Ev.prototype.preventDefault = function () { this.defaultPrevented = true; };
  Ev.prototype.stopPropagation = function () { this._stop = true; };
  Ev.prototype.stopImmediatePropagation = function () { this._stop = true; };
  NP.addEventListener = function (t, f) { (this._ls[t] = this._ls[t] || []).push(f); };
  NP.removeEventListener = function (t, f) { var l = this._ls[t]; if (l) this._ls[t] = l.filter(function (x) { return x !== f; }); };
  NP.dispatchEvent = function (e) {
    if (!e.target) e.target = this;
    var n = this;
    while (n) {
      e.currentTarget = n;
      var l = (n._ls && n._ls[e.type] || []).slice();
      for (var i = 0; i < l.length; i++) {
        try { if (typeof l[i] === 'function') l[i].call(n, e); else if (l[i] && l[i].handleEvent) l[i].handleEvent(e); } catch (x) { err(x); }
      }
      var h = n['on' + e.type];
      if (typeof h === 'function') { try { h.call(n, e); } catch (x2) { err(x2); } }
      if (e._stop || !e.bubbles) break;
      n = n.parentNode || (n === document ? g : null);
    }
    return !e.defaultPrevented;
  };

  function TextNode(s) { Node.call(this, 3); this._t = String(s); }
  TextNode.prototype = Object.create(NP);
  def(TextNode.prototype, 'textContent', function () { return this._t; }, function (v) { this._t = String(v); mark(); });
  def(TextNode.prototype, 'data', function () { return this._t; }, function (v) { this._t = String(v); mark(); });
  def(TextNode.prototype, 'nodeValue', function () { return this._t; }, function (v) { this._t = String(v); mark(); });
  function Comment(s) { Node.call(this, 8); this._t = String(s); }
  Comment.prototype = Object.create(NP);
  function Frag() { Node.call(this, 11); }
  Frag.prototype = Object.create(NP);
  function Doc() { Node.call(this, 9); }
  Doc.prototype = Object.create(NP);

  // ---------- Style ----------
  function parseStyle(st, text) {
    for (var k in st) delete st[k];
    var parts = String(text).split(';');
    for (var i = 0; i < parts.length; i++) {
      var p = parts[i], j = p.indexOf(':');
      if (j > 0) st[camel(p.substring(0, j).trim())] = p.substring(j + 1).trim();
    }
  }
  function mkStyle(el) {
    var st = {};
    function css() {
      var s = '';
      for (var k in st) if (st[k] !== '' && st[k] != null) s += kebab(k) + ': ' + st[k] + '; ';
      return s.trim();
    }
    Object.defineProperty(st, 'setProperty', { value: function (k, v) { st[camel(k)] = String(v); mark(); } });
    Object.defineProperty(st, 'getPropertyValue', { value: function (k) { var v = st[camel(k)]; return v == null ? '' : v; } });
    Object.defineProperty(st, 'removeProperty', { value: function (k) { delete st[camel(k)]; mark(); } });
    Object.defineProperty(st, 'cssText', { get: css, set: function (v) { parseStyle(st, v); mark(); } });
    el._st = st; el._cssText = css;
    if (typeof Proxy === 'function') {
      return new Proxy(st, {
        set: function (t, k, v) { t[k] = v; mark(); return true; },
        deleteProperty: function (t, k) { delete t[k]; mark(); return true; }
      });
    }
    return st;
  }

  // ---------- Element ----------
  function Element(tag) {
    Node.call(this, 1);
    this.localName = String(tag).toLowerCase(); this.tagName = this.localName.toUpperCase();
    this._a = {}; this._k = [];
    this.style = mkStyle(this);
  }
  Element.prototype = Object.create(NP);
  var EP = Element.prototype;
  EP._set = function (k, v) { if (!(k in this._a)) this._k.push(k); this._a[k] = v; };
  EP.setAttribute = function (k, v) {
    k = String(k).toLowerCase(); v = String(v);
    if (k === 'style') { parseStyle(this._st, v); mark(); return; }
    this._set(k, v); mark();
  };
  EP.getAttribute = function (k) {
    k = String(k).toLowerCase();
    if (k === 'style') { var s = this._cssText(); return s ? s : (k in this._a ? this._a[k] : null); }
    return k in this._a ? this._a[k] : null;
  };
  EP.hasAttribute = function (k) { return this.getAttribute(k) !== null; };
  EP.removeAttribute = function (k) {
    k = String(k).toLowerCase();
    if (k === 'style') { parseStyle(this._st, ''); mark(); return; }
    if (k in this._a) { delete this._a[k]; this._k.splice(this._k.indexOf(k), 1); mark(); }
  };
  def(EP, 'attributes', function () { var o = []; for (var i = 0; i < this._k.length; i++) o.push({ name: this._k[i], value: this._a[this._k[i]] }); return o; });
  function reflect(n, a) { def(EP, n, function () { return this._a[a] == null ? '' : this._a[a]; }, function (v) { this.setAttribute(a, v); }); }
  ['id', 'title', 'alt', 'src', 'href', 'value', 'type', 'name', 'placeholder', 'rel', 'target', 'action', 'method'].forEach(function (n) { reflect(n, n); });
  reflect('className', 'class'); reflect('htmlFor', 'for');
  function reflectBool(n) { def(EP, n, function () { return n in this._a; }, function (v) { if (v) this.setAttribute(n, ''); else this.removeAttribute(n); }); }
  ['hidden', 'disabled', 'checked', 'required', 'readOnly'].forEach(reflectBool);
  def(EP, 'classList', function () {
    var el = this;
    function list() { return (el._a['class'] || '').split(/\s+/).filter(Boolean); }
    function save(l) { el.setAttribute('class', l.join(' ')); }
    return {
      add: function () { var l = list(); for (var i = 0; i < arguments.length; i++) if (l.indexOf(arguments[i]) < 0) l.push(arguments[i]); save(l); },
      remove: function () { var a = arguments; save(list().filter(function (x) { for (var i = 0; i < a.length; i++) if (a[i] === x) return false; return true; })); },
      contains: function (c) { return list().indexOf(c) >= 0; },
      toggle: function (c, f) { var has = list().indexOf(c) >= 0, want = f === undefined ? !has : !!f; if (want && !has) this.add(c); if (!want && has) this.remove(c); return want; },
      item: function (i) { return list()[i] || null; },
      get length() { return list().length; }
    };
  });
  def(EP, 'dataset', function () {
    var el = this;
    if (typeof Proxy !== 'function') return {};
    return new Proxy({}, {
      get: function (t, k) { var v = el.getAttribute('data-' + kebab(String(k))); return v === null ? undefined : v; },
      set: function (t, k, v) { el.setAttribute('data-' + kebab(String(k)), v); return true; }
    });
  });
  function ser(n) {
    var i, s;
    if (n.nodeType === 3) return (n.parentNode && RAW[n.parentNode.localName]) ? n._t : esc(n._t, false);
    if (n.nodeType === 8) return '<!--' + n._t + '-->';
    if (n.nodeType === 1) {
      s = '<' + n.localName;
      for (i = 0; i < n._k.length; i++) s += ' ' + n._k[i] + '="' + esc(n._a[n._k[i]], true) + '"';
      var c = n._cssText();
      if (c) s += ' style="' + esc(c, true) + '"';
      s += '>';
      if (VOID[n.localName]) return s;
      for (i = 0; i < n.childNodes.length; i++) s += ser(n.childNodes[i]);
      return s + '</' + n.localName + '>';
    }
    s = ''; for (i = 0; i < n.childNodes.length; i++) s += ser(n.childNodes[i]); return s;
  }
  function inner(n) { var s = ''; for (var i = 0; i < n.childNodes.length; i++) s += ser(n.childNodes[i]); return s; }
  def(EP, 'outerHTML', function () { return ser(this); });
  def(EP, 'innerHTML', function () { return inner(this); }, function (h) {
    for (var i = 0; i < this.childNodes.length; i++) this.childNodes[i].parentNode = null;
    this.childNodes = [];
    var f = parseFragment(String(h));
    this.appendChild(f); mark();
  });
  EP.insertAdjacentHTML = function (pos, h) {
    var f = parseFragment(String(h));
    if (pos === 'beforeend') this.appendChild(f);
    else if (pos === 'afterbegin') this.insertBefore(f, this.childNodes[0] || null);
    else if (pos === 'beforebegin' && this.parentNode) this.parentNode.insertBefore(f, this);
    else if (pos === 'afterend' && this.parentNode) this.parentNode.insertBefore(f, this.nextSibling);
  };
  EP.click = function () { var e = new Ev('click'); this.dispatchEvent(e); return e; };
  EP.focus = EP.blur = EP.scrollIntoView = EP.scrollTo = function () {};
  EP.getBoundingClientRect = function () { return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 }; };
  ['offsetWidth', 'offsetHeight', 'clientWidth', 'clientHeight', 'offsetTop', 'offsetLeft', 'scrollTop', 'scrollLeft'].forEach(function (n) { def(EP, n, function () { return 0; }, function () {}); });

  // ---------- HTML fragment parser ----------
  function parseFragment(h) {
    var root = new Frag(), stack = [root], i = 0, n = h.length;
    function top() { return stack[stack.length - 1]; }
    while (i < n) {
      var c = h.charAt(i);
      if (c === '<') {
        if (h.substr(i, 4) === '<!--') {
          var ce = h.indexOf('-->', i + 4); if (ce < 0) ce = n;
          top().appendChild(new Comment(h.substring(i + 4, ce))); i = ce + 3; continue;
        }
        if (h.charAt(i + 1) === '/') {
          var e2 = h.indexOf('>', i); if (e2 < 0) e2 = n;
          var nm = h.substring(i + 2, e2).trim().toLowerCase();
          for (var k = stack.length - 1; k > 0; k--) if (stack[k].localName === nm) { stack.length = k; break; }
          i = e2 + 1; continue;
        }
        var m = /^<([a-zA-Z][a-zA-Z0-9-]*)/.exec(h.substr(i, 60));
        if (m) {
          var name = m[1].toLowerCase(), j = i + m[0].length, el = new Element(name), self = false;
          while (j < n) {
            var ch = h.charAt(j);
            if (ch === '>') { j++; break; }
            if (ch === '/' && h.charAt(j + 1) === '>') { self = true; j += 2; break; }
            if (ch === ' ' || ch === '\n' || ch === '\t' || ch === '\r' || ch === '/') { j++; continue; }
            var s0 = j; while (j < n && ' \n\t\r=>/'.indexOf(h.charAt(j)) < 0) j++;
            var an = h.substring(s0, j).toLowerCase(), av = '';
            if (h.charAt(j) === '=') {
              j++; var q = h.charAt(j);
              if (q === '"' || q === "'") { var qe = h.indexOf(q, j + 1); if (qe < 0) qe = n; av = h.substring(j + 1, qe); j = qe + 1; }
              else { var v0 = j; while (j < n && ' \n\t\r>'.indexOf(h.charAt(j)) < 0) j++; av = h.substring(v0, j); }
            }
            if (an === 'style') parseStyle(el._st, unesc(av)); else el._set(an, unesc(av));
          }
          top().appendChild(el);
          if (!self && !VOID[name]) {
            if (RAW[name]) {
              var re = h.toLowerCase().indexOf('</' + name, j); if (re < 0) re = n;
              if (re > j) el.appendChild(new TextNode(h.substring(j, re)));
              var rg = h.indexOf('>', re); j = rg < 0 ? n : rg + 1;
            } else stack.push(el);
          }
          i = j; continue;
        }
        top().appendChild(new TextNode('<')); i++; continue;
      }
      var t = h.indexOf('<', i); if (t < 0) t = n;
      top().appendChild(new TextNode(unesc(h.substring(i, t)))); i = t;
    }
    return root;
  }

  // ---------- Selectors ----------
  var selCache = {};
  function splitTop(s, ch) {
    var out = [], d = 0, cur = '';
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i);
      if (c === '(' || c === '[') d++; else if (c === ')' || c === ']') d--;
      if (c === ch && d === 0) { out.push(cur); cur = ''; } else cur += c;
    }
    out.push(cur); return out;
  }
  function isId(c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c === '-' || c === '_' || c > '\u007f'; }
  function parseCompound(s, i) {
    var c = { tag: null, id: null, cls: [], attrs: [], pseudo: [], never: false }, n = s.length;
    function ident() { var a = i; while (i < n && isId(s.charAt(i))) i++; return s.substring(a, i); }
    while (i < n) {
      var ch = s.charAt(i);
      if (ch === ' ' || ch === '>' || ch === '+' || ch === '~' || ch === ',') break;
      if (ch === '*') { c.tag = '*'; i++; }
      else if (ch === '#') { i++; c.id = ident(); }
      else if (ch === '.') { i++; c.cls.push(ident()); }
      else if (ch === '[') {
        var e = s.indexOf(']', i); if (e < 0) e = n;
        var body = s.substring(i + 1, e), op = '', nm = body, val = '', ops = ['^=', '$=', '*=', '~=', '|=', '='];
        for (var o = 0; o < ops.length; o++) { var p = body.indexOf(ops[o]); if (p > 0) { op = ops[o]; nm = body.substring(0, p); val = body.substring(p + ops[o].length); break; } }
        val = val.trim(); if (val.length > 1 && (val.charAt(0) === '"' || val.charAt(0) === "'")) val = val.substring(1, val.length - 1);
        c.attrs.push({ n: nm.trim().toLowerCase(), op: op, v: val }); i = e + 1;
      } else if (ch === ':') {
        i++; if (s.charAt(i) === ':') { c.never = true; i++; }
        var pn = ident(), arg = null;
        if (s.charAt(i) === '(') { var d = 1, a0 = ++i; while (i < n && d > 0) { var x = s.charAt(i); if (x === '(') d++; else if (x === ')') d--; i++; } arg = s.substring(a0, i - 1); }
        c.pseudo.push({ n: pn.toLowerCase(), a: arg });
      } else { var t = ident(); if (!t) { i++; c.never = true; } else c.tag = t.toLowerCase(); }
    }
    return { c: c, i: i };
  }
  function parseChain(s) {
    var chain = [], i = 0, comb = ' ', n = s.length;
    while (i < n) {
      var c = s.charAt(i);
      if (c === ' ') { i++; continue; }
      if (c === '>' || c === '+' || c === '~') { comb = c; i++; continue; }
      var r = parseCompound(s, i); chain.push({ c: r.c, comb: chain.length ? comb : '' }); comb = ' '; i = r.i;
    }
    return chain;
  }
  function parseSel(s) {
    if (selCache[s]) return selCache[s];
    var parts = splitTop(s, ','), out = [];
    for (var i = 0; i < parts.length; i++) { var t = parts[i].trim(); if (t) out.push(parseChain(t)); }
    return (selCache[s] = out);
  }
  function hasClass(el, c) { return (' ' + (el._a['class'] || '').split('\n').join(' ').split('\t').join(' ') + ' ').indexOf(' ' + c + ' ') >= 0; }
  function nth(expr, idx) {
    expr = expr.split(' ').join('').toLowerCase();
    if (expr === 'odd') expr = '2n+1'; else if (expr === 'even') expr = '2n';
    var p = expr.indexOf('n');
    if (p < 0) return parseInt(expr, 10) === idx;
    var as = expr.substring(0, p), a = as === '' ? 1 : as === '-' ? -1 : parseInt(as, 10), b = expr.length > p + 1 ? parseInt(expr.substring(p + 1), 10) : 0;
    for (var k = 0; k < 1000; k++) { var v = a * k + b; if (v === idx) return true; if (a >= 0 && v > idx) return false; if (a < 0 && v < 0) return false; }
    return false;
  }
  function elIndex(el) { var p = el.parentNode, k = 0; if (!p) return 1; for (var i = 0; i < p.childNodes.length; i++) { if (p.childNodes[i].nodeType === 1) k++; if (p.childNodes[i] === el) return k; } return k; }
  function elCount(el) { var p = el.parentNode, k = 0; if (!p) return 1; for (var i = 0; i < p.childNodes.length; i++) if (p.childNodes[i].nodeType === 1) k++; return k; }
  function cmatch(el, c) {
    if (el.nodeType !== 1 || c.never) return false;
    if (c.tag && c.tag !== '*' && el.localName !== c.tag) return false;
    if (c.id && el._a.id !== c.id) return false;
    var i;
    for (i = 0; i < c.cls.length; i++) if (!hasClass(el, c.cls[i])) return false;
    for (i = 0; i < c.attrs.length; i++) {
      var a = c.attrs[i], v = el.getAttribute(a.n);
      if (v === null) return false;
      if (a.op === '=' && v !== a.v) return false;
      if (a.op === '^=' && v.indexOf(a.v) !== 0) return false;
      if (a.op === '$=' && (v.length < a.v.length || v.substring(v.length - a.v.length) !== a.v)) return false;
      if (a.op === '*=' && v.indexOf(a.v) < 0) return false;
      if (a.op === '~=' && (' ' + v + ' ').indexOf(' ' + a.v + ' ') < 0) return false;
      if (a.op === '|=' && v !== a.v && v.indexOf(a.v + '-') !== 0) return false;
    }
    for (i = 0; i < c.pseudo.length; i++) {
      var p = c.pseudo[i];
      if (p.n === 'first-child') { if (elIndex(el) !== 1) return false; }
      else if (p.n === 'last-child') { if (elIndex(el) !== elCount(el)) return false; }
      else if (p.n === 'only-child') { if (elCount(el) !== 1) return false; }
      else if (p.n === 'nth-child') { if (!nth(p.a || '', elIndex(el))) return false; }
      else if (p.n === 'not') { var ch = parseChain(p.a || ''); if (ch.length === 1 && cmatch(el, ch[0].c)) return false; }
      else if (p.n === 'empty') { if (el.childNodes.length) return false; }
      else if (p.n === 'root') { if (el !== document.documentElement) return false; }
      else if (p.n === 'checked' || p.n === 'disabled') { if (!(p.n in el._a)) return false; }
      else return false; // :hover, :focus, ... never match (no interaction state yet)
    }
    return true;
  }
  function chainMatch(el, ch, i) {
    var p = ch[i];
    if (!cmatch(el, p.c)) return false;
    if (i === 0) return true;
    var cb = p.comb, s;
    if (cb === '>') return !!el.parentNode && chainMatch(el.parentNode, ch, i - 1);
    if (cb === ' ') { var a = el.parentNode; while (a) { if (chainMatch(a, ch, i - 1)) return true; a = a.parentNode; } return false; }
    if (cb === '+') { s = prevEl(el); return !!s && chainMatch(s, ch, i - 1); }
    s = prevEl(el); while (s) { if (chainMatch(s, ch, i - 1)) return true; s = prevEl(s); }
    return false;
  }
  function matches(el, sel) { var cs = parseSel(sel); for (var i = 0; i < cs.length; i++) if (cs[i].length && chainMatch(el, cs[i], cs[i].length - 1)) return true; return false; }
  function desc(root) {
    var out = [], st = root.childNodes.slice().reverse();
    while (st.length) { var n = st.pop(); out.push(n); for (var i = n.childNodes.length - 1; i >= 0; i--) st.push(n.childNodes[i]); }
    return out;
  }
  function qsa(root, sel) { var all = desc(root), out = []; for (var i = 0; i < all.length; i++) if (all[i].nodeType === 1 && matches(all[i], sel)) out.push(all[i]); return out; }
  function byTag(root, t) { t = t.toLowerCase(); return desc(root).filter(function (n) { return n.nodeType === 1 && (t === '*' || n.localName === t); }); }
  function byClass(root, c) { var cs = c.split(' ').filter(Boolean); return desc(root).filter(function (n) { if (n.nodeType !== 1) return false; for (var i = 0; i < cs.length; i++) if (!hasClass(n, cs[i])) return false; return cs.length > 0; }); }
  NP.querySelectorAll = function (s) { return qsa(this, s); };
  NP.querySelector = function (s) { return qsa(this, s)[0] || null; };
  NP.getElementsByTagName = function (t) { return byTag(this, t); };
  NP.getElementsByClassName = function (c) { return byClass(this, c); };
  EP.matches = function (s) { return matches(this, s); };
  EP.closest = function (s) { var n = this; while (n && n.nodeType === 1) { if (matches(n, s)) return n; n = n.parentNode; } return null; };

  // ---------- Document ----------
  var document = new Doc();
  var DP = Doc.prototype;
  DP.createElement = function (t) { return new Element(t); };
  DP.createTextNode = function (t) { return new TextNode(t); };
  DP.createComment = function (t) { return new Comment(t); };
  DP.createDocumentFragment = function () { return new Frag(); };
  DP.createEvent = function (t) { return new Ev(t); };
  DP.getElementById = function (id) { var a = desc(this); for (var i = 0; i < a.length; i++) if (a[i].nodeType === 1 && a[i]._a.id === id) return a[i]; return null; };
  function childTag(p, t) { if (!p) return null; for (var i = 0; i < p.childNodes.length; i++) if (p.childNodes[i].localName === t) return p.childNodes[i]; return null; }
  def(DP, 'documentElement', function () { return childTag(this, 'html'); });
  def(DP, 'head', function () { return childTag(this.documentElement, 'head'); });
  def(DP, 'body', function () { return childTag(this.documentElement, 'body'); });
  def(DP, 'title', function () { var t = byTag(this, 'title')[0]; return t ? tc(t).trim() : ''; }, function (v) { var t = byTag(this, 'title')[0]; if (t) t.textContent = v; });
  def(DP, 'scripts', function () { return byTag(this, 'script'); });
  def(DP, 'forms', function () { return byTag(this, 'form'); });
  def(DP, 'images', function () { return byTag(this, 'img'); });
  def(DP, 'links', function () { return byTag(this, 'a').filter(function (a) { return a.hasAttribute('href'); }); });
  document.readyState = 'loading'; document.cookie = ''; document.URL = g.__URL; document.domain = '';
  document.defaultView = g;

  // ---------- Hydrate from Rust ----------
  var D = g.__DOM, objs = [], i, d;
  for (i = 0; i < D.length; i++) {
    d = D[i];
    if (d[0] === 0) objs.push(document);
    else if (d[0] === 1) {
      var e = new Element(d[1]);
      for (var a = 0; a < d[2].length; a++) { if (d[2][a][0] === 'style') parseStyle(e._st, d[2][a][1]); else e._set(d[2][a][0], d[2][a][1]); }
      objs.push(e);
    } else if (d[0] === 2) objs.push(new TextNode(d[1]));
    else objs.push(new Comment(d[1]));
  }
  for (i = 0; i < D.length; i++) {
    var kids = D[i][3];
    for (var k = 0; k < kids.length; k++) { var ch = objs[kids[k]]; if (ch) { ch.parentNode = objs[i]; objs[i].childNodes.push(ch); } }
  }
  g.__DOM = null; dirty = false;

  // ---------- Window globals ----------
  g.window = g; g.self = g; g.top = g; g.parent = g; g.document = document; g.globalThis = g;
  g.Node = Node; g.Element = Element; g.Event = Ev; g.CustomEvent = Ev; g.HTMLElement = Element; g.Text = TextNode;
  g._ls = {}; g.parentNode = null;
  g.addEventListener = function (t, f) { NP.addEventListener.call(g, t, f); };
  g.removeEventListener = function (t, f) { NP.removeEventListener.call(g, t, f); };
  g.dispatchEvent = function (e) { return NP.dispatchEvent.call(g, e); };
  g.innerWidth = g.__VW; g.innerHeight = g.__VH; g.outerWidth = g.__VW; g.outerHeight = g.__VH; g.devicePixelRatio = 1;
  g.navigator = { userAgent: 'Sparrow/0.1', language: 'en-US', languages: ['en-US'], platform: 'Sparrow', onLine: true, cookieEnabled: false };
  var u = g.__URL, loc = { _h: u };
  def(loc, 'href', function () { return this._h; }, function (v) { nav = String(v); });
  loc.assign = loc.replace = function (v) { nav = String(v); };
  loc.reload = function () { nav = loc._h; };
  loc.toString = function () { return loc._h; };
  var m = /^([a-z]+:)\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/i.exec(u);
  loc.protocol = m ? m[1] : ''; loc.host = loc.hostname = m ? m[2] : ''; loc.pathname = m ? m[3] || '/' : ''; loc.search = m && m[4] ? m[4] : ''; loc.hash = m && m[5] ? m[5] : '';
  loc.origin = m ? m[1] + '//' + m[2] : 'null';
  g.location = loc; document.location = loc;
  g.history = { length: 1, state: null, pushState: function () {}, replaceState: function () {}, back: function () {}, forward: function () {}, go: function () {} };
  g.open = function (v) { if (v) nav = String(v); return null; };
  function Store() {
    var o = {};
    this.getItem = function (k) { return Object.prototype.hasOwnProperty.call(o, k) ? o[k] : null; };
    this.setItem = function (k, v) { o[k] = String(v); };
    this.removeItem = function (k) { delete o[k]; };
    this.clear = function () { o = {}; };
    this.key = function (n) { return Object.keys(o)[n] || null; };
    def(this, 'length', function () { return Object.keys(o).length; });
  }
  g.localStorage = new Store(); g.sessionStorage = new Store();
  g.getComputedStyle = function (el) { return el.style; };
  g.matchMedia = function (q) { return { matches: false, media: q, addListener: function () {}, removeListener: function () {}, addEventListener: function () {}, removeEventListener: function () {} }; };
  g.performance = { now: function () { return now; } };
  var NoopObs = function () { this.observe = this.unobserve = this.disconnect = function () {}; this.takeRecords = function () { return []; }; };
  g.MutationObserver = g.IntersectionObserver = g.ResizeObserver = NoopObs;
  g.fetch = function () { return Promise.reject(new Error('fetch is not supported yet')); };
  g.XMLHttpRequest = function () { this.open = this.send = this.setRequestHeader = function () {}; };
  g.alert = g.alert || function (m) { if (g.__log) g.__log('alert: ' + m); };
  g.confirm = function () { return false; }; g.prompt = function () { return null; };
  g.scrollTo = g.scroll = g.scrollBy = g.focus = g.blur = function () {};

  // ---------- Timers (virtual clock, driven from Rust) ----------
  g.setTimeout = function (f, ms) { var a = Array.prototype.slice.call(arguments, 2); timers.push({ id: tid, f: f, t: now + Math.max(0, +ms || 0), a: a, rep: -1 }); return tid++; };
  g.setInterval = function (f, ms) { var a = Array.prototype.slice.call(arguments, 2), r = Math.max(1, +ms || 1); timers.push({ id: tid, f: f, t: now + r, a: a, rep: r }); return tid++; };
  g.clearTimeout = g.clearInterval = function (id) { timers = timers.filter(function (t) { return t.id !== id; }); };
  g.requestAnimationFrame = function (f) { return g.setTimeout(function () { f(now); }, 16); };
  g.cancelAnimationFrame = g.clearTimeout;
  g.queueMicrotask = function (f) { g.setTimeout(f, 0); };

  // ---------- <video>/<audio> (HTMLMediaElement). Rust owns playback; JS queues commands "idx|op|value". ----------
  var mq = [];
  function mediaList() { return desc(document).filter(function (n) { return n.nodeType === 1 && (n.localName === 'video' || n.localName === 'audio'); }); }
  function mIdx(el) { return mediaList().indexOf(el); }
  function mst(el) { return el._m || (el._m = { paused: true, t: 0, d: NaN, v: 1, muted: false, ended: false }); }
  EP.play = function () { var s = mst(this); s.paused = false; mq.push(mIdx(this) + '|play|'); return Promise.resolve(); };
  EP.pause = function () { var s = mst(this); s.paused = true; mq.push(mIdx(this) + '|pause|'); };
  EP.load = function () { mq.push(mIdx(this) + '|load|' + (this._a.src || '')); };
  EP.canPlayType = function (t) { t = String(t).toLowerCase(); return (t.indexOf('audio/') === 0 || t.indexOf('mp4') > 0 || t.indexOf('ogg') > 0 || t.indexOf('mpeg') > 0 || t.indexOf('wav') > 0 || t.indexOf('flac') > 0) ? 'maybe' : ''; };
  def(EP, 'paused', function () { return mst(this).paused; });
  def(EP, 'ended', function () { return mst(this).ended; });
  def(EP, 'duration', function () { return mst(this).d; });
  def(EP, 'readyState', function () { return isNaN(mst(this).d) ? 0 : 4; });
  def(EP, 'currentTime', function () { return mst(this).t; }, function (v) { mst(this).t = +v || 0; mq.push(mIdx(this) + '|seek|' + (+v || 0)); });
  def(EP, 'volume', function () { return mst(this).v; }, function (v) { mst(this).v = Math.max(0, Math.min(1, +v)); mq.push(mIdx(this) + '|volume|' + mst(this).v); });
  def(EP, 'muted', function () { return mst(this).muted; }, function (v) { mst(this).muted = !!v; mq.push(mIdx(this) + '|muted|' + (v ? 1 : 0)); });
  ['autoplay', 'loop', 'controls'].forEach(reflectBool);
  reflect('poster', 'poster');
  var _sa = EP.setAttribute;
  EP.setAttribute = function (k, v) {
    _sa.call(this, k, v);
    if (String(k).toLowerCase() === 'src' && (this.localName === 'video' || this.localName === 'audio')) mq.push(mIdx(this) + '|load|' + String(v));
  };

  // ---------- Hooks called from Rust ----------
  g.__runTimers = function (budget, max) {
    var end = now + budget, ran = 0;
    while (ran < max) {
      var best = null;
      for (var i = 0; i < timers.length; i++) if (timers[i].t <= end && (!best || timers[i].t < best.t || (timers[i].t === best.t && timers[i].id < best.id))) best = timers[i];
      if (!best) break;
      now = Math.max(now, best.t);
      if (best.rep > 0) best.t += best.rep; else timers.splice(timers.indexOf(best), 1);
      try { if (typeof best.f === 'function') best.f.apply(g, best.a); } catch (x) { err(x); }
      ran++;
    }
    now = Math.max(now, end);
    return ran;
  };
  g.__fire = function (type) {
    if (type === 'DOMContentLoaded') { document.readyState = 'interactive'; document.dispatchEvent(new Ev(type)); }
    else { document.readyState = 'complete'; g.dispatchEvent(new Ev(type, { bubbles: false })); }
  };
  g.__fireOn = function (id, type) {
    var el = document.getElementById(id);
    if (!el) return false;
    var e = new Ev(type); el.dispatchEvent(e);
    if (type === 'click' && !e.defaultPrevented) {
      var a = el.closest('a'); if (a && a.hasAttribute('href')) nav = a.getAttribute('href');
    }
    return true;
  };
  g.__mediaCmds = function () { var s = mq.join('\n'); mq = []; return s; };
  g.__mediaState = function (i, paused, t, d, v, muted, ended) {
    var el = mediaList()[i]; if (!el) return;
    var s = mst(el); s.paused = !!paused; s.t = t; s.d = d; s.v = v; s.muted = !!muted; s.ended = !!ended;
  };
  g.__mediaEvent = function (i, type) { var el = mediaList()[i]; if (el) el.dispatchEvent(new Ev(type, { bubbles: false })); };
  g.__isDirty = function () { return dirty; };
  g.__serialize = function () { dirty = false; var h = document.documentElement; return h ? ser(h) : ''; };
  g.__takeNav = function () { var n = nav; nav = ''; return n; };
})(this);
