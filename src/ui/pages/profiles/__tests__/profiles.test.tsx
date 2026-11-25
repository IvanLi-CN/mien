import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { TransportProvider } from "../../../transport/context";
import type { ControlTransport } from "../../../transport/types";
import { ProfileForm } from "../../profile-edit/profile-form";
import { IsolationProof, ProfileStatus } from "../profile-card";

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
