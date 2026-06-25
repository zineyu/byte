import type { DaemonConnectionView as GeneratedDaemonConnectionView } from "../generated/DaemonConnectionView";
import type { RuntimeEvent as GeneratedRuntimeEvent } from "../generated/RuntimeEvent";
import type { SessionView } from "../generated/SessionView";

export type LoadState = "loading" | "ready" | "error";

// ts-rs flattens tagged enums into an intersection whose keys are the variant
// names. Project that back into a TypeScript-friendly discriminated union that
// mirrors the on-the-wire JSON shape (`{ type, ...fields }`).
type RuntimeEventVariant<E> = E extends { sequence: number } & infer U
	? U extends infer U1
		? U1 extends object
			? { [K in keyof U1]: { type: K } & U1[K] }[keyof U1]
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
	role: "developer" | "assistant";
	content: string;
	status: "streaming" | "completed" | "error";
	error?: string;
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
