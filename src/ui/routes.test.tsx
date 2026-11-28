import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { describe, expect, it } from "vitest";
import { AppLayout } from "./layout/app-layout";
import { routes } from "./routes";

describe("routes", () => {
  it("档案首屏挂在 /profiles", async () => {
    const router = createMemoryRouter(routes.routes, { initialEntries: ["/profiles"] });
    render(<RouterProvider router={router} />);
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent("档案");
  });

  it("根路由重定向到首屏常量路径", () => {
    const index = routes.routes[0]?.children?.find((route) => route.index);
    expect(index?.element).toBeTruthy();
  });

  it("渲染编辑路由的标题", async () => {
    const router = createMemoryRouter(routes.routes, { initialEntries: ["/profiles/01X/edit"] });
    render(<RouterProvider router={router} />);
    expect(await screen.findByRole("heading", { name: "编辑档案" })).toBeInTheDocument();
  });

  it("导航包含全部一级入口", async () => {
    const router = createMemoryRouter(routes.routes, { initialEntries: ["/proxies"] });
    render(<RouterProvider router={router} />);
    const nav = await screen.findByRole("navigation", { name: "Mien" });
    expect(nav.querySelectorAll("a")).toHaveLength(5);
  });

  it("未知路径给出 404", async () => {
    const router = createMemoryRouter(routes.routes, { initialEntries: ["/nope"] });
    render(<RouterProvider router={router} />);
    expect(await screen.findByRole("heading", { name: "页面不存在" })).toBeInTheDocument();
  });
});

describe("AppLayout", () => {
  it("顶栏固定品牌名", () => {
    const router = createMemoryRouter([{ path: "/", element: <AppLayout /> }], {
      initialEntries: ["/"],
    });
    render(<RouterProvider router={router} />);
    expect(screen.getByText("Mien")).toBeInTheDocument();
  });
});
