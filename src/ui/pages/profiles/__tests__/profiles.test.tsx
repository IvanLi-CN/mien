import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { TransportProvider } from "../../../transport/context";
import type { ControlTransport } from "../../../transport/types";
import { ProfileForm } from "../../profile-edit/profile-form";
import { IsolationProof, ProfileCard, ProfileStatus } from "../profile-card";
import type { Profile } from "../profile-model";

function createTransport(command: ControlTransport["command"]): ControlTransport {
  return { command, subscribe: async () => () => undefined };
}
function createCommandMock(implementation: () => Promise<unknown> = async () => undefined) {
  const mock = vi.fn(async (_name: string, _args?: unknown) => implementation());
  return { mock, command: mock as unknown as ControlTransport["command"] };
}
function renderForm(command: ControlTransport["command"]) {
  return render(
    <MemoryRouter initialEntries={["/profiles/new"]}>
      <TransportProvider value={createTransport(command)}>
        <ProfileForm />
      </TransportProvider>
    </MemoryRouter>,
  );
}

describe("ProfileStatus", () => {
  it.each([
    ["starting", "启动中"],
    ["running", "运行中"],
    ["stopping", "停止中"],
    ["stopped", "已停止"],
    ["crashed", "已崩溃"],
    ["failed", "启动失败"],
  ] as const)("renders %s with text and a status dot", (state, label) => {
    const { container } = render(<ProfileStatus state={state} />);
    expect(screen.getByText(label)).toBeInTheDocument();
    expect(container.querySelector(".profile-status__dot")).toBeInTheDocument();
    expect(container.querySelector(`.profile-status--${state}`)).toBeInTheDocument();
  });
});

describe("Profile proof semantics", () => {
  it("keeps unknown isolation visually distinct from stopped runtime", () => {
    const unknown = render(<IsolationProof isolation="unknown" />);
    const unknownProof = unknown.container.querySelector(".proof-segment");
    expect(unknownProof).toHaveClass("proof-segment--unknown");
    expect(unknownProof).toHaveTextContent("未确认");
    unknown.unmount();
    const stopped = render(<ProfileStatus state="stopped" />);
    const stoppedStatus = stopped.container.querySelector(".profile-status");
    expect(stoppedStatus).toHaveClass("profile-status--stopped");
    expect(stoppedStatus).toHaveTextContent("已停止");
    expect(stoppedStatus).not.toHaveClass("proof-segment--unknown");
  });
});

function profileFixture(overrides: Partial<Profile> = {}): Profile {
  return {
    id: "01JPROFILE0000000000000000",
    name: "Slice",
    group_id: null,
    tags: [],
    notes: null,
    fingerprint: {
      schema_version: 1,
      seed: 42,
      platform: "macos",
      platform_version: "14.5",
      brand: "Edge",
      brand_version: "148.0.0.0",
      hardware_concurrency: 12,
      languages: ["en-US", "en"],
      timezone: "America/Los_Angeles",
      window: { width: 1440, height: 900 },
      webrtc: { disable_non_proxied_udp: true },
      network: { doh_mode: "disabled", ipv6_mode: "block" },
    },
    kernel_version: "148.0.7778.215",
    proxy_id: null,
    mode: "headful",
    status: "active",
    isolation: "unknown",
    direct: true,
    reduced_assurance: true,
    limitations: ["Host-bound screen size remains observable."],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

function renderCard(command: ControlTransport["command"], profile = profileFixture()) {
  return render(
    <MemoryRouter initialEntries={["/profiles"]}>
      <TransportProvider value={createTransport(command)}>
        <ProfileCard profile={profile} />
      </TransportProvider>
    </MemoryRouter>,
  );
}

describe("ProfileCard runtime controls", () => {
  it("calls runtime_start and shows running with the direct-mode disclosure", async () => {
    const calls: string[] = [];
    const command = vi.fn(async (name: string) => {
      calls.push(name);
      if (name === "runtime_status") return { state: "stopped", isolation: "unknown" };
      if (name === "runtime_start")
        return {
          runtimeId: "rt-01JPROFILE0000000000000000",
          state: "running",
          isolation: "enforced",
          reducedAssurance: true,
          disclosures: ["direct mode: no proxy-level DNS or fail-closed guarantee"],
        };
      throw new Error(`unexpected command ${name}`);
    }) as unknown as ControlTransport["command"];

    renderCard(command);
    fireEvent.click(await screen.findByRole("button", { name: "启动" }));

    await waitFor(() => expect(calls).toContain("runtime_start"));
    expect(await screen.findByText("运行中")).toBeInTheDocument();
    expect(
      await screen.findByText(/direct mode: no proxy-level DNS or fail-closed guarantee/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "停止" })).toBeEnabled();
  });

  it("calls runtime_stop when the runtime is running", async () => {
    const calls: string[] = [];
    const command = vi.fn(async (name: string) => {
      calls.push(name);
      if (name === "runtime_status") return { state: "running", isolation: "enforced" };
      if (name === "runtime_stop") return { state: "stopped", isolation: "unknown" };
      throw new Error(`unexpected command ${name}`);
    }) as unknown as ControlTransport["command"];

    renderCard(command);
    fireEvent.click(await screen.findByRole("button", { name: "停止" }));

    await waitFor(() => expect(calls).toContain("runtime_stop"));
    expect(await screen.findByText("已停止")).toBeInTheDocument();
  });

  it("surfaces a stable error code instead of pretending the runtime started", async () => {
    const command = vi.fn(async (name: string) => {
      if (name === "runtime_status") return { state: "stopped", isolation: "unknown" };
      if (name === "runtime_start")
        throw { code: "KERNEL_NOT_FOUND", message: "kernel executable is missing" };
      throw new Error(`unexpected command ${name}`);
    }) as unknown as ControlTransport["command"];

    renderCard(command);
    fireEvent.click(await screen.findByRole("button", { name: "启动" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("KERNEL_NOT_FOUND");
    expect(screen.getByText("已停止")).toBeInTheDocument();
  });
});

describe("ProfileForm safeguards", () => {
  it("focuses the name field on validation failure and does not render credential values", async () => {
    const { command, mock } = createCommandMock();
    renderForm(command);
    fireEvent.submit(
      screen.getByRole("button", { name: "创建档案" }).closest("form") as HTMLFormElement,
    );
    expect(await screen.findByText("请输入档案名称")).toBeInTheDocument();
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "名称" }));
    expect(screen.queryByLabelText(/密码|password/i)).not.toBeInTheDocument();
    expect(screen.getByText(/凭据不会回显/)).toBeInTheDocument();
    expect(mock).not.toHaveBeenCalled();
  });
  it("asks before leaving dirty form and stays when navigation is cancelled", () => {
    const { command } = createCommandMock();
    renderForm(command);
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), {
      target: { value: "Changed" },
    });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(confirm).toHaveBeenCalledWith("有未保存的更改，确定离开吗？");
    expect(screen.getByDisplayValue("Changed")).toBeInTheDocument();
    confirm.mockRestore();
  });
  it("disables submit and sends only one command while saving", async () => {
    const { command, mock } = createCommandMock(() => new Promise(() => undefined));
    renderForm(command);
    fireEvent.change(screen.getByRole("textbox", { name: "名称" }), {
      target: { value: "One submit" },
    });
    const form = screen
      .getByRole("button", { name: "创建档案" })
      .closest("form") as HTMLFormElement;
    fireEvent.submit(form);
    fireEvent.submit(form);
    await waitFor(() => expect(mock).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: "保存中…" })).toBeDisabled();
  });
});
