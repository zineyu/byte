import type { AppState, DaemonConnectionView } from "./types";

const initialConnection: DaemonConnectionView = {
	connected: false,
	state: null,
	error: null,
};

export const initialState: AppState = {
	loadState: "loading",
	connection: initialConnection,
	events: [],
	messages: [],
	runState: {
		runId: null,
		isSending: false,
	},
};
