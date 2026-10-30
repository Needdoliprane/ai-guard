export type Child = Node | string | number | null | undefined | false;
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    node.append(
      child instanceof Node ? child : document.createTextNode(String(child)),
    );
  }
  return node;
}
export function text(node: Node, value: unknown) {
  node.textContent =
    value === null || value === undefined ? "Inconnu" : String(value);
}
export function button(
  label: string,
  className: string,
  action: () => void,
  glyph?: string,
): HTMLButtonElement {
  const node = el("button", className, glyph ? icon(glyph) : null, label);
  node.type = "button";
  node.addEventListener("click", action);
  return node;
}
const paths: Record<string, string[]> = {
  cube: [
    "m12 3 8 4.6v8.8L12 21l-8-4.6V7.6z",
    "m4 7.6 8 4.6 8-4.6M12 12.2V21M8 5.3l8 4.6",
  ],
  grid: ["M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z"],
  agents: [
    "M7 8h10a3 3 0 0 1 3 3v7H4v-7a3 3 0 0 1 3-3zM12 4v4M10 4h4M8 12v2M16 12v2M8 18v3M16 18v3M1 12h3M20 12h3",
  ],
  branch: [
    "M6 3v12a4 4 0 0 0 4 4h8M6 9h8a4 4 0 0 0 4-4V3",
    "M4 3h4M16 3h4M16 17l2 2-2 2",
  ],
  terminal: ["m5 7 5 5-5 5M13 17h6"],
  shield: ["m12 3 8 3v5c0 5-4 8-8 10-4-2-8-5-8-10V6z", "m8 12 3 3 5-6"],
  folder: ["M3 7V5h6l2 3h10v12H3z"],
  box: ["m12 3 9 5-9 5-9-5zM3 8v9l9 5 9-5V8M12 13v9"],
  pulse: ["M2 12h5l3-8 4 16 3-8h5"],
  lock: ["M6 10h12v11H6zM8 10V6a4 4 0 0 1 8 0v4"],
  arrow: ["M5 12h14m-5-5 5 5-5 5"],
  chevron: ["m9 5 7 7-7 7"],
  down: ["M12 3v12m-5-5 5 5 5-5M4 17v4h16v-4"],
  search: ["M16 16 21 21", "M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0"],
  sun: [
    "M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8M12 1v3M12 20v3M1 12h3M20 12h3M4 4l2 2M18 18l2 2M4 20l2-2M18 6l2-2",
  ],
  moon: ["M20 15a9 9 0 0 1-11-11A9 9 0 1 0 20 15z"],
  refresh: ["M20 8a9 9 0 1 0 1 7M20 3v6h-6"],
  check: ["m5 12 4 4L19 6"],
  x: ["m6 6 12 12M6 18 18 6"],
  info: ["M12 11v6M12 7h.01", "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0"],
  alert: ["m12 3 10 18H2zM12 9v5M12 17h.01"],
  menu: ["M4 6h16M4 12h16M4 18h16"],
  logout: ["M10 3H4v18h6M9 12h12m-4-4 4 4-4 4"],
  copy: ["M8 8h12v13H8zM16 8V3H3v13h5"],
  clock: ["M12 6v6l4 2", "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0"],
  cpu: [
    "M6 6h12v12H6zM9 9h6v6H9zM9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4",
  ],
};
export function icon(name: string): SVGSVGElement {
  const node = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  node.setAttribute("viewBox", "0 0 24 24");
  node.setAttribute("fill", "none");
  node.setAttribute("stroke", "currentColor");
  node.setAttribute("stroke-width", "1.6");
  node.setAttribute("stroke-linecap", "round");
  node.setAttribute("stroke-linejoin", "round");
  node.setAttribute("aria-hidden", "true");
  node.classList.add("icon");
  for (const value of paths[name] ?? paths.cube!) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", value);
    node.append(path);
  }
  return node;
}
