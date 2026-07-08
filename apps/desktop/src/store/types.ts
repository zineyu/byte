import type { DaemonConnectionView as GeneratedDaemonConnectionView } from "../generated/DaemonConnectionView";
import type { JsonValue } from "../generated/serde_json/JsonValue";
import type { RuntimeEvent as GeneratedRuntimeEvent } from "../generated/RuntimeEvent";
import type { SessionSummary as GeneratedSessionSummary } from "../generated/SessionSummary";
import type { SessionView as GeneratedSessionView } from "../generated/SessionView";
import type { Message as GeneratedMessage } from "../generated/Message";
import type { MessageBody as GeneratedMessageBody } from "../generated/MessageBody";
import type { ToolCall } from "../generated/ToolCall";

export type LoadState = "loading" | "ready" | "error";

// ts-rs emits an externally-tagged shape for MessageBlock, but the wire format
// is internally tagged (Rust: #[serde(tag = "type", rename_all = "camelCase")]).
// Use runtime-correct types at the frontend boundaries and cast generated values.
export type MessageBlock =
  | { type: "text"; text: string }
  | ({ type: "toolCall" } & ToolCall);

export type MessageBody = Array<MessageBlock>;

// ts-rs emits an externally-tagged shape for BlockDelta, but the wire format
// is internally tagged (Rust: #[serde(tag = "type", rename_all = "camelCase")]).
// Use the runtime-correct discriminated union at frontend boundaries.
export type BlockDelta =
  | { type: "textDelta"; delta: string }
  | {
      type: "toolCallDelta";
      id: string | null;
      name: string | null;
      argumentsDelta: string | null;
    };

export type Message = Omit<GeneratedMessage, "body" | "toolCallId"> & {
  body: MessageBody;
  toolCallId?: string | null;
};

export type SessionView = Omit<GeneratedSessionView, "messages"> & {
  messages: Message[];
};

export type SessionSummary = GeneratedSessionSummary;

export function asMessageBody(body: GeneratedMessageBody): MessageBody {
  return body as unknown as MessageBody;
}

export function asMessage(message: GeneratedMessage): Message {
  return { ...message, body: asMessageBody(message.body) };
}

export function asSessionView(session: GeneratedSessionView): SessionView {
  return { ...session, messages: session.messages.map(asMessage) };
}

// ts-rs flattens tagged enums into an intersection whose keys are the variant
// names. Project that back into a TypeScript-friendly discriminated union that
// mirrors the on-the-wire JSON shape (`{ type, ...fields }`).
type RuntimeEventVariant<E> = E extends { sequence: number } & infer U
  ? U extends infer U1
    ? U1 extends object
      ? {
          [K in keyof U1]: K extends "message_completed"
            ? { type: K } & Omit<U1[K], "body"> & { body: MessageBody | null }
            : K extends "message_delta"
              ? { type: K } & Omit<U1[K], "delta"> & { delta: BlockDelta }
              : { type: K } & U1[K];
        }[keyof U1]
      : never
    : never
  : never;

export type RuntimeEvent = {
  sequence: GeneratedRuntimeEvent["sequence"];
} & RuntimeEventVariant<GeneratedRuntimeEvent>;

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

export type DaemonConnectionView = GeneratedDaemonConnectionView;

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
