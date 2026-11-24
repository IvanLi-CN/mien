// AJV-compatible JSON Schema validator for the keyword subset in `schema-keywords.mjs`, with
// strict compile-time checks: the document must declare draft 2020-12, every keyword must be
// implemented, and every `$ref`, `$defs`/`definitions` entry and `#anchor` must resolve before any
// instance is validated.
//
// `allOf`/`anyOf`/`oneOf` are stubbed to `true`, so a document that relies on them still compiles
// while the keyword never rejects; `if`/`then`/`else`/`not` are never evaluated. Every stubbed or
// unevaluated keyword is reported as an annotation, and `report()` fails whenever a document uses
// one, so a schema cannot silently claim coverage it does not have.
import { walk } from "../lib/check-utils.mjs";

const FORMATS = {
  "date-time": /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/,
  date: /^\d{4}-\d{2}-\d{2}$/,
  time: /^\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?$/,
  uri: /^[A-Za-z][A-Za-z0-9+.-]*:[^\s]*$/,
  "uri-reference": /^[^\s]*$/,
  email: /^[^\s@]+@[^\s@]+\.[^\s@]+$/,
  uuid: /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/,
  ipv4: /^(?:\d{1,3}\.){3}\d{1,3}$/,
  ipv6: /^[0-9a-fA-F:]+$/,
  hostname:
    /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)*$/,
};

export function typeOf(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (typeof value === "number") return Number.isInteger(value) ? "integer" : "number";
  return typeof value;
}

function deepEqual(left, right) {
  if (left === right) return true;
  if (typeof left !== typeof right || typeof left !== "object" || left === null || right === null)
    return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every((key) => deepEqual(left[key], right[key]))
  );
}

const STUBBED = new Set(["allOf", "anyOf", "oneOf"]);
const NEVER_EVALUATED = new Set(["if", "then", "else", "not", "dependentSchemas", "dependencies"]);

/**
 * Compiles a schema document. `name` labels errors; `root` is the resolution base for `#/$defs/...`
 * and `#anchor` pointers, defaulting to the document `$id`. Throws on any unresolvable reference.
 */
export function compile(schema, name, root = schema?.$id, origin = "") {
  if (schema?.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    throw new Error(`${name}: $schema must be https://json-schema.org/draft/2020-12/schema`);
  }
  if (!root) throw new Error(`${name}: schema has no $id`);
  const notes = [];
  const refTargets = new Map();

  const resolve = (pointer) => {
    let node = schema;
    for (const raw of pointer.slice(2).split("/")) {
      const token = raw.replace(/~1/g, "/").replace(/~0/g, "~");
      if (!node || typeof node !== "object" || !Object.hasOwn(node, token)) return undefined;
      node = node[token];
    }
    return node;
  };
  const anchor = (anchorName) => {
    for (const { value } of walk(schema)) {
      if (value?.$anchor === anchorName || value?.$dynamicAnchor === anchorName) return value;
    }
    return undefined;
  };

  for (const { value, pointer } of walk(schema)) {
    if (!value || typeof value !== "object" || Array.isArray(value)) continue;
    for (const [keyword, keywordValue] of Object.entries(value)) {
      if (STUBBED.has(keyword)) notes.push(`${pointer}: ${keyword} stubbed`);
      else if (NEVER_EVALUATED.has(keyword)) notes.push(`${pointer}: ${keyword} not evaluated`);
      else if (keyword === "unevaluatedProperties" || keyword === "unevaluatedItems")
        notes.push(`${pointer}: ${keyword} not evaluated`);
      if (keyword === "format" && !FORMATS[keywordValue])
        throw new Error(`${name}: ${pointer}: unsupported format ${keywordValue}`);
      if (keyword !== "$ref") continue;
      if (typeof keywordValue !== "string")
        throw new Error(`${name}: ${pointer}: $ref must be a string`);
      if (!keywordValue.startsWith("#"))
        throw new Error(`${name}: ${pointer}: external $ref ${keywordValue} is unsupported`);
      const target = keywordValue.startsWith("#/")
        ? resolve(keywordValue)
        : anchor(keywordValue.slice(1));
      if (target === undefined)
        throw new Error(`${name}: ${pointer}: unresolved $ref ${keywordValue}`);
      refTargets.set(keywordValue, target);
    }
  }

  function at(node) {
    const target = node.$ref ? refTargets.get(node.$ref) : undefined;
    const local = { ...node };
    delete local.$ref;
    return target && typeof target === "object" ? { ...target, ...local } : local;
  }

  function validate(value, node = schema, where = "#") {
    if (node === true) return [];
    return check(value, at(node), where);
  }

  function check(value, node, where) {
    const errors = [];
    const type = typeOf(value);
    if (node.type !== undefined) {
      const allowed = Array.isArray(node.type) ? node.type : [node.type];
      if (!allowed.includes(type) && !(type === "integer" && allowed.includes("number"))) {
        return [`${where}: expected ${allowed.join("|")}, got ${type}`];
      }
    }
    if (node.enum !== undefined && !node.enum.some((candidate) => deepEqual(candidate, value))) {
      errors.push(`${where}: ${JSON.stringify(value)} not in enum`);
    }
    if (node.const !== undefined && !deepEqual(node.const, value))
      errors.push(`${where}: must equal ${JSON.stringify(node.const)}`);

    if (type === "string") {
      const length = [...value].length;
      if (node.minLength !== undefined && length < node.minLength)
        errors.push(`${where}: shorter than ${node.minLength}`);
      if (node.maxLength !== undefined && length > node.maxLength)
        errors.push(`${where}: longer than ${node.maxLength}`);
      if (node.pattern !== undefined && !new RegExp(node.pattern, "u").test(value))
        errors.push(`${where}: does not match ${node.pattern}`);
      if (node.format !== undefined && !FORMATS[node.format].test(value))
        errors.push(`${where}: not a ${node.format}`);
    }
    if (type === "number" || type === "integer") {
      if (node.minimum !== undefined && value < node.minimum)
        errors.push(`${where}: below ${node.minimum}`);
      if (node.maximum !== undefined && value > node.maximum)
        errors.push(`${where}: above ${node.maximum}`);
      if (node.exclusiveMinimum !== undefined && value <= node.exclusiveMinimum)
        errors.push(`${where}: not above ${node.exclusiveMinimum}`);
      if (node.exclusiveMaximum !== undefined && value >= node.exclusiveMaximum)
        errors.push(`${where}: not below ${node.exclusiveMaximum}`);
      if (
        node.multipleOf !== undefined &&
        Math.abs(value / node.multipleOf - Math.round(value / node.multipleOf)) > 1e-9
      ) {
        errors.push(`${where}: not a multiple of ${node.multipleOf}`);
      }
    }
    if (type === "array") {
      if (node.minItems !== undefined && value.length < node.minItems)
        errors.push(`${where}: fewer than ${node.minItems} items`);
      if (node.maxItems !== undefined && value.length > node.maxItems)
        errors.push(`${where}: more than ${node.maxItems} items`);
      if (node.uniqueItems === true) {
        for (let index = 0; index < value.length; index += 1) {
          if (value.slice(0, index).some((seen) => deepEqual(seen, value[index])))
            errors.push(`${where}/${index}: duplicate item`);
        }
      }
      const prefix = node.prefixItems ?? (Array.isArray(node.items) ? node.items : []);
      prefix.forEach((itemSchema, index) => {
        if (index < value.length)
          errors.push(...validate(value[index], itemSchema, `${where}/${index}`));
      });
      const rest = Array.isArray(node.items) ? node.additionalItems : node.items;
      if (rest !== undefined) {
        for (let index = prefix.length; index < value.length; index += 1) {
          if (rest === false) errors.push(`${where}/${index}: no additional items allowed`);
          else errors.push(...validate(value[index], rest, `${where}/${index}`));
        }
      }
      if (
        node.contains !== undefined &&
        !value.some(
          (item, index) => validate(item, node.contains, `${where}/${index}`).length === 0,
        )
      ) {
        errors.push(`${where}: no item matches contains`);
      }
    }
    if (type === "object") {
      const keys = Object.keys(value);
      const required = node.required ?? [];
      for (const key of required)
        if (!Object.hasOwn(value, key)) errors.push(`${where}: missing required "${key}"`);
      if (node.minProperties !== undefined && keys.length < node.minProperties)
        errors.push(`${where}: fewer than ${node.minProperties} properties`);
      if (node.maxProperties !== undefined && keys.length > node.maxProperties)
        errors.push(`${where}: more than ${node.maxProperties} properties`);
      const patterns = Object.entries(node.patternProperties ?? {});
      for (const key of keys) {
        let matched = false;
        if (node.properties && Object.hasOwn(node.properties, key)) {
          matched = true;
          errors.push(...validate(value[key], node.properties[key], `${where}/${key}`));
        }
        for (const [pattern, subSchema] of patterns) {
          if (new RegExp(pattern, "u").test(key)) {
            matched = true;
            errors.push(...validate(value[key], subSchema, `${where}/${key}`));
          }
        }
        if (node.propertyNames !== undefined)
          errors.push(...validate(key, node.propertyNames, `${where}/${key}#name`));
        if (!matched && node.additionalProperties !== undefined) {
          if (node.additionalProperties === false)
            errors.push(`${where}: unknown property "${key}"`);
          else errors.push(...validate(value[key], node.additionalProperties, `${where}/${key}`));
        }
        const dependents = node.dependentRequired?.[key];
        for (const dependent of dependents ?? []) {
          if (!Object.hasOwn(value, dependent))
            errors.push(`${where}/${key}: requires "${dependent}"`);
        }
      }
    }
    return errors;
  }

  const defs =
    Object.keys(schema.$defs ?? {}).length + Object.keys(schema.definitions ?? {}).length;
  return { name, origin, defNames: defs, notes, refTargets, validate };
}
