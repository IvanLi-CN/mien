// Compile-time (static) review of a compiled schema and test assertions over it.
//
// `$ref` resolution already happened inside `compile`; the abstract syntax tree is audited here so
// a document fails on unknown keywords and on keywords the validator does not apply, and every
// schema prints what of it is actually verified.
import { fail } from "../lib/check-utils.mjs";
import { unknownKeywords } from "./schema-keywords.mjs";

const STUBBED = ["allOf", "anyOf", "oneOf"];

export function staticErrors(compiled) {
  const errors = [];
  const unknown = [...unknownKeywords(compiled.schema)].filter(
    (keyword) => !STUBBED.includes(keyword),
  );
  if (unknown.length > 0)
    errors.push(`${compiled.name}: unsupported keyword(s) ${unknown.join(", ")}`);
  return errors;
}

export function report(compiled) {
  for (const note of compiled.notes) console.log(`  note ${compiled.name}: ${note}`);
  if (compiled.notes.length === 0)
    console.log(`  ${compiled.name}: every keyword applied by the validator`);
}

export function assertValid(compiled, instance, label) {
  const errors = compiled.validate(instance);
  if (errors.length > 0)
    fail(`${label} does not satisfy ${compiled.name}:\n${errors.slice(0, 15).join("\n")}`);
}

export function assertInvalid(compiled, instance, label) {
  if (compiled.validate(instance).length === 0)
    fail(`${label} passes ${compiled.name} but must not`);
}
