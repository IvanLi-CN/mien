import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./i18n";
import "./styles.css";

import { RouterProvider } from "react-router";
import { AppProviders } from "./providers/app";
import { routes } from "./routes";

const container = document.getElementById("root");
if (!container) {
  throw new Error("缺少 #root 挂载点");
}

createRoot(container).render(
  <StrictMode>
    <AppProviders>
      <RouterProvider router={routes} />
    </AppProviders>
  </StrictMode>,
);
