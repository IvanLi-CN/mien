#!/usr/bin/env node
// PKG-008: 发布说明必须逐类列出已知限制。
//
// 这个检查器读取 `docs/KNOWN-LIMITATIONS.md`（唯一事实源）与发布说明，
// 要求发布说明覆盖全部六类，且每类至少有一条实质内容。它不检查措辞，
// 只检查"某一类是否被漏掉或写空"，因为漏项才是 PKG-008 要防的事故。
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");

/** TEST-PLAN.md:295 列出的六类；顺序固定，便于报告比对。 */
export const REQUIRED_CATEGORIES = [
  { key: "kernel-defects", title: "内核缺陷", heading: /内核缺陷/ },
  { key: "pseudo-headful", title: "伪有头", heading: /伪有头/ },
  { key: "tuic", title: "TUIC", heading: /TUIC/ },
  { key: "bootstrap-leak", title: "节点 bootstrap 泄漏", heading: /bootstrap/ },
  { key: "cross-system", title: "跨系统限制", heading: /跨系统/ },
  { key: "shared-engine", title: "共享引擎故障域", heading: /共享引擎/ },
];

/**
 * 至少要有这么多非空白字符，避免"占位一句话"通过门禁。20 个中文字符约
 * 等于一句完整陈述；真实条目的正文远超这个下限，它只拦空节与纯占位符。
 */
const MIN_BODY_CHARS = 20;

/**
 * 把 Markdown 切成 `##`/`###` 章节。发布说明把六类放在二级章节下的三级
 * 标题里，事实源则直接用二级标题，两种深度都要认。
 */
export function splitSections(markdown) {
  const sections = [];
  let current = null;
  for (const line of markdown.split("\n")) {
    const heading = /^(#{2,3})\s+(.*)$/.exec(line);
    if (heading) {
      // 三级标题只在它所属的二级章节内开新节；同级或更高级标题才关闭
      // 上一节。否则 `##` 正文里夹一个 `###` 会把双方都切空。
      const depth = heading[1].length;
      if (current && depth > current.depth) {
        current = { title: heading[2].trim(), depth, body: [], parent: current };
        sections.push(current);
        continue;
      }
      current = { title: heading[2].trim(), depth, body: [], parent: null };
      sections.push(current);
      continue;
    }
    if (current) current.body.push(line);
  }
  return sections.map((section) => ({
    title: section.title,
    depth: section.depth,
    body: section.body.join("\n").trim(),
  }));
}

/** 去掉标题里的编号前缀，得到可比较的类别名。 */
export function normalizeTitle(title) {
  return title.replace(/^\d+(\.\d+)*[.、)]?\s*/, "").trim();
}

/**
 * 检查一份文档是否覆盖全部六类。
 * 返回 `{ missing, empty, ok }`；`missing` 是整类缺失，`empty` 是写了标题但没有实质正文。
 */
export function auditCoverage(markdown) {
  const sections = splitSections(markdown);
  const missing = [];
  const empty = [];
  for (const category of REQUIRED_CATEGORIES) {
    const match = sections.find((section) => category.heading.test(normalizeTitle(section.title)));
    if (!match) {
      missing.push(category.key);
      continue;
    }
    // 统计时忽略纯标题行与表格分隔线，避免用空表格骗过检查。
    // 去掉表格分隔线与纯空白后按非空白字符计数，避免用空格或空表骗过检查。
    const substantive = match.body
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "" && !/^\|?[\s|:-]+\|?$/.test(line))
      .join("")
      .replace(/\s+/g, "");
    if (substantive.length < MIN_BODY_CHARS) empty.push(category.key);
  }
  return { missing, empty, ok: missing.length === 0 && empty.length === 0 };
}

/** 发布说明是否真的引用了事实源；避免发布说明与本文各写一份互相漂移。 */
export function referencesSource(markdown) {
  return /KNOWN-LIMITATIONS\.md/.test(markdown);
}

function main(argv) {
  const mode = argv[0] ?? "check";
  if (mode === "--help" || mode === "-h") {
    console.log(
      "usage: release-notes.mjs [check|coverage] [--notes PATH]\n" +
        "  check     verify the release notes cover all six PKG-008 categories\n" +
        "  coverage  audit docs/KNOWN-LIMITATIONS.md itself\n" +
        "  --notes   release notes path (default docs/RELEASE.md)",
    );
    return 0;
  }

  const notesIndex = argv.indexOf("--notes");
  const notesPath =
    notesIndex >= 0 && argv[notesIndex + 1]
      ? argv[notesIndex + 1]
      : join(ROOT, "docs", "RELEASE.md");

  const target = mode === "coverage" ? join(ROOT, "docs", "KNOWN-LIMITATIONS.md") : notesPath;
  const markdown = readFileSync(target, "utf8");
  const result = auditCoverage(markdown);

  if (result.ok) {
    console.log(`release-notes: PASS (${REQUIRED_CATEGORIES.length} categories) in ${target}`);
    return 0;
  }
  for (const key of result.missing) console.error(`release-notes: category missing: ${key}`);
  for (const key of result.empty) console.error(`release-notes: category has no substance: ${key}`);
  return 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
