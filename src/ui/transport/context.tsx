import { createContext, type ReactNode, useContext, useMemo } from "react";
import { type ControlTransport, transport } from "../transport/types";

const TransportContext = createContext<ControlTransport | null>(null);

/** 注入传输适配器；页面断言非空，缺失即装配错误。 */
export function TransportProvider({
  value,
  children,
}: {
  value?: ControlTransport;
  children: ReactNode;
}) {
  const resolved = useMemo(() => value ?? null, [value]);
  return <TransportContext.Provider value={resolved}>{children}</TransportContext.Provider>;
}

/** 读取传输；单例解析走 `transport()`，此处只给同步注入的部分。 */
export function useTransport(): ControlTransport | null {
  return useContext(TransportContext);
}

export { transport };
