import type { ReactNode } from "react";
import { QueryProvider } from "../providers/query";
import { TransportProvider } from "../transport/context";
import { transport } from "../transport/types";

/** 全局装配：传输、查询缓存、i18n 由入口初始化。 */
export function AppProviders({ children }: { children: ReactNode }) {
  return (
    <QueryProvider>
      <TransportProvider value={transport()}>{children}</TransportProvider>
    </QueryProvider>
  );
}
