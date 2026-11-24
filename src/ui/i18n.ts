/**
 * i18next 单例：资源内联，因此同步初始化（`initImmediate: false`、不启用 Suspense），
 * 组件首帧即可取到文案。
 */
import i18n from "i18next";
import { initReactI18next } from "react-i18next";

/** 命名空间按域拆分；骨架只有 `common`，文案随后续 surface 迁入 `locales/`。 */
export const namespaces = ["common"] as const;

/** 中英双语基准文案。 */
export const resources = {
  "zh-CN": {
    common: {
      appName: "Mien",
      nav: {
        profiles: "档案",
        proxies: "代理",
        kernels: "内核",
        activity: "活动",
        settings: "设置",
      },
      pages: {
        profiles: "档案",
        profile_new: "创建档案",
        profile_detail: "档案详情",
        profile_edit: "编辑档案",
        proxies: "代理库",
        kernels: "内核与引擎",
        activity: "活动与审计",
        settings: "设置",
        not_found: "页面不存在",
        back: "回到档案",
      },
      hints: {
        profiles: "列表、Space 工作区与运行控制后续接入。",
        profile_new: "预设、指纹、代理与运行模式表单在纵向切片中实现。",
        profile_detail: "详情、运行时、日志与限制提示后续接入。",
        profile_edit: "指纹、代理、启动地址与导出后续接入。",
        proxies: "代理创建、检查与分享链接导入在 T9-C 接入。",
        kernels: "能力清单与 active 切换依赖 T4 spike 结论。",
        activity: "事件流与审计查询在事件契约冻结后接入。",
        settings: "本地设置；服务端 Token 与查看器配置随控制面落地。",
        skeleton: "骨架占位",
      },
    },
  },
  en: {
    common: {
      appName: "Mien",
      nav: {
        profiles: "Profiles",
        proxies: "Proxies",
        kernels: "Kernels",
        activity: "Activity",
        settings: "Settings",
      },
      pages: {
        profiles: "Profiles",
        profile_new: "New profile",
        profile_detail: "Profile",
        profile_edit: "Edit profile",
        proxies: "Proxies",
        kernels: "Kernels and engines",
        activity: "Activity and audit",
        settings: "Settings",
        not_found: "Page not found",
        back: "Back to profiles",
      },
      hints: {
        profiles: "List, Space workspace and runtime control land later.",
        profile_new: "Preset, fingerprint, proxy and run-mode forms land in the vertical slice.",
        profile_detail: "Details, runtime, logs and limit hints land later.",
        profile_edit: "Fingerprint, proxy, start URL and export land later.",
        proxies: "Proxy create, check and share-link import land with T9-C.",
        kernels: "Capability lists and activation wait on the T4 spike.",
        activity: "Event stream and audit queries wait on the event contract freeze.",
        settings: "Local settings; server tokens and viewer config follow the control plane.",
        skeleton: "Skeleton placeholder",
      },
    },
  },
};

void i18n.use(initReactI18next).init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  supportedLngs: ["zh-CN", "en"],
  ns: namespaces,
  defaultNS: "common",
  initImmediate: false,
  react: { useSuspense: false },
  interpolation: { escapeValue: false },
  resources,
});

export default i18n;
