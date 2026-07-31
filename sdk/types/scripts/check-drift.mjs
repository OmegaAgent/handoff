#!/usr/bin/env node
/**
 * check-drift.mjs — keep the hand-written `index.d.ts` honest against
 * `spec/openapi.yaml`.
 *
 * `index.d.ts` is hand-maintained: no generator is vendored in this repository
 * and this package installs nothing. So the guarantee that the declarations
 * still cover the contract has to come from somewhere else. This script is
 * that somewhere.
 *
 * What it does:
 *   1. Scans `spec/openapi.yaml` with an indentation-aware regex — no YAML
 *      parser, no dependencies — to enumerate the schema names declared under
 *      `components: -> schemas:`.
 *   2. Scans `index.d.ts` for exported types inside the delimited region
 *      `// #region openapi:components/schemas` … `// #endregion …`.
 *   3. Asserts the two sets are equal, and exits non-zero with a readable list
 *      of missing / extra names otherwise.
 *
 * Paths are resolved relative to THIS FILE, so it runs from any cwd.
 *
 * Usage: node scripts/check-drift.mjs
 */

import { readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PKG_ROOT = resolve(HERE, "..");
const REPO_ROOT = resolve(PKG_ROOT, "..", "..");
const SPEC_PATH = join(REPO_ROOT, "spec", "openapi.yaml");
const DTS_PATH = join(PKG_ROOT, "index.d.ts");

const REGION_START = "// #region openapi:components/schemas";
const REGION_END = "// #endregion openapi:components/schemas";

/** Number of leading spaces on a line. Tabs are not legal YAML indentation. */
function indentOf(line) {
  const m = /^( *)/.exec(line);
  return m[1].length;
}

function isBlankOrComment(line) {
  const t = line.trim();
  return t === "" || t.startsWith("#");
}

/**
 * Enumerate `components/schemas` keys by indentation, without a YAML parser.
 *
 * Structure being walked:
 *
 *     components:          <- indent 0
 *       schemas:           <- indent 2
 *         RaiseRequest:    <- indent 4, this is what we collect
 *           type: object   <- indent 6+, ignored
 */
function readSchemaNames(yamlText) {
  const lines = yamlText.split("\n");
  const names = [];

  let inComponents = false;
  let inSchemas = false;
  let schemaIndent = null;

  for (const line of lines) {
    if (isBlankOrComment(line)) continue;
    const indent = indentOf(line);

    if (indent === 0) {
      // A new top-level key ends any block we were inside.
      inComponents = /^components:\s*$/.test(line);
      inSchemas = false;
      schemaIndent = null;
      continue;
    }

    if (!inComponents) continue;

    if (inSchemas && schemaIndent !== null && indent < schemaIndent) {
      // Dedented out of the schemas block (e.g. back to another
      // `components:` child such as `parameters:`).
      inSchemas = false;
      schemaIndent = null;
    }

    if (!inSchemas) {
      const m = /^( +)schemas:\s*$/.exec(line);
      if (m) {
        inSchemas = true;
        schemaIndent = null; // learned from the first child key
      }
      continue;
    }

    // Inside `schemas:`. The first child key we see fixes the schema indent.
    const keyMatch = /^( +)([A-Za-z_][A-Za-z0-9_]*):\s*$/.exec(line);
    if (!keyMatch) continue;
    const keyIndent = keyMatch[1].length;
    if (schemaIndent === null) schemaIndent = keyIndent;
    if (keyIndent === schemaIndent) names.push(keyMatch[2]);
  }

  return names;
}

/** Exported `type` / `interface` names inside the mirrored region. */
function readExportedRegionTypes(dtsText) {
  const start = dtsText.indexOf(REGION_START);
  const end = dtsText.indexOf(REGION_END);
  if (start === -1 || end === -1 || end < start) {
    throw new Error(
      `index.d.ts is missing the region markers.\n` +
        `  expected: ${REGION_START}\n` +
        `        and: ${REGION_END}`
    );
  }
  const region = dtsText.slice(start, end);
  const names = [];
  const re = /^export\s+(?:declare\s+)?(?:interface|type)\s+([A-Za-z_$][\w$]*)/gm;
  let m;
  while ((m = re.exec(region)) !== null) names.push(m[1]);
  return names;
}

/** Every exported name in the file, for a more useful failure message. */
function readAllExportedTypes(dtsText) {
  const names = new Set();
  const re = /^export\s+(?:declare\s+)?(?:interface|type)\s+([A-Za-z_$][\w$]*)/gm;
  let m;
  while ((m = re.exec(dtsText)) !== null) names.add(m[1]);
  return names;
}

function duplicates(list) {
  const seen = new Set();
  const dupes = new Set();
  for (const n of list) {
    if (seen.has(n)) dupes.add(n);
    seen.add(n);
  }
  return [...dupes];
}

function main() {
  let yamlText;
  try {
    yamlText = readFileSync(SPEC_PATH, "utf8");
  } catch (err) {
    console.error(`check-drift: cannot read the spec at ${SPEC_PATH}`);
    console.error(`  ${err.message}`);
    return 2;
  }

  let dtsText;
  try {
    dtsText = readFileSync(DTS_PATH, "utf8");
  } catch (err) {
    console.error(`check-drift: cannot read ${DTS_PATH}`);
    console.error(`  ${err.message}`);
    return 2;
  }

  const schemaNames = readSchemaNames(yamlText);
  if (schemaNames.length === 0) {
    console.error(
      `check-drift: found no schemas under components/schemas in ${SPEC_PATH}.`
    );
    console.error(
      "  Either the document moved or the indentation scan needs updating."
    );
    return 2;
  }

  let regionNames;
  try {
    regionNames = readExportedRegionTypes(dtsText);
  } catch (err) {
    console.error(`check-drift: ${err.message}`);
    return 2;
  }

  const allExported = readAllExportedTypes(dtsText);
  const schemaSet = new Set(schemaNames);
  const regionSet = new Set(regionNames);

  const missing = schemaNames.filter((n) => !regionSet.has(n));
  const extra = regionNames.filter((n) => !schemaSet.has(n));
  const dupeSchemas = duplicates(schemaNames);
  const dupeRegion = duplicates(regionNames);

  const specRel = relative(REPO_ROOT, SPEC_PATH);
  const dtsRel = relative(REPO_ROOT, DTS_PATH);

  if (
    missing.length === 0 &&
    extra.length === 0 &&
    dupeSchemas.length === 0 &&
    dupeRegion.length === 0
  ) {
    console.log(
      `check-drift: OK — ${schemaNames.length} schemas in ${specRel} ` +
        `all have an exported type in ${dtsRel}.`
    );
    return 0;
  }

  console.error("check-drift: FAILED — the declarations have drifted.\n");
  console.error(`  spec: ${SPEC_PATH} (${schemaNames.length} schemas)`);
  console.error(`  types: ${DTS_PATH} (${regionNames.length} exported)\n`);

  if (missing.length > 0) {
    console.error(
      `  MISSING — declared in ${specRel}, not exported in the mirrored region (${missing.length}):`
    );
    for (const n of missing) {
      const hint = allExported.has(n)
        ? "  (exported, but outside // #region openapi:components/schemas)"
        : "";
      console.error(`    - ${n}${hint}`);
    }
    console.error("");
  }

  if (extra.length > 0) {
    console.error(
      `  EXTRA — exported in the mirrored region, no such schema in ${specRel} (${extra.length}):`
    );
    for (const n of extra) console.error(`    - ${n}`);
    console.error(
      "    Move helper types outside the region, or fix the name to match the spec.\n"
    );
  }

  if (dupeSchemas.length > 0) {
    console.error(`  DUPLICATE schema names in ${specRel}:`);
    for (const n of dupeSchemas) console.error(`    - ${n}`);
    console.error("");
  }

  if (dupeRegion.length > 0) {
    console.error(`  DUPLICATE exported names in the mirrored region:`);
    for (const n of dupeRegion) console.error(`    - ${n}`);
    console.error("");
  }

  console.error(
    "  Fix: hand-edit index.d.ts to match the spec, then re-run `npm run check`."
  );
  return 1;
}

process.exit(main());
