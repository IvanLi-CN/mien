import { Outlet } from "react-router";
import { Nav } from "./nav";

/** 应用骨架：顶栏 + 内容区。 */
export function AppLayout() {
  return (
    <div className="flex h-full flex-col">
      <header className="flex h-11 items-center gap-4 border-border border-b px-3">
        <span className="font-mono text-sm tracking-tight">Mien</span>
        <Nav />
      </header>
      <main className="min-h-0 flex-1 overflow-auto p-4">
        <Outlet />
      </main>
    </div>
  );
}
