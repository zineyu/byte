import {
  AlertCircle,
  CheckCircle2,
  File,
  FileSearch,
  Folder,
  Loader2,
  Search,
  XCircle,
} from "lucide-react";
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
          <div className="tool-call-avatar tool-call-avatar--running">
            <Loader2
              size={14}
              className="tool-call-spinner"
              aria-hidden="true"
            />
          </div>
          <span className="tool-call-name">工具</span>
          <span className="tool-call-status-badge tool-call-status-badge--running">
            <Loader2
              size={12}
              className="tool-call-status-spinner"
              aria-hidden="true"
            />
            运行中
          </span>
        </div>
      </div>
    );
  }

  const isRunning = toolCall.status === "running";
  const isError = toolCall.status === "error";

  return (
    <div
      className={`tool-call-card ${isRunning ? "tool-call-card--running" : ""} ${isError ? "tool-call-card--error" : ""}`}
    >
      <div className="tool-call-header">
        <ToolAvatar name={toolCall.name} status={toolCall.status} />
        <span className="tool-call-name">
          {argumentSummary(toolCall.name, toolCall.arguments)}
        </span>
        <StatusBadge status={toolCall.status} />
      </div>
      {toolCall.status === "completed" && (
        <div className="tool-call-body">
          <ToolOutput
            name={toolCall.name}
            arguments={toolCall.arguments}
            output={toolCall.output}
          />
        </div>
      )}
      {isError && (
        <div className="tool-call-error" role="alert">
          <AlertCircle size={14} aria-hidden="true" />
          <span>{toolCall.error ?? "工具执行失败"}</span>
        </div>
      )}
    </div>
  );
}

function ToolAvatar({
  name,
  status,
}: {
  name: string;
  status: ToolCallState["status"];
}) {
  const modifier =
    status === "running"
      ? "running"
      : status === "error"
        ? "error"
        : "completed";
  return (
    <div className={`tool-call-avatar tool-call-avatar--${modifier}`}>
      <ToolIcon name={name} />
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
  return <Search size={14} aria-hidden="true" />;
}

const STATUS_LABELS: Record<ToolCallState["status"], string> = {
  running: "运行中",
  completed: "已完成",
  error: "失败",
};

function StatusIcon({ status }: { status: ToolCallState["status"] }) {
  if (status === "running") {
    return (
      <Loader2
        size={12}
        className="tool-call-status-spinner"
        aria-hidden="true"
      />
    );
  }
  if (status === "error") {
    return (
      <XCircle
        size={12}
        className="tool-call-status-error"
        aria-hidden="true"
      />
    );
  }
  return (
    <CheckCircle2
      size={12}
      className="tool-call-status-done"
      aria-hidden="true"
    />
  );
}

function StatusBadge({ status }: { status: ToolCallState["status"] }) {
  return (
    <span
      className={`tool-call-status-badge tool-call-status-badge--${status}`}
    >
      <StatusIcon status={status} />
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
}: {
  name: string;
  arguments: JsonValue;
  output: string | null;
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
