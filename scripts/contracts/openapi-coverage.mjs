// Index of every schema reachable from OpenAPI operation responses and bodies.
//
// Traversal is breadth-first: a pointer's hop count is the fewest `$ref` hops from an operation
// response/body schema to it (0 for the body schema itself). A hop counts as `strict` only when
// the referring schema is itself strict; the residual is `lax`. References into a definition's
// `$defs` (i.e. `a.json#/$defs/x` below the definition) stay lax, so a stubbed `oneOf`/`anyOf` can
// never satisfy a strict requirement.
const DISCRIMINATORS = [
  "const",
  "enum",
  "type",
  "required",
  "properties",
  "format",
  "pattern",
  "minimum",
  "oneOf",
  "anyOf",
  "allOf",
];
const METHODS = ["get", "post", "put", "patch", "delete"];

export const escapeToken = (token) => String(token).replace(/~/g, "~0").replace(/\//g, "~1");

function walkSchema(where, node, out) {
  if (!node || typeof node !== "object") return;
  if (Array.isArray(node)) {
    node.forEach((item, index) => {
      walkSchema(`${where}/${index}`, item, out);
    });
    return;
  }
  for (const [key, value] of Object.entries(node))
    walkSchema(`${where}/${escapeToken(key)}`, value, out);
  if (typeof node.$ref === "string") out.refs.push(where);
  else if (DISCRIMINATORS.some((key) => Object.hasOwn(node, key))) out.discriminators.push(where);
}

const cache = new WeakMap();

/** Pointer sets of one schema document: `{ refs, discriminators }` as escaped JSON pointers. */
export function schemaPointers(schema) {
  if (cache.has(schema)) return cache.get(schema);
  const out = { refs: [], discriminators: [] };
  walkSchema("#", schema, out);
  cache.set(schema, out);
  return out;
}

export function resolvePointer(document, pointer) {
  if (pointer === "#" || pointer === "") return document;
  if (!pointer.startsWith("#/")) return undefined;
  let node = document;
  for (const raw of pointer.slice(2).split("/")) {
    const token = raw.replace(/~1/g, "/").replace(/~0/g, "~");
    if (Array.isArray(node)) {
      if (!/^\d+$/.test(token) || Number(token) >= node.length) return undefined;
      node = node[Number(token)];
    } else if (node && typeof node === "object" && Object.hasOwn(node, token)) {
      node = node[token];
    } else {
      return undefined;
    }
  }
  return node;
}

function bodiesOfSchema(into, schema) {
  if (!schema) return;
  const pointers = schemaPointers(schema);
  into.refs.push(...pointers.refs);
  into.discriminators.push(...pointers.discriminators);
}

function collectBodies(into, response) {
  if (response.$ref) into.refs.push(response.$ref);
  for (const body of Object.values(response.content ?? {})) bodiesOfSchema(into, body?.schema);
}

function operationBodies(document, operation) {
  const into = { refs: [], discriminators: [] };
  for (const [status, response] of Object.entries(operation.responses ?? {})) {
    if (!/^[1-5]\d\d$/.test(status)) continue;
    collectBodies(into, response);
    const target = response.$ref ? resolvePointer(document, response.$ref) : undefined;
    if (target) collectBodies(into, target);
  }
  for (const body of Object.values(operation.requestBody?.content ?? {}))
    bodiesOfSchema(into, body?.schema);
  return into;
}

const paramRef = (parameter) => (typeof parameter?.$ref === "string" ? parameter.$ref : "");

/**
 * Builds the index. `level.get(pointer)` is the fewest hops; `strict.has(pointer)` tells whether a
 * discriminating component carried it there; `parts.get(h)` holds the pointers at exactly `h` hops.
 */
export function buildIndex(document) {
  const level = new Map();
  const strict = new Set();
  const operations = [];

  const seed = (pointer) => {
    if (level.has(pointer)) return;
    level.set(pointer, 0);
    strict.add(pointer);
  };

  for (const [pathName, item] of Object.entries(document.paths ?? {})) {
    const shared = (item.parameters ?? []).map(paramRef).filter(Boolean);
    for (const method of METHODS) {
      const operation = item[method];
      if (!operation) continue;
      const bodies = operationBodies(document, operation);
      const origin = {
        label: `${method.toUpperCase()} ${pathName}`,
        operationId: operation.operationId,
        resource: (item.parameters ?? []).some((parameter) =>
          paramRef(parameter).endsWith("/ProfileId"),
        ),
        refs: new Set(shared),
        discriminators: new Set(bodies.discriminators),
        security: operation.security ?? document.security ?? [],
        scope: operation["x-required-scope"] ?? null,
        statuses: Object.keys(operation.responses ?? {}),
      };
      for (const pointer of shared) seed(pointer);
      for (const pointer of bodies.refs) {
        if (!level.has(pointer)) level.set(pointer, 0);
        origin.refs.add(pointer);
      }
      for (const pointer of origin.discriminators) seed(pointer);
      operations.push(origin);
    }
  }

  let frontier = [...level.keys()].filter((pointer) => pointer.startsWith("#/"));
  while (frontier.length > 0) {
    const next = [];
    for (const pointer of frontier) {
      const target = resolvePointer(document, pointer);
      if (!target || typeof target !== "object") continue;
      const found = schemaPointers(target);
      const fromStrict = strict.has(pointer);
      const hop = level.get(pointer) + 1;
      for (const child of [...found.refs, ...found.discriminators]) {
        const known = level.get(child);
        if (known === undefined || known > hop) next.push(child);
        else if (known === hop && fromStrict && !strict.has(child)) next.push(child);
        else continue;
        level.set(child, hop);
        if (fromStrict) strict.add(child);
        else strict.delete(child);
      }
    }
    frontier = next;
  }

  const parts = new Map();
  for (const pointer of level.keys()) {
    const hop = level.get(pointer);
    if (!parts.has(hop)) parts.set(hop, []);
    parts.get(hop).push(pointer);
  }
  return { level, strict, parts, operations };
}

/** Members of an `enum` (or `items.enum`) reached at `pointer`, or null. */
export function enumOf(document, pointer) {
  const target = resolvePointer(document, pointer);
  const values = target?.enum ?? target?.items?.enum;
  return Array.isArray(values) ? values : null;
}

export { METHODS };
