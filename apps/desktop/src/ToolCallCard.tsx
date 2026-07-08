import {
  AlertCircle,
  ChevronDown,
  File,
  FilePenLine,
  FilePlus,
  FileSearch,
  Folder,
  Loader2,
  Search,
  Terminal,
  XCircle,
} from "lucide-react";
import { useState } from "react";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { ToolCallState } from "./store";

type ToolCallCardProps = {
  toolCall: ToolCallState | undefined;
};

export function ToolCallCard({ toolCall }: ToolCallCardProps) {
  if (!toolCall) {
    return (
      <div className="tool-call-card tool-call-card--loading">
        <div className="tool-call-header">
          <Loader2 size={16} className="tool-call-spinner" aria-hidden="true" />
          <span className="tool-call-name">工具</span>
          <span className="tool-call-status-badge tool-call-status-badge--running">
            运行中
          </span>
        </div>
      </div>
    );
  }

  const isRunning = toolCall.status === "running";
  const isError = toolCall.status === "error";
  const [isExpanded, setIsExpanded] = useState(false);

  return (
    <div
      className={`tool-call-card ${isRunning ? "tool-call-card--running" : ""} ${isError ? "tool-call-card--error" : ""}`}
    >
      <div className="tool-call-header">
        <ToolIcon name={toolCall.name} />
        <span className="tool-call-name">
          {argumentSummary(toolCall.name, toolCall.arguments)}
        </span>
        <StatusBadge status={toolCall.status} />
        <button
          type="button"
          className="tool-call-toggle"
          aria-expanded={isExpanded}
          aria-label={isExpanded ? "折叠" : "展开"}
          onClick={() => setIsExpanded((prev) => !prev)}
        >
          <ChevronDown
            size={16}
            className={`tool-call-toggle__icon ${isExpanded ? "tool-call-toggle__icon--expanded" : ""}`}
            aria-hidden="true"
          />
        </button>
      </div>
      {isExpanded && isError && (
        <div className="tool-call-error" role="alert">
          <AlertCircle size={14} aria-hidden="true" />
          <span>{toolCall.error ?? "工具执行失败"}</span>
        </div>
      )}
      {isExpanded &&
        (toolCall.status === "completed" ||
          toolCall.status === "error" ||
          (toolCall.status === "running" && toolCall.output)) && (
          <div className="tool-call-body">
            <ToolOutput
              name={toolCall.name}
              arguments={toolCall.arguments}
              output={toolCall.output}
              exitCode={toolCall.exitCode}
            />
          </div>
        )}
    </div>
  );
}

function ToolIcon({ name }: { name: string }) {
  if (name === "read_file") {
    return <File size={14} aria-hidden="true" />;
  }
  if (name === "list_directory") {
    return <Folder size={14} aria-hidden="true" />;
  }
  if (name === "grep") {
    return <FileSearch size={14} aria-hidden="true" />;
  }
  if (name === "find_files") {
    return <Search size={14} aria-hidden="true" />;
  }
  if (name === "apply_patch") {
    return <FilePenLine size={14} aria-hidden="true" />;
  }
  if (name === "write_file") {
    return <FilePlus size={14} aria-hidden="true" />;
  }
  if (name === "run_command") {
    return <Terminal size={14} aria-hidden="true" />;
  }
  return <Search size={14} aria-hidden="true" />;
}

const STATUS_LABELS: Record<ToolCallState["status"], string> = {
  running: "运行中",
  completed: "已完成",
  error: "失败",
};

function StatusBadge({ status }: { status: ToolCallState["status"] }) {
  return (
    <span
      className={`tool-call-status-badge tool-call-status-badge--${status}`}
    >
      {status === "running" && (
        <Loader2
          size={12}
          className="tool-call-status-spinner"
          aria-hidden="true"
        />
      )}
      {status === "error" && (
        <XCircle
          size={12}
          className="tool-call-status-error"
          aria-hidden="true"
        />
      )}
      {STATUS_LABELS[status]}
    </span>
  );
}

function argumentSummary(name: string, arguments_: JsonValue): string {
  if (
    typeof arguments_ !== "object" ||
    arguments_ === null ||
    Array.isArray(arguments_)
  ) {
    return name;
  }

  const pairs: string[] = [];
  const entries = Object.entries(arguments_);
  for (const [key, value] of entries.slice(0, 3)) {
    const display = formatJsonValue(value);
    pairs.push(`${key}: ${display}`);
  }
  if (entries.length > 3) {
    pairs.push("...");
  }
  return `${name}(${pairs.join(", ")})`;
}

function formatJsonValue(value: JsonValue): string {
  if (typeof value === "string") return `"${value}"`;
  if (typeof value === "number" || typeof value === "boolean")
    return String(value);
  if (value === null) return "null";
  return "...";
}

function ToolOutput({
  name,
  arguments: arguments_,
  output,
  exitCode,
}: {
  name: string;
  arguments: JsonValue;
  output: string | null;
  exitCode: number | null;
}) {
  if (!output) {
    return <span className="tool-call-empty">无输出</span>;
  }

  if (name === "read_file") {
    const path = getArgumentString(arguments_, "path") ?? "文件";
    return (
      <>
        <div className="tool-call-file-header">{path}</div>
        <pre className="tool-call-pre">{output}</pre>
      </>
    );
  }

  if (name === "list_directory") {
    const entries = parseJsonArray(output);
    const path = getArgumentString(arguments_, "path") ?? ".";
    return (
      <>
        <div className="tool-call-list-header">
          <span>{path}</span>
          <span>{entries.length} 项</span>
        </div>
        <ul className="tool-call-list">
          {entries.map((entry, index) => {
            const nameValue =
              typeof entry === "object" && entry !== null ? entry.name : entry;
            const typeValue =
              typeof entry === "object" && entry !== null ? entry.type : "file";
            return (
              <li key={index} className="tool-call-list-item">
                {typeValue === "directory" ? (
                  <Folder size={14} aria-hidden="true" />
                ) : (
                  <File size={14} aria-hidden="true" />
                )}
                <span>{String(nameValue ?? "")}</span>
              </li>
            );
          })}
        </ul>
      </>
    );
  }

  if (name === "grep") {
    const parsed = parseGrepOutput(output);
    return (
      <div className="tool-call-grep">
        <div className="tool-call-grep-matches">
          {parsed.matches.map((match, index) => (
            <div key={index} className="tool-call-grep-line">
              <span className="tool-call-grep-location">
                {match.path}:{match.line}
              </span>
              <span className="tool-call-grep-content">{match.content}</span>
            </div>
          ))}
        </div>
        {parsed.warnings.length > 0 && (
          <div className="tool-call-grep-warnings">
            {parsed.warnings.map((warning, index) => (
              <div key={index} className="tool-call-grep-warning">
                {warning}
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }

  if (name === "find_files") {
    const paths = parseJsonArray(output).map(String);
    return (
      <ul className="tool-call-list">
        {paths.map((path, index) => (
          <li key={index} className="tool-call-list-item">
            <File size={14} aria-hidden="true" />
            <span>{path}</span>
          </li>
        ))}
      </ul>
    );
  }

  if (name === "apply_patch" || name === "write_file") {
    return <DiffView output={output} />;
  }

  if (name === "run_command") {
    const command = getArgumentString(arguments_, "command") ?? "";
    return (
      <div className="tool-call-command">
        <div className="tool-call-command-line">
          <span className="tool-call-command-prompt">$</span>
          <span className="tool-call-command-text">{command}</span>
        </div>
        <pre className="tool-call-pre">{output}</pre>
        {exitCode !== null && (
          <div className="tool-call-exit-code">
            <span
              className={`tool-call-exit-code-badge ${exitCode === 0 ? "tool-call-exit-code-badge--success" : "tool-call-exit-code-badge--error"}`}
            >
              exit {exitCode}
            </span>
          </div>
        )}
      </div>
    );
  }

  return <pre className="tool-call-pre">{output}</pre>;
}

function getArgumentString(arguments_: JsonValue, key: string): string | null {
  if (
    typeof arguments_ !== "object" ||
    arguments_ === null ||
    Array.isArray(arguments_)
  ) {
    return null;
  }
  const value = arguments_[key];
  return typeof value === "string" ? value : null;
}

function parseJsonArray(
  output: string,
): Array<{ [key: string]: JsonValue } | string | number | boolean | null> {
  try {
    const parsed = JSON.parse(output);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function parseGrepOutput(output: string): {
  matches: Array<{ path: string; line: number; content: string }>;
  warnings: string[];
} {
  try {
    const parsed = JSON.parse(output) as {
      matches?: Array<{ path?: string; line?: number; content?: string }>;
      warnings?: string[];
    };
    return {
      matches: (parsed.matches ?? []).map((match) => ({
        path: String(match.path ?? ""),
        line: Number(match.line ?? 0),
        content: String(match.content ?? ""),
      })),
      warnings: (parsed.warnings ?? []).map(String),
    };
  } catch {
    return { matches: [], warnings: [] };
  }
}

type DiffLineType =
  | "summary"
  | "header"
  | "hunk"
  | "delete"
  | "insert"
  | "context";

function classifyDiffLine(line: string): DiffLineType {
  if (line.startsWith("---")) return "header";
  if (line.startsWith("+++")) return "header";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("-")) return "delete";
  if (line.startsWith("+")) return "insert";
  return "context";
}

function DiffView({ output }: { output: string }) {
  const lines = output.split("\n");
  let diffStarted = false;
  return (
    <div className="tool-call-diff">
      {lines.map((line, index) => {
        if (!diffStarted && line.startsWith("--- ")) {
          diffStarted = true;
        }
        const lineType = diffStarted ? classifyDiffLine(line) : "summary";
        return (
          <div
            key={index}
            className={`tool-call-diff-line tool-call-diff-line--${lineType}`}
          >
            <pre>{line}</pre>
          </div>
        );
      })}
    </div>
  );
}
