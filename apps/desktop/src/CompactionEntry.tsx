import { useState } from "react";
import { ChevronDown } from "lucide-react";

export type CompactionEntryProps = {
  id: string;
  summary: string;
  firstMessageId?: string;
  lastMessageId?: string;
  timestamp: string | null;
  expanded?: boolean;
};

function formatTimestamp(iso: string | null): string {
  if (!iso) return "";
  const date = new Date(iso);
  return date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function CompactionEntry({
  id,
  summary,
  firstMessageId,
  lastMessageId,
  timestamp,
  expanded: expandedProp,
}: CompactionEntryProps) {
  const [expandedState, setExpandedState] = useState(false);
  const expanded = expandedProp ?? expandedState;
  const setExpanded = expandedProp !== undefined ? undefined : setExpandedState;
  const hasRange = firstMessageId || lastMessageId;

  return (
    <div className="chat-message chat-message--summary" data-compaction-id={id}>
      <div className="chat-message__body">
        <button
          type="button"
          className="chat-message__summary-header"
          onClick={() => setExpanded?.((current) => !current)}
          aria-expanded={expanded}
          aria-controls={`compaction-body-${id}`}
          disabled={!hasRange}
          style={{
            width: "100%",
            border: "none",
            background: "transparent",
            padding: 0,
            cursor: hasRange ? "pointer" : "default",
            textAlign: "left",
          }}
        >
          <span>会话摘要</span>
          <span
            style={{
              display: "flex",
              alignItems: "center",
              gap: "0.5rem",
            }}
          >
            {timestamp && (
              <time className="chat-message__timestamp" dateTime={timestamp}>
                {formatTimestamp(timestamp)}
              </time>
            )}
            {hasRange && (
              <span
                className="compaction-toggle"
                aria-hidden="true"
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  justifyContent: "center",
                  transition: "transform 0.2s ease",
                  transform: expanded ? "rotate(180deg)" : "rotate(0deg)",
                }}
              >
                <ChevronDown size={14} strokeWidth={2} />
              </span>
            )}
          </span>
        </button>
        <div
          id={`compaction-body-${id}`}
          className="chat-message__content chat-message__content--summary"
        >
          {summary}
        </div>
        {expanded && hasRange && (
          <div className="compaction-footer">
            <span className="compaction-footer-label">Compacted messages:</span>
            <span className="compaction-footer-range">
              {firstMessageId ?? ""}
              {firstMessageId && lastMessageId ? " … " : ""}
              {lastMessageId ?? ""}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
