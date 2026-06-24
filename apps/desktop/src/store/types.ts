import type { DaemonState } from "../generated/DaemonState";
import type { MessageRole } from "../generated/MessageRole";
import type { SessionView } from "../generated/SessionView";

export type LoadState = "loading" | "ready" | "error";

export type RuntimeEvent =
  | { sequence: number; type: "daemon_started"; state: DaemonState }
  | { sequence: number; type: "state_changed"; state: DaemonState }
  | { sequence: number; type: "error"; run_id?: string; message: string }
  | {
      sequence: number;
      type: "run_started";
      session_id: string;
      run_id: string;
    }
  | {
      sequence: number;
      type: "run_finished";
      run_id: string;
      status: "succeeded" | "failed";
      error?: string;
    }
  | {
      sequence: number;
      type: "message_started";
      run_id: string;
      message_id: string;
      role: MessageRole;
    }
  | {
      sequence: number;
      type: "message_delta";
      run_id: string;
      message_id: string;
      delta: string;
    }
  | {
      sequence: number;
      type: "message_completed";
      run_id: string;
      message_id: string;
    };

export type RuntimeEventLogEntry = RuntimeEvent & {
  receivedAt: Date;
};

export type ChatMessage = {
  id: string;
  role: "developer" | "assistant";
  content: string;
  status: "streaming" | "completed" | "error";
  error?: string;
};

export type ChatRunState = {
  runId: string | null;
  isSending: boolean;
};

export type DaemonConnectionView = {
  connected: boolean;
  state: DaemonState | null;
  error: string | null;
};

export type AppState = {
  loadState: LoadState;
  connection: DaemonConnectionView;
  events: RuntimeEventLogEntry[];
  messages: ChatMessage[];
  runState: ChatRunState;
};

export type StoreAction =
  | { type: "runtime_event"; event: RuntimeEvent }
  | { type: "load_session"; session: SessionView }
  | { type: "reset_session" }
  | { type: "send_message"; id: string; content: string }
  | {
      type: "set_connection";
      connection: DaemonConnectionView;
      loadState?: LoadState;
    }
  | { type: "set_load_state"; loadState: LoadState };
