import type { RuntimeEventLogEntry } from "./types";

export type EventGroup = {
  event: RuntimeEventLogEntry;
  count: number;
};

export const TIMELINE_MAX_EVENTS = 32;

export function groupEvents(events: RuntimeEventLogEntry[]): EventGroup[] {
  const result: EventGroup[] = [];
  const limit = Math.min(events.length, TIMELINE_MAX_EVENTS);
  for (let index = 0; index < limit; index += 1) {
    const event = events[index];
    if (event.type === "state_changed" && result.length > 0) {
      const last = result[result.length - 1];
      if (
        last.event.type === "state_changed" &&
        last.event.state.status === event.state.status
      ) {
        last.count += 1;
        continue;
      }
    }
    result.push({ event, count: 1 });
  }
  return result;
}
