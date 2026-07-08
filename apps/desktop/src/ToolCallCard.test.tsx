import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ToolCallCard } from "./ToolCallCard";
import type { ToolCallState } from "./store";

function buildToolCall(
  overrides: Partial<ToolCallState> & { name: string; arguments: object },
): ToolCallState {
  return {
    toolCallId: "tc-1",
    messageId: "m-1",
    runId: "r-1",
    status: "completed",
    output: null,
    error: null,
    ...overrides,
  } as ToolCallState;
}

async function expandToolCard() {
  const toggle = screen.getByRole("button", { name: "展开" });
  await userEvent.click(toggle);
}

describe("ToolCallCard", () => {
  it("renders a loading state with a running status badge", () => {
    render(<ToolCallCard toolCall={undefined} />);
    expect(screen.getByText("工具")).toBeInTheDocument();
    expect(screen.getByText("运行中")).toBeInTheDocument();
  });

  it("collapses output by default", () => {
    const toolCall = buildToolCall({
      name: "list_directory",
      arguments: { path: "." },
      output: JSON.stringify([{ name: "src", type: "directory" }]),
    });

    render(<ToolCallCard toolCall={toolCall} />);

    expect(screen.getByText('list_directory(path: ".")')).toBeInTheDocument();
    expect(screen.getByText("已完成")).toBeInTheDocument();
    expect(screen.queryByText("src")).not.toBeInTheDocument();
  });

  it("renders a completed list_directory result with a path header and entries", async () => {
    const toolCall = buildToolCall({
      name: "list_directory",
      arguments: { path: "." },
      output: JSON.stringify([
        { name: "src", type: "directory" },
        { name: "package.json", type: "file" },
      ]),
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText('list_directory(path: ".")')).toBeInTheDocument();
    expect(screen.getByText("已完成")).toBeInTheDocument();
    expect(screen.getByText(".")).toBeInTheDocument();
    expect(screen.getByText("2 项")).toBeInTheDocument();
    expect(screen.getByText("src")).toBeInTheDocument();
    expect(screen.getByText("package.json")).toBeInTheDocument();
  });

  it("renders an error state with a failure badge and message", async () => {
    const toolCall = buildToolCall({
      name: "read_file",
      arguments: { path: "missing.txt" },
      status: "error",
      output: "not found",
      error: "not found",
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText("失败")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("not found");
    expect(screen.getByText("missing.txt")).toBeInTheDocument();
  });

  it("renders a read_file output with a file header and pre block", async () => {
    const toolCall = buildToolCall({
      name: "read_file",
      arguments: { path: "README.md" },
      output: "# Hello",
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.getByText("# Hello")).toBeInTheDocument();
  });

  it("renders a unified diff for apply_patch output", async () => {
    const toolCall = buildToolCall({
      name: "apply_patch",
      arguments: { path: "lib.rs", patch: [{ search: "old", replace: "new" }] },
      output: [
        "applied 1 patch(es) to lib.rs",
        "",
        "--- lib.rs",
        "+++ lib.rs",
        "@@ -1 +1 @@",
        "-fn old() {}",
        "+fn new() {}",
      ].join("\n"),
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText("-fn old() {}")).toBeInTheDocument();
    expect(screen.getByText("+fn new() {}")).toBeInTheDocument();
  });

  it("renders a unified diff for write_file output", async () => {
    const toolCall = buildToolCall({
      name: "write_file",
      arguments: { path: "hello.txt", content: "Hello, world!" },
      output: [
        "wrote 13 bytes to hello.txt",
        "",
        "--- /dev/null",
        "+++ hello.txt",
        "@@ -0,0 +1 @@",
        "+Hello, world!",
      ].join("\n"),
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText("+Hello, world!")).toBeInTheDocument();
  });

  it("renders generic tool output as a pre block", async () => {
    const toolCall = buildToolCall({
      name: "custom_tool",
      arguments: {},
      output: "raw output",
    });

    render(<ToolCallCard toolCall={toolCall} />);
    await expandToolCard();

    expect(screen.getByText("raw output")).toBeInTheDocument();
  });
});
