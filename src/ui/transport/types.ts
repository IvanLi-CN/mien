/**
 * 领域传输抽象（ADR-0008 / DESIGN.md §11.1）：页面与领域服务只经由这里调用核心，
 * 不直接 `fetch` 或 `invoke`；桌面与服务端只在适配器层分化。
 *
 * 骨架：类型为占位，DTO 与 `CommandName` 由 T8 从 Rust `serde`/`ts-rs` 生成后替换。
 */

import { createHttpTransport } from "./http-transport";
import { createTauriTransport } from "./tauri-transport";

export type DomainEvent = {
  eventId: string;
  sequence: number;
  kind: string;
  payload: unknown;
};

/** 命令名固定为 `<domain>_<action>`。 */
export type CommandName = `${string}_${string}`;

export interface ControlTransport {
  command<T>(name: CommandName, args?: unknown): Promise<T>;
  /** 订阅统一事件频道，返回退订函数。 */
  subscribe(handler: (event: DomainEvent) => void): Promise<() => void>;
}

/** Tauri IPC 专有前缀；其余走 HTTP。 */
export const EVENT_CHANNEL = "mien://event";

/** 是否运行在 Tauri WebView 中。 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

let current: ControlTransport | undefined;

/** 进程级单例；首个调用时按宿主选择适配器。 */
export function transport(): ControlTransport {
  current ??= isTauri() ? createTauriTransport() : createHttpTransport("/api/v1");
  return current;
}
