import { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import { PrismLight as SyntaxHighlighter } from "react-syntax-highlighter";
import bash from "react-syntax-highlighter/dist/esm/languages/prism/bash";
import javascript from "react-syntax-highlighter/dist/esm/languages/prism/javascript";
import json from "react-syntax-highlighter/dist/esm/languages/prism/json";
import markdown from "react-syntax-highlighter/dist/esm/languages/prism/markdown";
import python from "react-syntax-highlighter/dist/esm/languages/prism/python";
import rust from "react-syntax-highlighter/dist/esm/languages/prism/rust";
import toml from "react-syntax-highlighter/dist/esm/languages/prism/toml";
import typescript from "react-syntax-highlighter/dist/esm/languages/prism/typescript";
import yaml from "react-syntax-highlighter/dist/esm/languages/prism/yaml";

SyntaxHighlighter.registerLanguage("javascript", javascript);
SyntaxHighlighter.registerLanguage("js", javascript);
SyntaxHighlighter.registerLanguage("typescript", typescript);
SyntaxHighlighter.registerLanguage("ts", typescript);
SyntaxHighlighter.registerLanguage("tsx", typescript);
SyntaxHighlighter.registerLanguage("python", python);
SyntaxHighlighter.registerLanguage("py", python);
SyntaxHighlighter.registerLanguage("rust", rust);
SyntaxHighlighter.registerLanguage("rs", rust);
SyntaxHighlighter.registerLanguage("bash", bash);
SyntaxHighlighter.registerLanguage("sh", bash);
SyntaxHighlighter.registerLanguage("shell", bash);
SyntaxHighlighter.registerLanguage("json", json);
SyntaxHighlighter.registerLanguage("yaml", yaml);
SyntaxHighlighter.registerLanguage("yml", yaml);
SyntaxHighlighter.registerLanguage("toml", toml);
SyntaxHighlighter.registerLanguage("markdown", markdown);
SyntaxHighlighter.registerLanguage("md", markdown);

export type MarkdownMessageProps = {
  content: string;
  status: "streaming" | "completed" | "error";
};

const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames ?? []),
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "del",
    "input",
  ],
  attributes: {
    ...defaultSchema.attributes,
    th: ["align"],
    td: ["align"],
    input: ["type", "checked", "disabled"],
  },
};

export function MarkdownMessage({ content, status }: MarkdownMessageProps) {
  const isMarkdownReady = status === "completed";

  const renderedMarkdown = useMemo(() => {
    return (
      <div className="chat-message__content markdown-body">
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          rehypePlugins={[[rehypeSanitize, sanitizeSchema]]}
          components={{
            a: ({ node: _node, ...props }) => (
              <a {...props} target="_blank" rel="noreferrer noopener" />
            ),
            pre: ({ children }) => <>{children}</>,
            code: ({ node: _node, className, children, ...props }) => {
              const match = /language-(\w+)/.exec(className || "");
              const language = match?.[1] ?? "";
              const code = String(children).replace(/\n$/, "");

              if (!language) {
                return (
                  <code className={className} {...props}>
                    {children}
                  </code>
                );
              }

              return (
                <div className="markdown-code-block">
                  <div className="markdown-code-block-header">
                    <span className="markdown-code-block-language">
                      {language}
                    </span>
                    <button
                      type="button"
                      className="markdown-code-block-copy"
                      onClick={() => navigator.clipboard.writeText(code)}
                      aria-label="复制代码"
                    >
                      复制
                    </button>
                  </div>
                  <SyntaxHighlighter
                    language={language}
                    PreTag="div"
                    useInlineStyles={false}
                    className="markdown-code-block-body"
                  >
                    {code}
                  </SyntaxHighlighter>
                </div>
              );
            },
          }}
        >
          {content}
        </ReactMarkdown>
      </div>
    );
  }, [content]);

  if (!isMarkdownReady) {
    return (
      <div className="chat-message__content markdown-body--streaming">
        {content || (status === "streaming" ? "…" : "")}
        {status === "streaming" && (
          <span className="chat-cursor" aria-hidden="true" />
        )}
      </div>
    );
  }

  return renderedMarkdown;
}
