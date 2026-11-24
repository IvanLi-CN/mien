import { createBrowserRouter, Navigate } from "react-router";
import { AppLayout } from "./layout/app-layout";
import { ActivityPage } from "./pages/activity";
import { KernelsPage } from "./pages/kernels";
import { NotFoundPage } from "./pages/not-found";
import { ProfileDetailPage } from "./pages/profile-detail";
import { ProfileNewPage } from "./pages/profile-new";
import { ProfilesPage } from "./pages/profiles";
import { ProxiesPage } from "./pages/proxies";
import { SettingsPage } from "./pages/settings";

/** 路由与 `docs/UI-SURFACES.md` §1 一一对应；首屏固定为 `/profiles`。 */
export const HOME_PATH = "/profiles";

/** 静态 surface 路由枚举，contracts:check 与 UI-SURFACES.md 交叉校验。 */
export const UI_SURFACE_ROUTES = [
  "/profiles",
  "/profiles/new",
  "/profiles/:id",
  "/profiles/:id/edit",
  "/proxies",
  "/kernels",
  "/activity",
  "/settings",
] as const;

/** Each surface must map to at least one OpenAPI path prefix. */
export const UI_SURFACE_API_PREFIXES = {
  "/profiles": "/api/v1/profiles",
  "/profiles/new": "/api/v1/profiles",
  "/profiles/:id": "/api/v1/profiles",
  "/profiles/:id/edit": "/api/v1/profiles",
  "/proxies": "/api/v1/proxies",
  "/kernels": "/api/v1/kernels",
  "/activity": "/api/v1/audit",
  "/settings": "/api/v1/auth",
} as const;

export const routes = createBrowserRouter([
  {
    path: "/",
    element: <AppLayout />,
    children: [
      { index: true, element: <Navigate to={HOME_PATH} replace /> },
      { path: "profiles", element: <ProfilesPage /> },
      { path: "profiles/new", element: <ProfileNewPage /> },
      { path: "profiles/:id", element: <ProfileDetailPage /> },
      { path: "profiles/:id/edit", element: <ProfileDetailPage edit /> },
      { path: "proxies", element: <ProxiesPage /> },
      { path: "kernels", element: <KernelsPage /> },
      { path: "activity", element: <ActivityPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "*", element: <NotFoundPage /> },
    ],
  },
]);
