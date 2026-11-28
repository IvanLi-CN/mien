// JSON Schema 2020-12 keywords implemented by the contract checks, and where a document uses
// keywords that are not implemented. `allOf`/`anyOf`/`oneOf` are stubbed to `true` so those
// documents go through, while their application is stubbed too, as listed in the annotation.
export const STUBBED = ["allOf", "anyOf", "oneOf"];
export const STUBBED_APPLIED = false;
const KEYWORDS = new Set([
  "$schema",
  "$id",
  "$ref",
  "$defs",
  "definitions",
  "$comment",
  "$anchor",
  "$dynamicRef",
  "title",
  "description",
  "examples",
  "default",
  "x-error-mappings",
  "deprecated",
  "readOnly",
  "writeOnly",
  "type",
  "enum",
  "const",
  "required",
  "properties",
  "patternProperties",
  "additionalProperties",
  "unevaluatedProperties",
  "propertyNames",
  "minProperties",
  "maxProperties",
  "dependentRequired",
  "dependentSchemas",
  "dependencies",
  "items",
  "prefixItems",
  "additionalItems",
  "contains",
  "unevaluatedItems",
  "minItems",
  "maxItems",
  "uniqueItems",
  "minLength",
  "maxLength",
  "pattern",
  "format",
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "exclusiveMaximum",
  "multipleOf",
  "if",
  "then",
  "else",
  "not",
  ...STUBBED,
]);

export function unknownKeywords(schema, found = new Set()) {
  if (Array.isArray(schema)) {
    for (const item of schema) unknownKeywords(item, found);
  } else if (schema && typeof schema === "object") {
    for (const [key, value] of Object.entries(schema)) {
      if (!KEYWORDS.has(key)) found.add(key);
      else if (key !== "enum" && key !== "const" && key !== "examples" && key !== "default")
        unknownKeywords(value, found);
    }
  }
  return found;
}
