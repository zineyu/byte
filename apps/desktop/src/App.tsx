import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

type DaemonStatus = 'ready';

type DaemonState = {
  status: DaemonStatus;
  daemon_version: string;
  protocol_version: number;
};

type RuntimeEvent =
  | {
      sequence: number;
      type: 'daemon_started';
      state: DaemonState;
    }
  | {
      sequence: number;
      type: 'state_changed';
      state: DaemonState;
    }
  | {
      sequence: number;
      type: 'error';
      message: string;
    };

type RuntimeEventLogEntry = RuntimeEvent & {
  receivedAt: string;
};

type DaemonConnectionView = {
  connected: boolean;
  state: DaemonState | null;
  error: string | null;
};

type LoadState = 'loading' | 'ready' | 'error';

const initialConnection: DaemonConnectionView = {
  connected: false,
  state: null,
  error: null,
};

function App() {
  const [loadState, setLoadState] = useState<LoadState>('loading');
  const [connection, setConnection] = useState<DaemonConnectionView>(initialConnection);
  const [events, setEvents] = useState<RuntimeEventLogEntry[]>([]);

  const refreshDaemonState = useCallback(async () => {
    setLoadState('loading');
    try {
      const nextConnection = await invoke<DaemonConnectionView>('get_daemon_state');
      setConnection(nextConnection);
      setLoadState('ready');
    } catch (error) {
      setConnection({
        connected: false,
        state: null,
        error: error instanceof Error ? error.message : String(error),
      });
      setLoadState('error');
    }
  }, []);

  useEffect(() => {
    const unlistenPromise = listen<RuntimeEvent>('daemon-event', (event) => {
      const runtimeEvent = event.payload;

      setEvents((currentEvents) => [
        { ...runtimeEvent, receivedAt: new Date().toLocaleTimeString() },
        ...currentEvents,
      ].slice(0, 8));

      if (runtimeEvent.type === 'daemon_started' || runtimeEvent.type === 'state_changed') {
        setConnection({
          connected: true,
          state: runtimeEvent.state,
          error: null,
        });
        setLoadState('ready');
      }

      if (runtimeEvent.type === 'error') {
        setConnection((currentConnection) => ({
          ...currentConnection,
          error: runtimeEvent.message,
        }));
      }
    });

    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    void refreshDaemonState();
  }, [refreshDaemonState]);

  const statusLabel = connection.connected ? 'Connected' : 'Disconnected';
  const statusTone = connection.connected ? 'online' : 'offline';

  return (
    <main className="app-shell">
      <section className="hero-panel" aria-labelledby="app-title">
        <div className="eyebrow">Desktop Coding Agent</div>
        <h1 id="app-title">Byte Agent</h1>
        <p className="summary">
          A vertical slice that proves the desktop shell can launch the local Rust daemon, send
          LF-delimited JSON-RPC over a Unix Domain Socket, and receive runtime events.
        </p>
      </section>

      <section className="status-card" aria-labelledby="daemon-status-title">
        <div className="status-header">
          <div>
            <div className="eyebrow">Daemon connection</div>
            <h2 id="daemon-status-title">Local runtime</h2>
          </div>
          <span className={`status-pill ${statusTone}`} aria-live="polite">
            <span className="status-dot" aria-hidden="true" />
            {loadState === 'loading' ? 'Checking' : statusLabel}
          </span>
        </div>

        {connection.connected && connection.state ? (
          <dl className="state-grid">
            <div>
              <dt>Status</dt>
              <dd>{connection.state.status}</dd>
            </div>
            <div>
              <dt>Daemon version</dt>
              <dd>{connection.state.daemon_version}</dd>
            </div>
            <div>
              <dt>Protocol version</dt>
              <dd>{connection.state.protocol_version}</dd>
            </div>
          </dl>
        ) : (
          <div className="empty-state" role="status">
            <h3>Daemon is not connected</h3>
            <p>{connection.error ?? 'Waiting for the desktop shell to start the local daemon.'}</p>
          </div>
        )}

        <button className="refresh-button" type="button" onClick={refreshDaemonState} disabled={loadState === 'loading'}>
          {loadState === 'loading' ? 'Checking…' : 'Refresh state'}
        </button>

        <section className="event-log" aria-labelledby="event-log-title">
          <div className="event-log-header">
            <div>
              <div className="eyebrow">Runtime events</div>
              <h3 id="event-log-title">Daemon event stream</h3>
            </div>
            <span>{events.length} shown</span>
          </div>

          {events.length > 0 ? (
            <ol className="event-list">
              {events.map((event) => (
                <li key={`${event.sequence}-${event.receivedAt}`}>
                  <span className="event-time">{event.receivedAt}</span>
                  <span className="event-type">{event.type}</span>
                  <span className="event-detail">{eventDetail(event)}</span>
                </li>
              ))}
            </ol>
          ) : (
            <p className="event-empty">No daemon events have arrived yet.</p>
          )}
        </section>
      </section>
    </main>
  );
}

function eventDetail(event: RuntimeEvent): string {
  switch (event.type) {
    case 'daemon_started':
      return `daemon ${event.state.daemon_version} started`;
    case 'state_changed':
      return `state is ${event.state.status}`;
    case 'error':
      return event.message;
  }
}

export default App;
