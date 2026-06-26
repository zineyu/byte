import type { SessionMessage } from "../generated/SessionMessage";
import type {
  AppState,
  RuntimeEvent,
  RuntimeEventLogEntry,
  StoreAction,
} from "./types";
// Hard cap for the in-memory runtime event log.
const MAX_EVENTS = 64;

export function reducer(state: AppState, action: StoreAction): AppState {
  switch (action.type) {
    case "runtime_event":
      return applyRuntimeEvent(state, action.event);
    case "load_session":
      return {
        ...state,
        currentSessionId: action.session.sessionId,
        messages: action.session.messages
          .filter(
            (
              message,
            ): message is SessionMessage & {
              role: "developer" | "assistant";
            } => message.role === "developer" || message.role === "assistant",
          )
          .map((message) => ({
            id: message.id,
            role: message.role,
            content: message.content,
            status: "completed" as const,
          })),
      };
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
        runState: {
          runId: null,
          isSending: false,
        },
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
            status: "completed" as const,
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
            status: "streaming",
          },
        ],
      };
    }
    case "message_delta":
      return {
        ...state,
        events,
        messages: state.messages.map((message) =>
          message.id === event.message_id
            ? { ...message, content: message.content + event.delta }
            : message,
        ),
      };
    case "message_completed":
      return {
        ...state,
        events,
        messages: state.messages.map((message) =>
          message.id === event.message_id
            ? { ...message, status: "completed" as const }
            : message,
        ),
      };
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
    case "session_changed":
      return { ...state, events };
  }
}
