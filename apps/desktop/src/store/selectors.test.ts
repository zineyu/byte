import { describe, expect, it } from "vitest";
import { groupEvents } from "./selectors";
import type { RuntimeEventLogEntry } from "./types";

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
      return { ...base, type, run_id: "r1", status: "succeeded" };
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
      return { ...base, type, run_id: "r1", message_id: "m1" };
    case "error":
      return { ...base, type, message: "oops" };
  }
}

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
