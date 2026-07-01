import { describe, expect, it } from "vitest";
import { buildTimelineItems, groupEvents } from "./selectors";
import type { ChatMessage, RuntimeEventLogEntry } from "./types";

function makeEvent(
  type: RuntimeEventLogEntry["type"],
  sequence: number,
  status: string = "ready",
): RuntimeEventLogEntry {
  const base = { sequence, receivedAt: new Date("2026-06-24T00:00:00.000Z") };
  switch (type) {
    case "daemon_started":
    case "state_changed":
      return {
        ...base,
        type,
        state: {
          status: status as "ready",
          daemon_version: "0.1.0",
          protocol_version: 1,
        },
      };
    case "run_started":
      return { ...base, type, session_id: "s1", run_id: "r1" };
    case "run_finished":
      return { ...base, type, run_id: "r1", status: "succeeded", error: null };
    case "message_started":
      return {
        ...base,
        type,
        run_id: "r1",
        message_id: "m1",
        role: "assistant",
      };
    case "message_delta":
      return { ...base, type, run_id: "r1", message_id: "m1", delta: "hi" };
    case "message_completed":
      return {
        ...base,
        type,
        run_id: "r1",
        message_id: "m1",
        tool_calls: null,
      };
    case "tool_started":
      return {
        ...base,
        type,
        run_id: "r1",
        tool_call_id: "tc1",
        name: "read_file",
      };
    case "tool_delta":
      return {
        ...base,
        type,
        run_id: "r1",
        tool_call_id: "tc1",
        message: "progress",
      };
    case "tool_finished":
      return {
        ...base,
        type,
        run_id: "r1",
        tool_call_id: "tc1",
        output: "ok",
        is_error: false,
      };
    case "error":
      return { ...base, type, message: "oops", run_id: null };
    case "run_cancelled":
      return { ...base, type, run_id: "r1" };
    case "session_changed":
      return { ...base, type, session_id: "s1", action: "created" };
  }
}

describe("buildTimelineItems", () => {
  it("returns message items for messages without tool calls", () => {
    const messages: ChatMessage[] = [
      { id: "m1", role: "developer", content: "hi", status: "completed" },
      { id: "m2", role: "assistant", content: "hello", status: "completed" },
    ];

    const items = buildTimelineItems(messages);

    expect(items).toHaveLength(2);
    expect(items[0]).toEqual({
      type: "message",
      id: "m1",
      message: messages[0],
    });
    expect(items[1]).toEqual({
      type: "message",
      id: "m2",
      message: messages[1],
    });
  });

  it("inserts tool_call items after the assistant message that requested them", () => {
    const messages: ChatMessage[] = [
      {
        id: "m1",
        role: "assistant",
        content: "I'll search",
        status: "completed",
        toolCalls: [{ id: "tc1", name: "grep", arguments: { pattern: "foo" } }],
      },
    ];

    const items = buildTimelineItems(messages);

    expect(items).toHaveLength(2);
    expect(items[0].type).toBe("message");
    expect(items[1]).toEqual({
      type: "tool_call",
      id: "tool-tc1",
      toolCallId: "tc1",
    });
  });

  it("preserves order across multiple assistant messages with tool calls", () => {
    const messages: ChatMessage[] = [
      { id: "m1", role: "developer", content: "a", status: "completed" },
      {
        id: "m2",
        role: "assistant",
        content: "b",
        status: "completed",
        toolCalls: [{ id: "tc1", name: "read_file", arguments: { path: "a" } }],
      },
      { id: "m3", role: "assistant", content: "c", status: "completed" },
    ];

    const items = buildTimelineItems(messages);

    expect(items.map((item) => item.type)).toEqual([
      "message",
      "message",
      "tool_call",
      "message",
    ]);
    expect(items[2]).toEqual({
      type: "tool_call",
      id: "tool-tc1",
      toolCallId: "tc1",
    });
  });
});

describe("groupEvents", () => {
  it("returns events as singleton groups by default", () => {
    const events: RuntimeEventLogEntry[] = [
      makeEvent("run_started", 1),
      makeEvent("message_started", 2),
      makeEvent("message_delta", 3),
      makeEvent("message_completed", 4),
    ];

    const grouped = groupEvents(events);

    expect(grouped).toHaveLength(4);
    expect(grouped.every((group) => group.count === 1)).toBe(true);
  });

  it("collapses consecutive state_changed events with the same status", () => {
    const events: RuntimeEventLogEntry[] = [
      makeEvent("state_changed", 1),
      makeEvent("state_changed", 2),
      makeEvent("state_changed", 3),
      makeEvent("run_started", 4),
    ];

    const grouped = groupEvents(events);

    expect(grouped).toHaveLength(2);
    expect(grouped[0]).toEqual({ event: events[0], count: 3 });
    expect(grouped[1]).toEqual({ event: events[3], count: 1 });
  });

  it("does not collapse state_changed events with different statuses", () => {
    const events: RuntimeEventLogEntry[] = [
      makeEvent("state_changed", 1, "ready"),
      makeEvent("state_changed", 2, "loading"),
    ];

    const grouped = groupEvents(events);

    expect(grouped).toHaveLength(2);
  });

  it("caps grouped events at TIMELINE_MAX_EVENTS", () => {
    const events: RuntimeEventLogEntry[] = Array.from(
      { length: 40 },
      (_, index) => makeEvent("message_delta", index + 1),
    );

    const grouped = groupEvents(events);

    expect(grouped).toHaveLength(32);
  });
});
