class Element extends EventTarget {
  className = ""; classList = { add: (name) => { this.className = `${this.className} ${name}`; } };
  dataset = {}; children = []; attributes = new Map(); disabled = false; parent = null; value = "";
  constructor(tag, text = "") { super(); this.tag = tag; this.text = text; }
  get parentElement() { return this.parent; }
  get firstChild() { return this.children[0]; }
  before(...nodes) {
    const parent = this.parent;
    for (const node of nodes) {
      node.remove();
      parent.children.splice(parent.children.indexOf(this), 0, node);
      node.parent = parent;
    }
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) { return this.descendants().filter((node) => node.matches(selector)); }
  matches(selector) {
    if (selector.endsWith(":not(:disabled)")) return !this.disabled && this.matches(selector.slice(0, -":not(:disabled)".length));
    if (selector === ":disabled") return this.disabled;
    if (selector.startsWith(".")) return this.className.split(" ").includes(selector.slice(1));
    if (selector === "[data-focus-key]") return this.dataset.focusKey !== undefined;
    const attribute = /^\[([^=]+)="([^"]+)"\]$/.exec(selector);
    if (attribute) return (attribute[1] === "data-focus-key" ? this.dataset.focusKey : this.attributes.get(attribute[1])) === attribute[2];
    return this.tag === selector;
  }
  closest(selector) { return selector.split(", ").some((part) => part === "[hidden]" && this.hidden || part === "[inert]" && this.inert) ?
    this : this.parent?.closest(selector) ?? null; }
  getClientRects() { return this.isConnected ? [{}] : []; }
  scrollIntoView() {}
  get isConnected() { return this === document.body || this.parent?.isConnected === true; }
  get textContent() { return this.text + this.children.map((child) => child.textContent).join(""); }
  set textContent(text) { this.replaceChildren(); this.text = String(text); }
  contains(node) { return this === node || this.children.some((child) => child.contains(node)); }
  descendants() { return this.children.flatMap((child) => [child, ...child.descendants()]); }
  append(...children) { for (const value of children) { const child = typeof value === "string" ? new Element("#text", value) : value; child.remove(); child.parent = this; this.children.push(child); } }
  replaceChildren(...children) { for (const child of [...this.children]) child.remove(); this.append(...children); }
  remove() { if (!this.parent) return; this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; }
  setAttribute(name, value) { this.attributes.set(name, value); }
  focus() { if (this.isConnected && !this.disabled) document.activeElement = this; }
  click() { if (!this.disabled) this.dispatchEvent(new Event("click")); }
}
export function installNodes(t, bindings = {}) {
  const body = new Element("body");
  const globals = { Node: Element, HTMLElement: Element, window: new EventTarget(),
    document: Object.assign(new EventTarget(), { body, activeElement: body, visibilityState: "visible", createElement: (tag) => new Element(tag),
      createElementNS: (_ns, tag) => new Element(tag), createTextNode: (text) => new Element("#text", text) }), ...bindings };
  for (const [name, value] of Object.entries(globals)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, name);
    Object.defineProperty(globalThis, name, { value, configurable: true });
    t.after(() => original ? Object.defineProperty(globalThis, name, original) : Reflect.deleteProperty(globalThis, name));
  }
  return body;
}
