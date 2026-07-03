import type {
  AppState,
  ChatMessage,
  Message,
  MessageBody,
  RuntimeEvent,
  RuntimeEventLogEntry,
  StoreAction,
  ToolCallState,
} from "./types";
import { getMessageBodyText, getMessageBodyToolCalls } from "./types";

// Hard cap for the in-memory runtime event log.
const MAX_EVENTS = 64;

export function reducer(state: AppState, action: StoreAction): AppState {
  switch (action.type) {
    case "runtime_event":
      return applyRuntimeEvent(state, action.event);
    case "load_session":
      return loadSession(state, action.session);
    case "set_sessions":
      return { ...state, sessions: action.sessions };
    case "add_session":
      return {
        ...state,
        sessions: [action.session, ...state.sessions],
      };
    case "remove_session":
      return {
        ...state,
        sessions: state.sessions.filter(
          (session) => session.sessionId !== action.sessionId,
        ),
      };
    case "set_current_session_id":
      return { ...state, currentSessionId: action.sessionId };
    case "reset_session":
      return {
        ...state,
        currentSessionId: null,
        messages: [],
        toolCalls: {},
        runState: {
          runId: null,
          isSending: false,
        },
        workspaceInstructions: null,
        workspaceInstructionsError: null,
      };
    case "send_message":
      return {
        ...state,
        runState: {
          runId: null,
          isSending: true,
        },
        messages: [
          ...state.messages,
          {
            id: action.id,
            role: "developer",
            content: action.content,
            body: [{ type: "text", text: action.content }],
            status: "completed" as const,
            timestamp: new Date().toISOString(),
          },
        ],
      };
    case "set_connection":
      return {
        ...state,
        connection: action.connection,
        loadState:
          action.loadState ??
          (action.connection.connected
            ? "ready"
            : action.connection.error
              ? "error"
              : state.loadState),
      };
    case "set_load_state":
      return {
        ...state,
        loadState: action.loadState,
      };
  }
  return state;
}

function loadSession(
  state: AppState,
  session: {
    sessionId: string;
    workspaceInstructions: string | null;
    workspaceInstructionsError: string | null;
    messages: Message[];
  },
): AppState {
  const toolCalls: Record<string, ToolCallState> = {};
  const messages: ChatMessage[] = session.messages
    .filter(
      (
        message,
      ): message is Message & {
        role: "developer" | "assistant" | "tool" | "summary";
      } =>
        message.role === "developer" ||
        message.role === "assistant" ||
        message.role === "tool" ||
        message.role === "summary",
    )
    .map((message) => {
      const content = getMessageBodyText(message.body);
      return {
        id: message.id,
        role: message.role,
        content,
        body: message.body,
        status: "completed" as const,
        timestamp: null,
      };
    });

  return {
    ...state,
    currentSessionId: session.sessionId,
    workspaceInstructions: session.workspaceInstructions ?? null,
    workspaceInstructionsError: session.workspaceInstructionsError ?? null,
    messages,
    toolCalls,
  };
}

function applyRuntimeEvent(state: AppState, event: RuntimeEvent): AppState {
  const logEntry: RuntimeEventLogEntry = { ...event, receivedAt: new Date() };
  const events =
    state.events.length >= MAX_EVENTS
      ? [logEntry, ...state.events.slice(0, MAX_EVENTS - 1)]
      : [logEntry, ...state.events];

  switch (event.type) {
    case "daemon_started":
    case "state_changed":
      return {
        ...state,
        events,
        loadState: "ready",
        connection: {
          connected: true,
          state: event.state,
          error: null,
        },
      };
    case "error": {
      const nextState: AppState = {
        ...state,
        events,
        connection: {
          ...state.connection,
          error: event.message,
        },
      };
      if (event.run_id) {
        nextState.runState = { runId: null, isSending: false };
        nextState.messages = state.messages.map((message) =>
          message.status === "streaming"
            ? { ...message, status: "error" as const, error: event.message }
            : message,
        );
      }
      return nextState;
    }
    case "run_started":
      return {
        ...state,
        events,
        runState: {
          runId: event.run_id,
          isSending: true,
        },
      };
    case "message_started": {
      if (event.role !== "assistant") {
        return { ...state, events };
      }
      return {
        ...state,
        events,
        messages: [
          ...state.messages,
          {
            id: event.message_id,
            role: "assistant",
            content: "",
            body: [{ type: "text", text: "" }],
            status: "streaming",
            timestamp: new Date().toISOString(),
          },
        ],
      };
    }
    case "message_delta": {
      if (event.delta.type !== "textDelta") {
        return { ...state, events };
      }
      const textDelta = event.delta.delta;
      return {
        ...state,
        events,
        messages: state.messages.map((message) => {
          if (message.id !== event.message_id) return message;
          const newBody = [...message.body];
          const block = newBody[event.block_index];
          if (block?.type === "text") {
            newBody[event.block_index] = {
              ...block,
              text: block.text + textDelta,
            };
          } else if (event.block_index === 0 && message.body.length === 0) {
            newBody.push({ type: "text", text: textDelta });
          }
          return {
            ...message,
            content: message.content + textDelta,
            body: newBody,
          };
        }),
      };
    }
    case "message_completed": {
      const completedBody = event.body as MessageBody | null;
      const toolCallsFromBody =
        completedBody?.reduce<Record<string, ToolCallState>>((acc, block) => {
          if (block.type !== "toolCall") return acc;
          acc[block.id] = {
            toolCallId: block.id,
            messageId: event.message_id,
            runId: event.run_id,
            name: block.name,
            arguments: block.arguments,
            status: "running",
            output: null,
            error: null,
          };
          return acc;
        }, {}) ?? {};

      return {
        ...state,
        events,
        messages: state.messages.map((message) => {
          if (message.id !== event.message_id) return message;
          const newBody = completedBody
            ? [...message.body, ...completedBody]
            : message.body;
          return {
            ...message,
            status: "completed" as const,
            timestamp: message.timestamp ?? new Date().toISOString(),
            content: completedBody
              ? message.content + getMessageBodyText(completedBody)
              : message.content,
            body: newBody,
          };
        }),
        toolCalls: {
          ...state.toolCalls,
          ...toolCallsFromBody,
        },
      };
    }
    case "run_finished": {
      const cancelled = event.status === "cancelled";
      const failed = event.status === "failed";
      return {
        ...state,
        events,
        runState: {
          runId: null,
          isSending: false,
        },
        messages: state.messages.map((message) =>
          message.status === "streaming"
            ? failed || cancelled
              ? {
                  ...message,
                  status: "error" as const,
                  error: event.error ?? (cancelled ? "已取消" : "运行失败"),
                }
              : { ...message, status: "completed" as const }
            : message,
        ),
      };
    }
    case "run_cancelled":
      return {
        ...state,
        events,
        runState: {
          runId: null,
          isSending: false,
        },
        messages: state.messages.map((message) =>
          message.status === "streaming"
            ? { ...message, status: "error" as const, error: "已取消" }
            : message,
        ),
      };
    case "tool_started": {
      const existing = state.toolCalls[event.tool_call_id];
      const messageId =
        existing?.messageId ??
        findMessageIdForToolCall(state.messages, event.tool_call_id) ??
        "";
      return {
        ...state,
        events,
        toolCalls: {
          ...state.toolCalls,
          [event.tool_call_id]: {
            toolCallId: event.tool_call_id,
            messageId,
            runId: event.run_id,
            name: event.name,
            arguments: existing?.arguments ?? null,
            status: "running",
            output: existing?.output ?? null,
            error: null,
          },
        },
      };
    }
    case "tool_finished": {
      const existing = state.toolCalls[event.tool_call_id];
      return {
        ...state,
        events,
        toolCalls: {
          ...state.toolCalls,
          [event.tool_call_id]: {
            toolCallId: event.tool_call_id,
            messageId: existing?.messageId ?? "",
            runId: event.run_id,
            name: existing?.name ?? "工具",
            arguments: existing?.arguments ?? null,
            status: event.is_error ? "error" : "completed",
            output: event.output,
            error: event.is_error ? event.output : null,
          },
        },
      };
    }
    case "session_changed":
      return { ...state, events };
  }
}

function findMessageIdForToolCall(
  messages: ChatMessage[],
  toolCallId: string,
): string | undefined {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message.role === "assistant" &&
      getMessageBodyToolCalls(message.body).some(
        (call) => call.id === toolCallId,
      )
    ) {
      return message.id;
    }
  }
  return undefined;
}
