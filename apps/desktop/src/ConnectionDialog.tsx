import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DaemonConnectionView } from "./generated/DaemonConnectionView";

export type ConnectionDialogMode = "initial" | "settings";

export function ConnectionDialog({
  open,
  mode,
  initialAddress,
  onClose,
  onConnected,
}: {
  open: boolean;
  mode: ConnectionDialogMode;
  initialAddress: string;
  onClose: () => void;
  onConnected: (view: DaemonConnectionView) => void;
}) {
  const [address, setAddress] = useState(initialAddress);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  if (!open) return null;

  const handleSubmit = async () => {
    setError(null);
    setLoading(true);
    try {
      const trimmed = address.trim();
      if (!trimmed) {
        setError("请输入 daemon 地址");
        return;
      }
      const view = await invoke<DaemonConnectionView>("set_daemon_address", {
        address: trimmed,
      });
      onConnected(view);
      if (!view.connected) {
        setError(view.error ?? "无法连接到 daemon");
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
    } finally {
      setLoading(false);
    }
  };

  const handleClose = () => {
    if (mode === "initial" && error) {
      // Require configuration on first launch; otherwise allow dismiss.
      return;
    }
    onClose();
  };

  return (
    <div className="modal-overlay" role="presentation" onClick={handleClose}>
      <div
        className="modal-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="connection-dialog-title"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 id="connection-dialog-title" className="modal-title">
          {mode === "initial" ? "连接到本地 Daemon" : "修改 Daemon 地址"}
        </h2>
        <p className="modal-description">
          {mode === "initial"
            ? "请输入手动启动的 byte-daemon WebSocket 地址。仅支持 127.0.0.1 或 localhost。"
            : "更新后我们会立即尝试连接新的 daemon 地址。"}
        </p>
        <div className="modal-form">
          <div className="modal-field">
            <label htmlFor="daemon-address" className="modal-label">
              Daemon 地址
            </label>
            <input
              id="daemon-address"
              type="text"
              className="modal-input"
              placeholder="127.0.0.1:8787"
              value={address}
              onChange={(event) => setAddress(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void handleSubmit();
                }
              }}
              disabled={loading}
              autoFocus
            />
            <span className="modal-hint">
              例如：127.0.0.1:8787 或 localhost:8787
            </span>
          </div>
          {error && <div className="modal-error">{error}</div>}
          <div className="modal-actions">
            {mode === "settings" && (
              <button
                type="button"
                className="modal-button modal-button--secondary"
                onClick={handleClose}
                disabled={loading}
              >
                取消
              </button>
            )}
            <button
              type="button"
              className="modal-button modal-button--primary"
              onClick={() => void handleSubmit()}
              disabled={loading || !address.trim()}
            >
              {loading ? "连接中…" : "连接"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
