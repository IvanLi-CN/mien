import type { components } from "../../../../docs/contracts/generated/openapi";

export type Profile = components["schemas"]["Profile"];
export type ProfileInput = components["schemas"]["ProfileInput"];
export type ProfilePatch = components["schemas"]["ProfilePatch"];
export type RuntimeState = components["schemas"]["RuntimeState"];
export type RuntimeMode = components["schemas"]["RuntimeMode"];
export type Isolation = components["schemas"]["Isolation"];

export const runtimeStateLabels: Record<RuntimeState, string> = {
  starting: "启动中",
  running: "运行中",
  stopping: "停止中",
  stopped: "已停止",
  crashed: "已崩溃",
  failed: "启动失败",
};

export const runtimeStateClasses: Record<RuntimeState, string> = {
  starting: "profile-status profile-status--starting",
  running: "profile-status profile-status--running",
  stopping: "profile-status profile-status--stopping",
  stopped: "profile-status profile-status--stopped",
  crashed: "profile-status profile-status--crashed",
  failed: "profile-status profile-status--failed",
};

export const modeLabels: Record<RuntimeMode, string> = {
  headful: "有头",
  headless: "无头",
  pseudo_headful: "伪有头",
};

export const isolationLabels: Record<Isolation, string> = {
  enforced: "沙箱已生效",
  unsandboxed: "无沙箱",
  unknown: "未确认",
};

export const defaultProfileInput: ProfileInput = {
  name: "",
  group_id: null,
  tags: [],
  notes: null,
  fingerprint: {
    schema_version: 1,
    platform: "macos",
    platform_version: "15",
    brand: "Chrome",
    brand_version: "148",
    hardware_concurrency: 8,
    languages: ["zh-CN", "en-US"],
    timezone: "Asia/Shanghai",
    window: { width: 1440, height: 900 },
    spoof: { font: true, audio: true, canvas: true, clientrects: true, gpu: true },
    webrtc: { disable_non_proxied_udp: true },
    network: { doh_mode: "disabled", ipv6_mode: "block" },
  },
  proxy_id: null,
  mode: "headful",
  automation_api: false,
  start_url: "",
};

export function profileState(profile: Profile): RuntimeState {
  return profile.runtime?.state ?? "stopped";
}

export function proofTone(value: "verified" | "warning" | "failed" | "neutral" | "unknown") {
  return `proof-segment proof-segment--${value}`;
}
