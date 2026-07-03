import { render, screen } from "@testing-library/react";
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

describe("ToolCallCard", () => {
  it("renders a loading state with a running status badge", () => {
    render(<ToolCallCard toolCall={undefined} />);
    expect(screen.getByText("工具")).toBeInTheDocument();
    expect(screen.getByText("运行中")).toBeInTheDocument();
  });

  it("renders a completed list_directory result with a path header and entries", () => {
    const toolCall = buildToolCall({
      name: "list_directory",
      arguments: { path: "." },
      output: JSON.stringify([
        { name: "src", type: "directory" },
        { name: "package.json", type: "file" },
      ]),
    });

    render(<ToolCallCard toolCall={toolCall} />);

    expect(screen.getByText('list_directory(path: ".")')).toBeInTheDocument();
    expect(screen.getByText("已完成")).toBeInTheDocument();
    expect(screen.getByText(".")).toBeInTheDocument();
    expect(screen.getByText("2 项")).toBeInTheDocument();
    expect(screen.getByText("src")).toBeInTheDocument();
    expect(screen.getByText("package.json")).toBeInTheDocument();
  });

  it("renders an error state with a failure badge and message", () => {
    const toolCall = buildToolCall({
      name: "read_file",
      arguments: { path: "missing.txt" },
      status: "error",
      output: "not found",
      error: "not found",
    });

    render(<ToolCallCard toolCall={toolCall} />);

    expect(screen.getByText("失败")).toBeInTheDocument();
    expect(screen.getByText("not found")).toBeInTheDocument();
  });

  it("renders a read_file output with a file header and pre block", () => {
    const toolCall = buildToolCall({
      name: "read_file",
      arguments: { path: "README.md" },
      output: "# Hello",
    });

    render(<ToolCallCard toolCall={toolCall} />);

    expect(screen.getByText("README.md")).toBeInTheDocument();
    expect(screen.getByText("# Hello")).toBeInTheDocument();
  });

  it("renders generic tool output as a pre block", () => {
    const toolCall = buildToolCall({
      name: "custom_tool",
      arguments: {},
      output: "raw output",
    });

    render(<ToolCallCard toolCall={toolCall} />);

    expect(screen.getByText("raw output")).toBeInTheDocument();
  });
});
