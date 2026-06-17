import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type DaemonStatus = 'ready';

type DaemonState = {
  status: DaemonStatus;
  daemon_version: string;
  protocol_version: number;
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
          A first vertical slice that proves the desktop shell can launch the local Rust daemon and read
          state over LF-delimited JSON-RPC on stdio.
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
      </section>
    </main>
  );
}

export default App;
