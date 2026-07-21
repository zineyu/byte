import type { DaemonConnectionView } from "../generated/DaemonConnectionView";
import type { JsonValue } from "../generated/serde_json/JsonValue";
import type { MessageBody } from "../generated/MessageBody";
import type { RuntimeEvent } from "../generated/RuntimeEvent";
import type { SessionSummary } from "../generated/SessionSummary";
import type { SessionView } from "../generated/SessionView";
import type { ToolCall } from "../generated/ToolCall";

// ts-rs is configured with its `serde-compat` feature, so the generated
// protocol types match the on-the-wire JSON shapes (internally tagged enums,
// flattened fields, optional serde defaults) and are used directly without a
// correction layer.
export type { BlockDelta } from "../generated/BlockDelta";
export type { DaemonConnectionView } from "../generated/DaemonConnectionView";
export type { Message } from "../generated/Message";
export type { MessageBlock } from "../generated/MessageBlock";
export type { MessageBody } from "../generated/MessageBody";
export type { RuntimeEvent } from "../generated/RuntimeEvent";
export type { SessionSummary } from "../generated/SessionSummary";
export type { SessionView } from "../generated/SessionView";

export type LoadState = "loading" | "ready" | "error";

export type RuntimeEventLogEntry = RuntimeEvent & {
  receivedAt: Date;
};

export type ChatMessage = {
  id: string;
  role: "developer" | "assistant" | "tool" | "summary";
  content: string;
  body: MessageBody;
  status: "streaming" | "completed" | "error";
  error?: string;
  timestamp: string | null;
  firstMessageId?: string;
  lastMessageId?: string;
};

export function getMessageBodyText(body: MessageBody): string {
  return body
    .filter((b): b is { type: "text"; text: string } => b.type === "text")
    .map((b) => b.text)
    .join("");
}

export function getMessageBodyToolCalls(body: MessageBody): ToolCall[] {
  return body
    .filter((b): b is { type: "toolCall" } & ToolCall => b.type === "toolCall")
    .map((b) => ({ id: b.id, name: b.name, arguments: b.arguments }));
}

export type ToolCallState = {
  toolCallId: string;
  messageId: string;
  runId: string;
  name: string;
  arguments: JsonValue;
  status: "running" | "completed" | "error";
  output: string | null;
  error: string | null;
  exitCode: number | null;
};

export type ChatRunState = {
  runId: string | null;
  isSending: boolean;
};

export type AppState = {
  loadState: LoadState;
  connection: DaemonConnectionView;
  events: RuntimeEventLogEntry[];
  sessions: SessionSummary[];
  currentSessionId: string | null;
  messages: ChatMessage[];
  toolCalls: Record<string, ToolCallState>;
  runState: ChatRunState;
  workspaceInstructions: string | null;
  workspaceInstructionsError: string | null;
};

export type TimelineMessageItem = {
  type: "message";
  id: string;
  message: ChatMessage;
};

export type TimelineSummaryItem = {
  type: "summary";
  id: string;
  message: ChatMessage;
  firstMessageId: string;
  lastMessageId: string;
};

export type TimelineItem = TimelineMessageItem | TimelineSummaryItem;

export type StoreAction =
  | { type: "runtime_event"; event: RuntimeEvent }
  | { type: "load_session"; session: SessionView }
  | { type: "set_sessions"; sessions: SessionSummary[] }
  | { type: "add_session"; session: SessionSummary }
  | { type: "remove_session"; sessionId: string }
  | { type: "set_current_session_id"; sessionId: string | null }
  | { type: "reset_session" }
  | { type: "send_message"; id: string; content: string }
  | {
      type: "set_connection";
      connection: DaemonConnectionView;
      loadState?: LoadState;
    }
  | { type: "set_load_state"; loadState: LoadState };
