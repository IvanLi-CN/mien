import { defineConfig } from "@playwright/test";

// 桌面壳的端到端用例跑在打包后的 Tauri 应用上；当前只登记 Playwright 基线与目录。
export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  reporter: "list",
  use: { trace: "retain-on-failure", baseURL: "http://127.0.0.1:4317" },
  webServer: {
    command: "bun run dev -- --host 127.0.0.1 --port 4317 --strictPort",
    url: "http://127.0.0.1:4317",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
