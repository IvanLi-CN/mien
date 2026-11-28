#!/usr/bin/env bun
// 契约字段与 Rust 结构体的双向比对。
//
// `automation_api`、`group_id`、`tags`、`proxy_id` 都曾经是"OpenAPI 声明支持、
// UI 有输入框、Rust 结构体没有这个字段"，于是 `serde` 静默丢弃，用户填了等于
// 没填，也没有任何报错。契约检查器看的是 OpenAPI 自己，测试看的是 TypeScript，
// 两边都发现不了这一层，所以这里直接比对 YAML 与 Rust 源码。
//
// 解析用文本而不是 AST：Rust 字段声明形状固定（`pub name: Type,`），而引入
// Rust 解析器只为读四个结构体不划算。为免误匹配局部变量，只在 `struct NAME {`
// 到配对的 `}` 之间扫描。
import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { parse } from "yaml";

const ROOT = process.env.CONTRACTS_ROOT ?? process.cwd();
const read = (...parts) => readFileSync(path.join(ROOT, ...parts), "utf8");

/**
 * 契约侧每一组要遵守的字段。
 *
 * `ignore` 记下故意不同的字段：`tags` 在 Rust 侧是 `Vec<String>`，契约里是对象
 * 数组，由视图层转换，字段名本身一致；`group_id`/`tags` 在 `Profile` 里不放在
 * 结构体上而是关系表，`ProfileView` 负责合并，所以检查 `ProfileView`。
 */
export const PAIRS = [
  {
    name: "ProfileInput -> CreateProfileInput",
    schema: ["components", "schemas", "ProfileInput"],
    rust: ["src-tauri", "src", "state.rs"],
    struct: "CreateProfileInput",
    // 没有任何豁免字段：契约声明的每一个都必须真的被接收，否则又会是一次
    // 静默丢弃。`kernel_version` 曾经被豁免，实际后端是支持指定的。
  },
  {
    name: "ProfilePatch -> UpdateProfileInput",
    schema: ["components", "schemas", "ProfilePatch"],
    rust: ["src-tauri", "src", "state.rs"],
    struct: "UpdateProfileInput",
    // `kernel_version` 只能通过 `/migrate-kernel` 迁移，不在 patch 里；
    // 契约的 ProfilePatch 也没有它，因此这里同样不需要豁免。
  },
  {
    name: "Profile -> ProfileView + Profile",
    schema: ["components", "schemas", "Profile"],
    rust: [
      { file: ["src-tauri", "src", "state.rs"], struct: "ProfileView" },
      { file: ["crates", "mien-core", "src", "domain", "profile.rs"], struct: "Profile" },
    ],
    // 响应侧：视图与领域结构合并后必须覆盖契约的全部字段。
    merged: true,
  },
];

/** 读取结构体的字段名。跳过 `#[serde(...)]` 属性行与注释。 */
export function structFields(source, structName) {
  const start = source.indexOf(`pub struct ${structName} {`);
  if (start < 0) throw new Error(`struct ${structName} not found`);
  const open = source.indexOf("{", start);
  let depth = 0;
  let end = -1;
  for (let index = open; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") depth += 1;
    else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        end = index;
        break;
      }
    }
  }
  if (end < 0) throw new Error(`struct ${structName} is not closed`);
  const body = source.slice(open + 1, end);
  const fields = [];
  // `#[serde(flatten)]` 的字段把对端结构摊平进同一层 JSON，它自己的名字不出现
  // 在契约里，所以记为"已摊平"，由调用方决定展开哪个结构而不是当成多余字段。
  let flattenNext = false;
  for (const rawLine of body.split("\n")) {
    const line = rawLine.trim();
    if (line === "") continue;
    if (line.startsWith("//")) continue;
    if (line.startsWith("#[")) {
      if (/serde\([^)]*flatten/.test(line)) flattenNext = true;
      continue;
    }
    const match = /^pub\s+([a-z_][a-z0-9_]*)\s*:/.exec(line);
    if (match) {
      if (!flattenNext) fields.push(match[1]);
      flattenNext = false;
    }
  }
  return fields;
}

/** 读取 OpenAPI schema 的 property 名。 */
export function schemaFields(document, segments) {
  let node = document;
  for (const segment of segments) {
    node = node?.[segment];
  }
  if (!node || typeof node !== "object") {
    throw new Error(`schema ${segments.join(".")} not found`);
  }
  return Object.keys(node.properties ?? {});
}

/** 把一组名字转成 `snake_case`，Rust 字段用这个形状。 */
export function toSnake(name) {
  return name.replace(/[A-Z]/g, (char) => `_${char.toLowerCase()}`);
}

function compareFields(expected, actual, ignored) {
  const wanted = new Set(expected.filter((field) => !ignored.includes(field)));
  const have = new Set(actual);
  return {
    missing: [...wanted].filter((field) => !have.has(field)),
    unexpected: [...have].filter((field) => !wanted.has(field) && !ignored.includes(field)),
  };
}

function main() {
  const document = parse(read("docs", "contracts", "openapi.yaml"));
  const errors = [];
  let checked = 0;

  for (const pair of PAIRS) {
    const contractFields = schemaFields(document, pair.schema);
    let implementation;
    if (pair.merged) {
      implementation = pair.rust.flatMap(({ file, struct }) => structFields(read(...file), struct));
    } else {
      implementation = structFields(read(...pair.rust), pair.struct);
    }
    const ignored = pair.optional ?? [];
    const { missing, unexpected } = compareFields(
      contractFields.map(toSnake),
      implementation,
      ignored,
    );
    if (missing.length > 0) {
      errors.push(
        `${pair.name}: the contract declares ${missing.join(", ")} but the struct has no such field; serde silently drops the value the UI sends`,
      );
    }
    if (unexpected.length > 0) {
      errors.push(
        `${pair.name}: the struct has ${unexpected.join(", ")} which the contract does not declare`,
      );
    }
    if (missing.length === 0 && unexpected.length === 0) {
      console.log(`PASS ${pair.name} (${contractFields.length} fields)`);
    }
    checked += 1;
  }

  for (const error of errors) console.error(`FAIL ${error}`);
  console.log(`field-parity: ${checked} pairs checked, ${errors.length} failure(s)`);
  return errors.length === 0 ? 0 : 1;
}

if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  process.exit(main());
}
