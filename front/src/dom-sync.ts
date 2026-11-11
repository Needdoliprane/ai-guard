// Keeps DOM nodes across renders. A node that is not replaced keeps the keyboard focus, a text selection and a
// click in progress, is not announced again by assistive technology, and lets the browser anchor its scroll position.
const rowState = new WeakMap<Element, { key: string; signature: string }>();
const markup = new WeakMap<Element, string>();
const serialized = new WeakMap<object, string>();

/** JSON of a value the control returned, computed once per object. */
export function signatureOf(value: object): string {
  let text = serialized.get(value);
  if (text === undefined) { text = JSON.stringify(value); serialized.set(value, text); }
  return text;
}

export interface KeyedRow { key: string; signature: string; build: () => HTMLElement }
/** The values of one longest strictly increasing subsequence of `values`. */
function longestIncreasing(values: number[]): Set<number> {
  // ends[length - 1]: position in `values` of the smallest value that ends an increasing run of that length.
  const ends: number[] = []; const previous: number[] = [];
  values.forEach((value, index) => {
    let low = 0; let high = ends.length;
    while (low < high) { const middle = (low + high) >> 1; if (values[ends[middle]!]! < value) low = middle + 1; else high = middle; }
    previous[index] = low ? ends[low - 1]! : -1; ends[low] = index;
  });
  const run = new Set<number>();
  for (let index = ends.at(-1) ?? -1; index >= 0; index = previous[index]!) run.add(values[index]!);
  return run;
}
/**
 * Makes `parent` hold exactly these rows, in this order. A row whose key and signature did not change keeps
 * its node, and keeps its place whenever it can: a browser takes the keyboard focus and a text selection
 * away from a node that leaves its parent, even to be inserted again at once. Rows that changed or left are
 * removed first; among the kept rows a longest run already in the wanted order is not touched, and only the
 * others are moved. A kept row that had to move is given back the focus it held; a text selection in it is
 * lost. Keys are unique within one call.
 */
export function syncRows(parent: HTMLElement, rows: KeyedRow[]): void {
  const page = parent.ownerDocument; const active = page.activeElement as HTMLElement | null;
  const held = active && parent.contains(active) ? active : null;
  const wanted = new Map(rows.map((row, index) => [row.key, { index, signature: row.signature }]));
  const kept = new Map<number, Element>(); const order: number[] = [];
  for (const node of [...parent.children]) {
    const state = rowState.get(node); const target = state ? wanted.get(state.key) : undefined;
    if (state && target && !kept.has(target.index) && target.signature === state.signature) { kept.set(target.index, node); order.push(target.index); }
    else node.remove();
  }
  const inPlace = longestIncreasing(order);
  // From the last row to the first: each row that is built or has to move goes right before its successor.
  let next: Element | null = null;
  for (let index = rows.length - 1; index >= 0; index--) {
    const row = rows[index]!; let node = kept.get(index);
    if (!node) { node = row.build(); rowState.set(node, { key: row.key, signature: row.signature }); parent.insertBefore(node, next); }
    else if (!inPlace.has(index)) parent.insertBefore(node, next);
    next = node;
  }
  // A node that left the page for good is not focused again: the view decides where that focus goes.
  if (held?.isConnected && page.activeElement !== held) held.focus({ preventScroll: true });
}

/** Makes `parent` hold exactly these nodes, in this order, without detaching a node that is already in place. */
export function place(parent: HTMLElement, ...nodes: (Node | null | false | undefined)[]): void {
  const wanted = nodes.filter((node): node is Node => !!node);
  for (const child of [...parent.childNodes]) if (!wanted.includes(child)) child.remove();
  let cursor: ChildNode | null = parent.firstChild;
  for (const node of wanted) { if (node === cursor) cursor = cursor.nextSibling; else parent.insertBefore(node, cursor); }
}

/** Replaces the children of `box` only when their markup changed. Returns true when it did. */
export function replaceIfChanged(box: HTMLElement, ...nodes: (HTMLElement | null)[]): boolean {
  const kept = nodes.filter((node): node is HTMLElement => node !== null);
  const next = kept.map((node) => node.outerHTML).join("\n");
  if (markup.get(box) === next) return false;
  markup.set(box, next); box.replaceChildren(...kept);
  return true;
}

/**
 * Makes `box` hold these sections, in this order. A section whose markup did not change keeps its node, so a
 * text selection, the focus and what assistive technology already read stay in it; a section that changed
 * is replaced by its new node. Sections are matched in order. Returns true when anything changed.
 */
export function syncSections(box: HTMLElement, ...nodes: (HTMLElement | null)[]): boolean {
  const existing = [...box.children]; const kept = new Set<Element>(); const wanted: Element[] = [];
  let from = 0; let changed = false;
  for (const node of nodes) {
    if (!node) continue;
    const next = node.outerHTML; let found = from;
    while (found < existing.length && markup.get(existing[found]!) !== next) found++;
    const current = existing[found];
    if (current) { kept.add(current); wanted.push(current); from = found + 1; }
    else { markup.set(node, next); wanted.push(node); changed = true; }
  }
  for (const child of existing) if (!kept.has(child)) { child.remove(); changed = true; }
  let cursor = box.firstElementChild;
  for (const node of wanted) { if (node === cursor) cursor = cursor.nextElementSibling; else box.insertBefore(node, cursor); }
  return changed;
}

/** Writes a text only when it differs, so an unchanged line is neither mutated nor announced again. */
export function setText(node: HTMLElement, text: string): void { if (node.textContent !== text) node.textContent = text; }

/** How long a live region stays empty before it is given again the sentence it already held. */
export const SAME_SENTENCE_MS = 100;
const rewrites = new WeakMap<object, ReturnType<typeof setTimeout>>();
/** The two members of a live region this module uses. */
export interface LiveRegion { textContent: string | null; readonly isConnected: boolean }
/**
 * Says a sentence in a live region, for callers that decided it must be said. A different sentence is
 * written at once. Assistive technology may stay silent when a region is given the text it already holds:
 * that sentence is announced by emptying the region, then writing it once after a short pause. Only the
 * latest sentence asked is ever written, and never in a region that left the page meanwhile.
 */
export function announce(region: LiveRegion, text: string): void {
  const pending = rewrites.get(region);
  if (pending !== undefined) { clearTimeout(pending); rewrites.delete(region); }
  if (region.textContent !== text) { region.textContent = text; return; }
  region.textContent = "";
  rewrites.set(region, setTimeout(() => { rewrites.delete(region); if (region.isConnected) region.textContent = text; }, SAME_SENTENCE_MS));
}
