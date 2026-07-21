import { useCallback, useReducer } from "react";
import { initialState } from "./initialState";
import { reducer } from "./reducer";
import type { SessionSummary } from "../generated/SessionSummary";
import type { SessionView } from "../generated/SessionView";
import type {
  DaemonConnectionView,
  LoadState,
  RuntimeEvent,
  StoreAction,
} from "./types";

export type StoreDispatch = (action: StoreAction) => void;

export function useByteStore() {
  const [state, dispatch] = useReducer(reducer, initialState);

  const applyEvent = useCallback((event: RuntimeEvent) => {
    dispatch({ type: "runtime_event", event });
  }, []);

  const loadSession = useCallback((session: SessionView) => {
    dispatch({ type: "load_session", session });
  }, []);

  const setSessions = useCallback((sessions: SessionSummary[]) => {
    dispatch({ type: "set_sessions", sessions });
  }, []);

  const addSession = useCallback((session: SessionSummary) => {
    dispatch({ type: "add_session", session });
  }, []);

  const removeSession = useCallback((sessionId: string) => {
    dispatch({ type: "remove_session", sessionId });
  }, []);

  const setCurrentSessionId = useCallback((sessionId: string | null) => {
    dispatch({ type: "set_current_session_id", sessionId });
  }, []);

  const resetSession = useCallback(() => {
    dispatch({ type: "reset_session" });
  }, []);

  const sendMessage = useCallback((id: string, content: string) => {
    dispatch({ type: "send_message", id, content });
  }, []);

  const setConnection = useCallback(
    (connection: DaemonConnectionView, loadState?: LoadState) => {
      dispatch({ type: "set_connection", connection, loadState });
    },
    [],
  );

  const setLoadState = useCallback((loadState: LoadState) => {
    dispatch({ type: "set_load_state", loadState });
  }, []);

  return {
    state,
    dispatch,
    applyEvent,
    loadSession,
    setSessions,
    addSession,
    removeSession,
    setCurrentSessionId,
    resetSession,
    sendMessage,
    setConnection,
    setLoadState,
  };
}
