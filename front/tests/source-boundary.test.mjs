// Guard: the shipped bundle is built from front/src only. Test doubles and fixtures can never be pulled in,
// and the supervision modules that must stay importable under Node carry no DOM or stylesheet dependency.
import assert from "node:assert/strict";
import { test } from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const source = fileURLToPath(new URL("../src/", import.meta.url));
const files = readdirSync(source, { recursive: true }).filter((name) => /\.(ts|css)$/.test(String(name))).map((name) => join(source, String(name)));
const specifiers = (text) => [...text.matchAll(/(?:^|\n)\s*(?:import|export)\s[^;]*?from\s+["']([^"']+)["']|(?:^|\n)\s*import\s+["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)|@import\s+url\(["']?([^"')]+)["']?\)/g)]
  .map((match) => match[1] ?? match[2] ?? match[3] ?? match[4]);

test("aucun fichier de front/src n’importe hors de front/src", () => {
  assert.ok(files.length >= 15, "the source tree was read");
  for (const file of files) for (const specifier of specifiers(readFileSync(file, "utf8"))) {
    assert.ok(specifier.startsWith("./") || specifier.startsWith("../"), `${relative(source, file)} importe ${specifier} : aucune dépendance d’exécution ni ressource distante`);
    const target = resolve(dirname(file), specifier);
    assert.ok(target.startsWith(source), `${relative(source, file)} importe ${specifier}, hors de front/src`);
    assert.ok(!/fixture|\.test\.|\/tests\//.test(target), `${relative(source, file)} importe un fichier de test`);
  }
});

test("le modèle et l’état de la supervision restent sans DOM ni feuille de style", () => {
  for (const name of ["supervision-model.ts", "supervision-store.ts", "human-model.ts", "human-api.ts", "access-model.ts", "summary-model.ts"]) {
    const text = readFileSync(join(source, name), "utf8");
    assert.ok(specifiers(text).every((specifier) => specifier.endsWith(".ts")), `${name} n’importe que des modules TypeScript`);
    const code = text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|\s)\/\/.*$/gm, "");
    assert.ok(!/\b(?:document|window)\s*[.[]/.test(code), `${name} ne touche ni document ni window`);
  }
});

test("les sources de la supervision n’embarquent aucune donnée d’exemple ni ressource distante", () => {
  for (const file of files.filter((name) => /supervision|journal|human-continuity|overview|summary-model|access-model/.test(relative(source, name)))) {
    const text = readFileSync(file, "utf8");
    assert.ok(!/https?:\/\//.test(text), `${relative(source, file)} ne référence aucune URL`);
    // Identifier-shaped literals would be invented rows: every identifier must come from a control read.
    assert.ok(!/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i.test(text), `${relative(source, file)} ne contient aucun identifiant en dur`);
  }
});

test("aucun fichier de front/src ne décide d’un droit à partir de la liste de capacités", () => {
  // Rights come from the operation catalogue of the context. The capability list may be validated by the parser,
  // compared as a whole to detect a changed context, or displayed; it is never searched for one capability.
  const lookup = /\bcapabilities\s*\??\.\s*(?:includes|some|find|findIndex|findLast|indexOf|lastIndexOf|has)\s*\(/;
  for (const file of files.filter((name) => name.endsWith(".ts"))) {
    const code = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|\s)\/\/.*$/gm, "");
    assert.doesNotMatch(code, lookup, `${relative(source, file)} cherche une capacité pour décider d’un accès`);
  }
});
