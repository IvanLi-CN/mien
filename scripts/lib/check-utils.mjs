// Primitives shared by the contract checks: Markdown parsing, file digesting, repo-relative
// paths and a minimal YAML+JSON reader for the OpenAPI document.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const MARKDOWN_EXTENSIONS = new Set([".md"]);

export function readUtf8(root, relative) {
  return readFileSync(path.join(root, relative), "utf8");
}

export function readJson(root, relative) {
  return JSON.parse(readUtf8(root, relative));
}

export function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

export function sha256File(root, relative) {
  return sha256(readFileSync(path.join(root, relative)));
}

/** All regular files under `relative`, recursively, as repo-relative POSIX paths, sorted. */
export function listFiles(root, relative = ".") {
  const absolute = path.join(root, relative);
  const out = [];
  for (const entry of readdirSync(absolute, { withFileTypes: true })) {
    const child = path.posix.join(relative, entry.name);
    if (entry.isDirectory()) out.push(...listFiles(root, child));
    else if (entry.isFile()) out.push(child);
  }
  return out.sort();
}

export function listMarkdown(root, relative = ".") {
  return listFiles(root, relative).filter((file) =>
    MARKDOWN_EXTENSIONS.has(path.posix.extname(file)),
  );
}

export function isFile(root, relative) {
  try {
    return statSync(path.join(root, relative)).isFile();
  } catch {
    return false;
  }
}

export function isDirectory(root, relative) {
  try {
    return statSync(path.join(root, relative)).isDirectory();
  } catch {
    return false;
  }
}

export function lineOf(text, offset) {
  let line = 1;
  for (let index = 0; index < offset && index < text.length; index += 1) {
    if (text.charCodeAt(index) === 10) line += 1;
  }
  return line;
}

/** Fenced-code spans of a Markdown text; `code` excludes the fence lines. */
export function fencedBlocks(text) {
  const blocks = [];
  const pattern = /^ {0,3}(`{3,}|~{3,})(.*)$/gm;
  let open = null;
  for (const match of text.matchAll(pattern)) {
    const [, fence, info] = match;
    const bodyStart = match.index + match[0].length + 1;
    if (open === null) {
      open = {
        fence: fence[0],
        length: fence.length,
        lang: info.trim().split(/\s+/)[0] ?? "",
        start: match.index,
        bodyStart,
      };
      continue;
    }
    const closing = new RegExp(`^ {0,3}${open.fence}{${open.length},}\\s*$`);
    if (closing.test(fence + info.trim()) || closing.test(match[0].trimEnd())) {
      blocks.push({
        lang: open.lang,
        start: open.start,
        body: text.slice(open.bodyStart, match.index),
      });
      open = null;
    }
  }
  if (open !== null)
    blocks.push({ lang: open.lang, start: open.start, body: text.slice(open.bodyStart) });
  return blocks;
}

/** Markdown headings outside code fences, as `{ level, text, offset }`. */
export function headings(text) {
  const masked = text.replace(/^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^ {0,3}\1[^\n]*$/gm, (block) =>
    block.replace(/[^\n]/g, " "),
  );
  const found = [];
  const pattern = /^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$/gm;
  for (const match of masked.matchAll(pattern))
    found.push({ level: match[1].length, text: match[2].trim(), offset: match.index });
  return found;
}

export function githubSlug(text) {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N} _-]/gu, "")
    .trim()
    .replace(/ /g, "-");
}

/** Every `[text](target)` / `![alt](target)` / reference definition target in a Markdown text. */
export function links(text) {
  const found = [];
  const pattern = /!?\[[^\]]*\]\(\s*(<[^>]*>|[^\s)]*)(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)/g;
  for (const match of text.matchAll(pattern)) {
    found.push({
      target: match[1].replace(/^<|>$/g, ""),
      offset: match.index + match[0].indexOf(match[1]),
    });
  }
  const definitions = /^ {0,3}\[[^\]]+\]:[ \t]*(<[^>]*>|\S+)/gm;
  for (const match of text.matchAll(definitions)) {
    found.push({
      target: match[1].replace(/^<|>$/g, ""),
      offset: match.index + match[0].indexOf(match[1]),
    });
  }
  return found;
}

/**
 * Minimal YAML reader for the OpenAPI document. Supports block sequences, block mappings with
 * a two-space indent step, `key:`/`- value` scalars, quoted and plain scalars, block scalars
 * (`|`/`>`) and flow sequences of plain scalars (`[a, b]`). Plain scalar typing follows the
 * JSON schema (booleans, null, numbers); `enum` values are re-typed by the caller.
 */
export function parseYaml(source) {
  const lines = source.split("\n").map((raw) => raw.replace(/\t/g, "  "));
  if (lines[0]?.trim() === "---") lines[0] = "";

  const indentOf = (line) => line.length - line.trimStart().length;
  /** Column of the first colon followed by space or end of line outside quotes, or null. */
  const columnOf = (value) => {
    let quote = "";
    for (let index = 0; index < value.length; index += 1) {
      const char = value[index];
      if (quote) {
        if (char === quote) quote = "";
      } else if (char === '"' || char === "'") {
        quote = char;
      } else if (char === ":" && (index === value.length - 1 || value[index + 1] === " ")) {
        return index;
      }
    }
    return null;
  };
  const isBlank = (line) => line.trim() === "" || line.trimStart().startsWith("#");

  function stripComment(value) {
    let quote = "";
    for (let index = 0; index < value.length; index += 1) {
      const char = value[index];
      if (quote) {
        if (char === quote) quote = "";
      } else if (char === '"' || char === "'") {
        quote = char;
      } else if (char === "#" && (index === 0 || value[index - 1] === " ")) {
        return value.slice(0, index).trimEnd();
      }
    }
    return value.trimEnd();
  }

  function scalar(raw) {
    const value = stripComment(raw).trim();
    if (value === "") return null;
    if (value.startsWith('"')) {
      if (!value.endsWith('"') || value.length < 2)
        throw new Error(`unterminated double quote: ${value}`);
      return JSON.parse(value);
    }
    if (value.startsWith("'")) {
      if (!value.endsWith("'") || value.length < 2)
        throw new Error(`unterminated single quote: ${value}`);
      return value.slice(1, -1).replace(/''/g, "'");
    }
    if (value === "[]") return [];
    if (value === "{}") return {};
    if (value === "true") return true;
    if (value === "false") return false;
    if (value === "null" || value === "~") return null;
    if (/^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][-+]?[0-9]+)?$/.test(value)) return Number(value);
    return value;
  }

  /** Offset of the key's separating colon in `content`, or null when the line is not a mapping entry. */
  function mappingKey(content) {
    const bare = stripComment(content);
    const quoted = bare.startsWith('"') || bare.startsWith("'") ? splitQuoted(bare) : null;
    if (quoted !== null && bare[quoted + 1] === ":") return { end: quoted + 1 };
    const column = columnOf(bare);
    return column === null ? null : { end: column };
  }

  /** Index of the closing quote of a quoted scalar at the start of `value`, or null. */
  function splitQuoted(value) {
    const quote = value[0];
    for (let index = 1; index < value.length; index += 1) {
      if (quote === '"' && value[index] === "\\") index += 1;
      else if (value[index] === quote) {
        if (quote === "'" && value[index + 1] === "'") index += 1;
        else return index;
      }
    }
    return null;
  }

  function skipBlank(index) {
    while (index < lines.length && isBlank(lines[index])) index += 1;
    return index;
  }

  function parseBlockScalar(index, parentIndent, folded) {
    const collected = [];
    let cursor = index;
    while (
      cursor < lines.length &&
      (lines[cursor].trim() === "" || indentOf(lines[cursor]) > parentIndent)
    ) {
      collected.push(lines[cursor]);
      cursor += 1;
    }
    const indent = Math.min(...collected.filter((line) => line.trim() !== "").map(indentOf));
    const pieces = collected.map((line) => line.slice(Number.isFinite(indent) ? indent : 0));
    return { value: pieces.join(folded ? " " : "\n").trimEnd(), index: cursor };
  }

  function flowSequence(raw) {
    const inner = stripComment(raw).slice(1, -1).trim();
    return inner === "" ? [] : inner.split(",").map((item) => scalar(item));
  }

  /**
   * Node started by `cursor`. A `- ` marker (when `allowList`) makes a block sequence; a key makes
   * a mapping; anything else is a scalar. Deeper lines never appear here as a node of their own.
   */
  function parseNode(index, indent, allowList) {
    const cursor = skipBlank(index);
    if (cursor >= lines.length) return { value: null, index: cursor };
    const line = lines[cursor];
    if (indentOf(line) < indent) return { value: null, index: cursor };
    const content = line.trimStart();
    if (allowList && content.startsWith("-")) return parseList(cursor, indent);
    if (mappingKey(stripComment(content)) !== null) return parseMapping(cursor, indent);
    if (content.startsWith("[") && stripComment(content).endsWith("]"))
      return { value: flowSequence(content), index: cursor + 1 };
    return { value: scalar(content), index: cursor + 1 };
  }

  /** Sequence items at `indent`: `- ` starts each; the rest of the line is the item at +2. */
  function parseList(index, indent) {
    const items = [];
    let cursor = index;
    while (cursor < lines.length) {
      const line = lines[cursor];
      if (line.trim() === "") {
        cursor += 1;
        continue;
      }
      if (indentOf(line) !== indent || !line.trimStart().startsWith("-")) break;
      if (!line.trimStart().startsWith("- ") && line.trim() !== "-") break;
      const body =
        line.trimStart() === "-"
          ? ""
          : line
              .trimStart()
              .slice(2)
              .replace(/^ {1,2}/, "");
      if (body === "") {
        const nested = parseNode(cursor + 1, indent + 2, true);
        items.push(nested.value);
        cursor = Math.max(nested.index, cursor + 1);
        continue;
      }
      lines[cursor] = `${" ".repeat(indent + 2)}${body}`;
      const nested = parseNode(cursor, indent + 2, true);
      items.push(nested.value);
      cursor = Math.max(nested.index, cursor + 1);
    }
    return { value: items, index: cursor };
  }

  /**
   * One mapping block at `indent`. Entries keep document order. A key with no inline value takes the
   * deeper block that follows; a sequence may also sit at the key's own indent, so it is accepted
   * there. A `- ` line at the mapping's indent is the inside of a sequence item whose marker was
   * stripped: everything parsed here belongs to that single item, so the block yields one item.
   */
  function parseMapping(index, indent) {
    const entries = [];
    /* list item members end at the next dash of the same level or at a line that indents out. */
    let cursor = index;
    const listEnd = (at) => {
      const next = skipBlank(at);
      if (next >= lines.length || indentOf(lines[next]) > indent) return lines.length;
      if (lines[next].trimStart().startsWith("-")) return next;
      return Math.max(next, lines.length);
    };
    while (cursor < lines.length) {
      const line = lines[cursor];
      if (line.trim() === "") {
        cursor += 1;
        continue;
      }
      const content = line.trimStart();
      const end = listEnd(cursor);
      if (indentOf(line) > indent || cursor > end) break;
      const bare = stripComment(content).trimEnd();
      const key = mappingKey(bare);
      if (key === null)
        throw new Error(`line ${cursor + 1}: expected a mapping entry, got ${content}`);
      const name = scalar(bare.slice(0, key.end));
      const rest = bare.slice(key.end + 1).trim();
      if (rest === "" || rest.startsWith("#")) {
        const next = cursor + 1;
        const deeper = next < lines.length && indentOf(lines[next]) > indent;
        const indentedList = next < lines.length && lines[next].trimStart().startsWith("-");
        const nested =
          deeper || indentedList
            ? parseNode(next, deeper ? indentOf(lines[next]) : indent, true)
            : { value: null, index: next };
        entries.push([name, nested.value]);
        cursor = Math.max(nested.index, cursor + 1);
        continue;
      }
      if (/^[|>][-+0-9]*$/.test(rest)) {
        const block = parseBlockScalar(cursor + 1, indent, rest.startsWith(">"));
        entries.push([name, block.value]);
        cursor = block.index;
        continue;
      }
      entries.push([name, rest.startsWith("[") ? flowSequence(rest) : scalar(rest)]);
      cursor += 1;
    }
    return { value: Object.fromEntries(entries), index: cursor };
  }

  const parsed = parseNode(0, 0, true);
  return parsed.value;
}

/** Deep walk yielding `{ parent, key, value, pointer }` for every node of a JSON-like tree. */
export function* walk(root, pointer = "#", parent = null, key = "") {
  yield { parent, key, value: root, pointer };
  if (Array.isArray(root)) {
    for (const [index, item] of root.entries())
      yield* walk(item, `${pointer}/${index}`, root, index);
  } else if (root && typeof root === "object") {
    for (const [childKey, child] of Object.entries(root)) {
      yield* walk(child, `${pointer}/${childKey}`, root, childKey);
    }
  }
}

/** Smallest key of `object` whose string/id starts with `prefix`; `'#'` for the root. */
export function resolvePointer(document, pointer) {
  if (pointer === "#" || pointer === "") return document;
  if (!pointer.startsWith("#/")) throw new Error(`unsupported pointer ${pointer}`);
  let node = document;
  for (const raw of pointer.slice(2).split("/")) {
    const token = raw.replace(/~1/g, "/").replace(/~0/g, "~");
    if (Array.isArray(node)) node = node[Number(token)];
    else if (node && typeof node === "object") node = node[token];
    else return undefined;
  }
  return node;
}

export function fail(lines) {
  const list = Array.isArray(lines) ? lines : [lines];
  throw new Error(list.join("\n"));
}
