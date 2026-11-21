import { lstat, mkdtemp, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";

const formFields = "input, textarea, select, [contenteditable], iframe";
const maxArtifactBytes = 8 * 1024 * 1024;

export async function saveBrowserFailure(page, { directory, secrets = [] }) {
  if (!page || page.isClosed() || !directory) return null;
  try {
    const parent = await lstat(directory);
    if (!parent.isDirectory() || parent.uid !== process.getuid() || (parent.mode & 0o077)) return null;
    const target = await mkdtemp(join(directory, "browser-failure-"));
    const files = [];
    const save = async (name, contents) => {
      if (Buffer.byteLength(contents) > maxArtifactBytes) return;
      await writeFile(join(target, name), contents, { flag: "wx", mode: 0o600 });
      files.push(name);
    };

    const redactions = [...new Set(secrets.filter(Boolean).flatMap((secret) =>
      [secret, encodeURIComponent(secret), JSON.stringify(secret).slice(1, -1)]))];
    // Sanitize the live DOM before rendering and clear form contents from the HTML copy.
    let html;
    let timer;
    try {
      const capture = page.evaluate(({ redactions, formFields }) => {
        const redact = (value) => redactions.reduce(
          (text, secret) => text.split(secret).join("[REDACTED]"), value);
        const walk = (root) => {
          const textNodes = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
          for (let node = textNodes.nextNode(); node; node = textNodes.nextNode())
            node.data = redact(node.data);
          for (const element of [root, ...root.querySelectorAll("*")]) {
            for (const attr of [...element.attributes]) {
              // Dropping a credential-bearing attribute avoids rewriting a resource URL.
              if (redact(attr.value) !== attr.value) element.removeAttributeNode(attr);
            }
          }
        };
        walk(document.documentElement);
        const snapshot = document.documentElement.cloneNode(true);
        for (const node of snapshot.querySelectorAll("script, iframe, base, meta[http-equiv]")) node.remove();
        for (const field of snapshot.querySelectorAll(formFields)) {
          field.removeAttribute("value");
          if (field.matches("textarea, [contenteditable]")) field.textContent = "[REDACTED]";
        }
        const csp = document.createElement("meta");
        csp.httpEquiv = "Content-Security-Policy";
        csp.content = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; form-action 'none'";
        snapshot.querySelector("head").prepend(csp);
        return "<!doctype html>\n" + redact(snapshot.outerHTML);
      }, { redactions, formFields });
      const deadline = new Promise((resolve) => { timer = setTimeout(resolve, 3000, null); });
      html = await Promise.race([capture, deadline]);
    } catch {
      return null;
    } finally {
      clearTimeout(timer);
    }
    if (html === null) return null;
    try { await save("page.html", html); } catch { /* Keep any screenshot that can still be captured. */ }
    try {
      await save("page.png", await page.screenshot({
        timeout: 3000, animations: "disabled", mask: [page.locator(formFields)],
      }));
    } catch { /* The HTML can still diagnose a page that failed to render. */ }
    return files.length ? { directory: basename(target), files } : null;
  } catch {
    return null;
  }
}
