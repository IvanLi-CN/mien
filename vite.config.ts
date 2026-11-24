import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri 端固定 5173（`tauri.conf.json` 的 devUrl）；端口归属由仓库约定而非自动递增。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: { port: Number(process.env.MIEN_VITE_PORT ?? 5173), strictPort: true },
  build: { outDir: "dist", target: "safari17", sourcemap: true },
});
