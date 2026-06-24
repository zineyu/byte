import type {
	AppState,
	RuntimeEvent,
	RuntimeEventLogEntry,
	StoreAction,
} from "./types";

const MAX_EVENTS = 64;

export function reducer(state: AppState, action: StoreAction): AppState {
	switch (action.type) {
		case "runtime_event":
			return applyRuntimeEvent(state, action.event);
		case "load_session":
			return {
				...state,
				messages: action.session.messages.map((message) => ({
					id: message.id,
					role: message.role,
					content: message.content,
					status: "completed" as const,
				})),
			};
		case "reset_session":
			return {
				...state,
				messages: [],
				runState: {
					runId: null,
					isSending: false,
				},
			};
		case "send_message":
			return {
				...state,
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
	}
}

function applyRuntimeEvent(state: AppState, event: RuntimeEvent): AppState {
	const logEntry: RuntimeEventLogEntry = { ...event, receivedAt: new Date() };
	const events = state.events.length >= MAX_EVENTS
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
			const failed = event.status === "failed";
			return {
				...state,
				events,
				runState: {
					runId: null,
					isSending: false,
				},
				messages: failed
					? state.messages.map((message) =>
							message.status === "streaming"
								? {
										...message,
										status: "error" as const,
										error: event.error ?? "运行失败",
									}
								: message,
						)
					: state.messages,
			};
		}
	}
}
