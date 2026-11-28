// 契约字段与 Rust 结构体的比对本身要有测试，否则它退化成恒绿的门禁。
//
// 重点不是"当前仓库通过"（那是 `check.mjs` 的事），而是"当有一天有人漏掉
// 一个字段时它会红"——所以这里用合成的源码验证三种失败形态。

import assert from "node:assert/strict";
import { test } from "node:test";

import { PAIRS, schemaFields, structFields, toSnake } from "./field-parity.mjs";

test("结构体字段解析只取 pub 字段", () => {
  const source = `
pub struct Sample {
    #[serde(default)]
    pub name: String,
    /// 注释
    pub notes: Option<String>,
    internal: u8,
}
`;
  assert.deepEqual(structFields(source, "Sample"), ["name", "notes"]);
});

test("#[serde(flatten)] 的字段不算对外字段", () => {
  const source = `
pub struct Sample {
    #[serde(flatten)]
    pub profile: Profile,
    pub isolation: Isolation,
}
`;
  assert.deepEqual(structFields(source, "Sample"), ["isolation"]);
});

test("嵌套的花括号不会提前结束结构体", () => {
  const source = `
pub struct Sample {
    pub window: Option<Window>,
    pub after: String,
}
`;
  assert.deepEqual(structFields(source, "Sample"), ["window", "after"]);
});

test("找不到结构体时报错而不是返回空列表", () => {
  assert.throws(() => structFields("pub struct Other { pub a: u8 }", "Sample"), /not found/);
});

test("契约属性名转成 Rust 的 snake_case", () => {
  assert.equal(toSnake("group_id"), "group_id");
  assert.equal(toSnake("automationApi"), "automation_api");
  assert.equal(toSnake("proxyId"), "proxy_id");
  assert.equal(toSnake("start_url"), "start_url");
});

test("读取 OpenAPI schema 的字段名", () => {
  const document = {
    components: { schemas: { Sample: { properties: { a: {}, b: {} } } } },
  };
  assert.deepEqual(schemaFields(document, ["components", "schemas", "Sample"]), ["a", "b"]);
});

test("缺失的 schema 报错而不是静默通过", () => {
  assert.throws(
    () => schemaFields({ components: {} }, ["components", "schemas", "X"]),
    /not found/,
  );
});

test("每一组比对都覆盖真实的契约与源码路径", () => {
  // 防止有人把某个 pair 的 optional 列表改成"什么都忽略"，让检查失去意义。
  for (const pair of PAIRS) {
    const ignored = pair.optional ?? [];
    if (pair.merged) {
      // 响应侧不允许任何豁免：字段全都要在。
      assert.equal(ignored.length, 0, `${pair.name} must not ignore fields`);
    } else {
      assert.ok(
        ignored.length <= 1,
        `${pair.name} ignores too many fields (${ignored.join(", ")}); that hides real gaps`,
      );
    }
  }
});

test("已知的四个断链字段都在必查范围内", () => {
  // automation_api / group_id / tags / proxy_id 曾经被静默丢弃，任何一个都
  // 不能出现在 optional 里，否则同一个 bug 可以无声复发。
  const inputPair = PAIRS.find((pair) => pair.name.startsWith("ProfileInput"));
  assert.ok(inputPair, "ProfileInput pair must exist");
  const ignored = inputPair.optional ?? [];
  for (const field of ["automation_api", "group_id", "tags", "proxy_id"]) {
    assert.ok(!ignored.includes(field), `${field} must never be exempt from the check`);
  }
});
