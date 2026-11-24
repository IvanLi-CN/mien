import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type ControlTransport, type DomainEvent, EVENT_CHANNEL } from "./types";

/** 桌面适配器：进程内命令调用 + 统一事件频道（ADR-0008）。 */
export function createTauriTransport(): ControlTransport {
  return {
    command: async <T>(name: string, args?: unknown) => {
      // invoke 的第二个参数是命令实参对象，缺省为空对象。
      return invoke<T>(name, (args ?? {}) as Record<string, unknown>);
    },
    subscribe: async (handler: (event: DomainEvent) => void) => {
      const unlisten = await listen<DomainEvent>(EVENT_CHANNEL, (event) => handler(event.payload));
      return unlisten;
    },
  };
}
