import { test } from "@playwright/test";

// 骨架 smoke：Web 壳在浏览器里可渲染重定向后的首屏。Tauri 桌面链路在 T9-E 起用 `_bun`。
test("根路径落在档案首屏", async ({ page }) => {
  await page.goto("/");
  await page.waitForURL("**/profiles");
});
