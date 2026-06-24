import { useCallback, useReducer } from "react";
import { initialState } from "./initialState";
import { reducer } from "./reducer";
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

	return {
		state,
		dispatch,
		applyEvent,
		loadSession,
		resetSession,
		sendMessage,
		setConnection,
	};
}
