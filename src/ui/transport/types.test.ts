import { describe, expect, it } from "vitest";
import { isTauri } from "./types";

describe("isTauri", () => {
  it("没有 Tauri 注入时走 HTTP 适配器", () => {
    expect(isTauri()).toBe(false);
  });

  it("存在 __TAURI_INTERNALS__ 时走 IPC 适配器", () => {
    const holder = window as unknown as Record<string, unknown>;
    holder.__TAURI_INTERNALS__ = {};
    expect(isTauri()).toBe(true);
    Reflect.deleteProperty(holder, "__TAURI_INTERNALS__");
  });
});
