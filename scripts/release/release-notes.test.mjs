// PKG-008: 发布说明必须逐类列出已知限制。
//
// 这些用例锁定两件事：六类齐全才算通过，以及漏项/空节必须被拒绝。第三条
// 用来防止检查器退化成"总是通过"。

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  auditCoverage,
  normalizeTitle,
  REQUIRED_CATEGORIES,
  referencesSource,
  splitSections,
} from "./release-notes.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");

/** 构造一份覆盖全部六类的文档，每类正文超过下限。 */
function completeDoc() {
  return REQUIRED_CATEGORIES.map(
    (category, index) =>
      `## ${index + 1}. ${category.title}\n\n这一类的说明文字足够长，不会被当成空节忽略掉。\n`,
  ).join("\n");
}

test("所需类别与 TEST-PLAN.md:295 的六类一致", () => {
  assert.deepEqual(
    REQUIRED_CATEGORIES.map((category) => category.title),
    ["内核缺陷", "伪有头", "TUIC", "节点 bootstrap 泄漏", "跨系统限制", "共享引擎故障域"],
  );
});

test("六类齐全的文档通过", () => {
  const result = auditCoverage(completeDoc());
  assert.equal(result.ok, true);
  assert.deepEqual(result.missing, []);
  assert.deepEqual(result.empty, []);
});

test("漏掉一类会被指名", () => {
  const markdown = completeDoc()
    .split("\n## ")
    .filter((section) => !section.includes("TUIC"))
    .join("\n## ");
  const result = auditCoverage(markdown);
  assert.equal(result.ok, false);
  assert.deepEqual(result.missing, ["tuic"]);
});

test("只有标题的空节不算覆盖", () => {
  const markdown = completeDoc().replace(
    /## 3\. TUIC\n\n这一类的说明文字足够长，不会被当成空节忽略掉。\n/,
    "## 3. TUIC\n\n",
  );
  const result = auditCoverage(markdown);
  assert.equal(result.ok, false);
  assert.deepEqual(result.empty, ["tuic"]);
});

test("表格分隔线与空白不能充数", () => {
  const markdown = completeDoc().replace(
    /## 3\. TUIC\n\n这一类的说明文字足够长，不会被当成空节忽略掉。\n/,
    "## 3. TUIC\n\n| --- | :---: |\n|  |   |\n",
  );
  assert.deepEqual(auditCoverage(markdown).empty, ["tuic"]);
});

test("三级标题在二级章节内单独成立", () => {
  const markdown = REQUIRED_CATEGORIES.map((category, index) =>
    index === 0
      ? `## 容器\n\n### ${category.title}\n\n这一类的说明文字足够长，不会被当成空节忽略掉。\n`
      : `### ${category.title}\n\n这一类的说明文字足够长，不会被当成空节忽略掉。\n`,
  ).join("\n");
  const result = auditCoverage(markdown);
  assert.equal(result.ok, true);
});

test("标题编号前缀不影响匹配", () => {
  assert.equal(normalizeTitle("4. 节点 bootstrap 泄漏"), "节点 bootstrap 泄漏");
  assert.equal(normalizeTitle("bootstrap 泄漏"), "bootstrap 泄漏");
  assert.equal(normalizeTitle("7) 跨系统限制"), "跨系统限制");
});

test("发布说明必须引用事实源以避免两处漂移", () => {
  const notes = readFileSync(join(ROOT, "docs", "RELEASE.md"), "utf8");
  assert.equal(referencesSource(notes), true);
});

test("仓库里的两份文档都真的通过检查", () => {
  for (const relative of ["docs/KNOWN-LIMITATIONS.md", "docs/RELEASE.md"]) {
    const markdown = readFileSync(join(ROOT, relative), "utf8");
    const result = auditCoverage(markdown);
    assert.equal(
      result.ok,
      true,
      `${relative} 缺少 ${result.missing.join(",")} 或写空 ${result.empty.join(",")}`,
    );
  }
});

test("切分保留章节顺序与标题深度", () => {
  const sections = splitSections("## 7. 限制\n\n### 内核缺陷\n正文\n\n## 8. 缺口\n");
  assert.deepEqual(
    sections.map((section) => [section.title, section.depth]),
    [
      ["7. 限制", 2],
      ["内核缺陷", 3],
      ["8. 缺口", 2],
    ],
  );
});
