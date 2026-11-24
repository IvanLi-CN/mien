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
