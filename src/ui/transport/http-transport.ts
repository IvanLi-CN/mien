import { type ControlTransport, EVENT_CHANNEL } from "./types";

/**
 * 服务端适配器：同源 `fetch` + CSRF；事件走 `/events` WebSocket。
 *
 * 骨架：命令以 POST 调用、CSRF 与票据握手由 T9-E 按 `docs/specs/control-api/SPEC.md` 补齐。
 */
export function createHttpTransport(baseUrl: string): ControlTransport {
  return {
    command: async <T>(name: string, args?: unknown) => {
      // 写入型命令上线前，这里先按 `<domain>_<action>` 映射到临时路径。
      const response = await fetch(`${baseUrl}/_ipc/${name}`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(args ?? {}),
      });
      if (!response.ok) {
        throw new Error(`${name} 失败：${response.status}`);
      }
      return (await response.json()) as T;
    },
    subscribe: async (handler) => {
      const socket = new WebSocket(
        `${location.origin.replace(/^http/, "ws")}${EVENT_CHANNEL.slice(1)}`,
      );
      socket.addEventListener("message", (message) => handler(JSON.parse(message.data as string)));
      return () => socket.close();
    },
  };
}
