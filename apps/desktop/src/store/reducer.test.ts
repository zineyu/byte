import { describe, expect, it } from "vitest";
import { initialState } from "./initialState";
import { reducer } from "./reducer";
import type { SessionView } from "../generated/SessionView";
import type { RuntimeEvent } from "./types";

const readyDaemonEvent: RuntimeEvent = {
	sequence: 1,
	type: "daemon_started",
	state: {
		status: "ready",
		daemon_version: "0.1.0",
		protocol_version: 1,
	},
};

describe("runtime event reducer", () => {
	it("applies daemon_started to connection and load state", () => {
		const next = reducer(initialState, {
			type: "runtime_event",
			event: readyDaemonEvent,
		});

		expect(next.connection.connected).toBe(true);
		expect(next.connection.state).toEqual({
			status: "ready",
			daemon_version: "0.1.0",
			protocol_version: 1,
		});
		expect(next.connection.error).toBeNull();
		expect(next.loadState).toBe("ready");
		expect(next.events).toHaveLength(1);
		expect(next.events[0].type).toBe("daemon_started");
	});

	it("applies state_changed to connection state", () => {
		const next = reducer(initialState, {
			type: "runtime_event",
			event: readyDaemonEvent,
		});
		const changed: RuntimeEvent = {
			sequence: 2,
			type: "state_changed",
			state: {
				status: "ready",
				daemon_version: "0.1.0",
				protocol_version: 1,
			},
		};

		const afterChange = reducer(next, {
			type: "runtime_event",
			event: changed,
		});

		expect(afterChange.events).toHaveLength(2);
		expect(afterChange.events[0].type).toBe("state_changed");
	});

	it("records a complete assistant streaming sequence in order", () => {
		const sessionId = "session-test-1";
		const runId = "run-test-1";
		const messageId = "msg-test-1";

		const sequence: RuntimeEvent[] = [
			{
				sequence: 2,
				type: "run_started",
				session_id: sessionId,
				run_id: runId,
			},
			{
				sequence: 3,
				type: "message_started",
				run_id: runId,
				message_id: messageId,
				role: "assistant",
			},
			{
				sequence: 4,
				type: "message_delta",
				run_id: runId,
				message_id: messageId,
				delta: "Hello",
			},
			{
				sequence: 5,
				type: "message_delta",
				run_id: runId,
				message_id: messageId,
				delta: " world",
			},
			{
				sequence: 6,
				type: "message_completed",
				run_id: runId,
				message_id: messageId,
			},
			{
				sequence: 7,
				type: "run_finished",
				run_id: runId,
				status: "succeeded",
			},
		];

		const final = sequence.reduce(
			(state, event) => reducer(state, { type: "runtime_event", event }),
			reducer(initialState, { type: "runtime_event", event: readyDaemonEvent }),
		);

		expect(final.runState).toEqual({ runId: null, isSending: false });
		expect(final.messages).toHaveLength(1);
		expect(final.messages[0]).toEqual({
			id: messageId,
			role: "assistant",
			content: "Hello world",
			status: "completed",
		});
		expect(final.events.map((event) => event.type)).toEqual([
			"run_finished",
			"message_completed",
			"message_delta",
			"message_delta",
			"message_started",
			"run_started",
			"daemon_started",
		]);
	});

	it("marks streaming messages as error when run_finished fails", () => {
		const runId = "run-fail-1";
		const messageId = "msg-fail-1";

		const afterStart = reducer(initialState, {
			type: "runtime_event",
			event: {
				sequence: 1,
				type: "run_started",
				session_id: "s1",
				run_id: runId,
			},
		});
		const afterMessage = reducer(afterStart, {
			type: "runtime_event",
			event: {
				sequence: 2,
				type: "message_started",
				run_id: runId,
				message_id: messageId,
				role: "assistant",
			},
		});
		const afterDelta = reducer(afterMessage, {
			type: "runtime_event",
			event: {
				sequence: 3,
				type: "message_delta",
				run_id: runId,
				message_id: messageId,
				delta: "partial",
			},
		});
		const afterFinish = reducer(afterDelta, {
			type: "runtime_event",
			event: {
				sequence: 4,
				type: "run_finished",
				run_id: runId,
				status: "failed",
				error: "provider timeout",
			},
		});

		expect(afterFinish.messages[0].status).toBe("error");
		expect(afterFinish.messages[0].error).toBe("provider timeout");
	});

	it("clears run state on error events that carry a run_id", () => {
		const runId = "run-error-1";
		const running = reducer(initialState, {
			type: "runtime_event",
			event: {
				sequence: 1,
				type: "run_started",
				session_id: "s1",
				run_id: runId,
			},
		});

		const afterError = reducer(running, {
			type: "runtime_event",
			event: {
				sequence: 2,
				type: "error",
				run_id: runId,
				message: "Provider config not found",
			},
		});

		expect(afterError.runState).toEqual({ runId: null, isSending: false });
		expect(afterError.connection.error).toBe("Provider config not found");
	});

	it("loads a session snapshot as completed messages", () => {
		const session: SessionView = {
			session_id: "session-load-1",
			workspace: "/home/dev/project",
			messages: [
				{
					id: "msg-1",
					parent_id: null,
					role: "developer",
					content: "Hello",
				},
				{
					id: "msg-2",
					parent_id: "msg-1",
					role: "assistant",
					content: "Hi there",
				},
			],
		};

		const next = reducer(initialState, { type: "load_session", session });

		expect(next.messages).toEqual([
			{
				id: "msg-1",
				role: "developer",
				content: "Hello",
				status: "completed",
			},
			{
				id: "msg-2",
				role: "assistant",
				content: "Hi there",
				status: "completed",
			},
		]);
	});

	it("resets session messages and run state", () => {
		const withMessage = reducer(initialState, {
			type: "load_session",
			session: {
				session_id: "session-reset",
				workspace: null,
				messages: [
					{
						id: "msg-1",
						parent_id: null,
						role: "developer",
						content: "Hello",
					},
				],
			},
		});
		const running = reducer(withMessage, {
			type: "runtime_event",
			event: {
				sequence: 1,
				type: "run_started",
				session_id: "s1",
				run_id: "r1",
			},
		});

		const next = reducer(running, { type: "reset_session" });

		expect(next.messages).toHaveLength(0);
		expect(next.runState).toEqual({ runId: null, isSending: false });
	});

	it("adds a developer message on send_message action", () => {
		const next = reducer(initialState, {
			type: "send_message",
			id: "user-1",
			content: "Hello",
		});

		expect(next.messages).toEqual([
			{
				id: "user-1",
				role: "developer",
				content: "Hello",
				status: "completed",
			},
		]);
	});

	it("sets connection state directly from a Tauri state check", () => {
		const next = reducer(initialState, {
			type: "set_connection",
			connection: {
				connected: true,
				state: {
					status: "ready",
					daemon_version: "0.1.0",
					protocol_version: 1,
				},
				error: null,
			},
		});

		expect(next.connection.connected).toBe(true);
		expect(next.loadState).toBe("ready");
	});
});
