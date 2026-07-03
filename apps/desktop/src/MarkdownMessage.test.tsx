import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { MarkdownMessage } from "./MarkdownMessage";

describe("MarkdownMessage", () => {
  it("renders streaming content as plain text with a cursor", () => {
    const { container } = render(
      <MarkdownMessage content="Hello " status="streaming" />,
    );

    const wrapper = container.querySelector(".markdown-body--streaming");
    expect(wrapper).toBeInTheDocument();
    expect(wrapper).toHaveClass("chat-message__content");
    expect(wrapper?.textContent).toContain("Hello ");
    expect(wrapper?.querySelector(".chat-cursor")).toBeInTheDocument();
  });

  it("renders completed Markdown as HTML", () => {
    const { container } = render(
      <MarkdownMessage content="**bold**" status="completed" />,
    );

    const strong = screen.getByText("bold");
    expect(strong.tagName).toBe("STRONG");
    expect(container.firstChild).toHaveClass("markdown-body");
    expect(container.firstChild).toHaveClass("chat-message__content");
  });

  it("renders an ellipsis for empty streaming messages", () => {
    const { container } = render(
      <MarkdownMessage content="" status="streaming" />,
    );

    const wrapper = container.querySelector(".markdown-body--streaming");
    expect(wrapper).toHaveTextContent("…");
  });

  it("sanitizes raw HTML from Markdown", () => {
    render(
      <MarkdownMessage
        content={"Hello <script>alert('xss')</script> world"}
        status="completed"
      />,
    );

    expect(document.querySelector("script")).not.toBeInTheDocument();
    expect(document.body).toHaveTextContent("Hello alert('xss') world");
  });

  it("adds safe link attributes to anchors", () => {
    render(
      <MarkdownMessage
        content="[link](https://example.com)"
        status="completed"
      />,
    );

    const link = screen.getByText("link");
    expect(link.tagName).toBe("A");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noreferrer noopener");
    expect(link).toHaveAttribute("href", "https://example.com");
  });

  it("renders inline code without the code block renderer", () => {
    render(<MarkdownMessage content="`inline code`" status="completed" />);

    const code = screen.getByText("inline code");
    expect(code.tagName).toBe("CODE");
    expect(code.closest(".markdown-code-block")).not.toBeInTheDocument();
  });

  it("renders fenced code blocks with a header, copy button, and syntax highlighter", () => {
    const { container } = render(
      <MarkdownMessage
        content={"```js\nconst x = 1;\n```"}
        status="completed"
      />,
    );

    expect(screen.getByText("js")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "复制代码" }),
    ).toBeInTheDocument();
    expect(
      container.querySelector(".markdown-code-block-body"),
    ).toBeInTheDocument();

    const codeBody = container.querySelector(".markdown-code-block-body");
    expect(codeBody?.textContent).toContain("const x = 1;");
  });

  it("copies code to the clipboard when the copy button is clicked", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn();
    vi.stubGlobal("navigator", { clipboard: { writeText } });

    render(
      <MarkdownMessage
        content={'```python\ndef hello():\n    return "hi"\n```'}
        status="completed"
      />,
    );

    await user.click(screen.getByRole("button", { name: "复制代码" }));

    expect(writeText).toHaveBeenCalledWith('def hello():\n    return "hi"');
  });

  it("renders error-state content as plain text", () => {
    const { container } = render(
      <MarkdownMessage content="**bold**" status="error" />,
    );

    const wrapper = container.querySelector(".markdown-body--streaming");
    expect(wrapper).toBeInTheDocument();
    expect(wrapper).toHaveTextContent("**bold**");
  });
});
