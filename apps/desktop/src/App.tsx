import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowUp,
  Bot,
  FileText,
  FolderOpen,
  MessageSquare,
  Plus,
  Settings,
  Sparkles,
  Trash2,
  User,
  Wrench,
  X,
} from "lucide-react";
import type { DaemonConnectionView } from "./generated/DaemonConnectionView";
import type { SessionSummary } from "./generated/SessionSummary";
import type { SessionView } from "./generated/SessionView";
import { open } from "@tauri-apps/plugin-dialog";
import { ToolCallCard } from "./ToolCallCard";
import {
  useByteStore,
  buildTimelineItems,
  type ChatRunState,
  type RuntimeEvent,
} from "./store";

function sessionTitle(session: SessionSummary): string {
  if (session.workspace) {
    const parts = session.workspace.split(/[/\\]/);
    const last = parts[parts.length - 1];
    if (last) return last;
  }
  return shortId(session.sessionId);
}

function shortId(id: string): string {
  const parts = id.split("-");
  return parts.length >= 2
    ? `${parts[0]}…${parts[parts.length - 1].slice(-4)}`
    : id.slice(0, 8);
}

export default function App() {
  const {
    state,
    applyEvent,
    loadSession: loadSessionAction,
    setSessions,
    removeSession,
    setCurrentSessionId,
    resetSession,
    sendMessage,
    setConnection,
    setLoadState,
  } = useByteStore();

  const [input, setInput] = useState("");
  const [showSettings, setShowSettings] = useState(false);
  const initialLoadDoneRef = useRef(false);

  const {
    loadState,
    connection,
    sessions,
    currentSessionId,
    messages,
    toolCalls,
    runState,
    workspaceInstructions,
    workspaceInstructionsError,
  } = state;

  const timelineItems = buildTimelineItems(messages);

  const refreshDaemonState = useCallback(async () => {
    setLoadState("loading");
    try {
      const nextConnection =
        await invoke<DaemonConnectionView>("get_daemon_state");
      setConnection(nextConnection, "ready");
    } catch (error) {
      setConnection(
        {
          connected: false,
          state: null,
          error: error instanceof Error ? error.message : String(error),
        },
        "error",
      );
    }
  }, [setConnection, setLoadState]);

  const listSessions = useCallback(async () => {
    try {
      const nextSessions = await invoke<SessionSummary[]>("list_sessions");
      setSessions(nextSessions);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConnection(
        {
          connected: false,
          state: null,
          error: message,
        },
        "error",
      );
    }
  }, [setSessions, setConnection]);

  const loadSession = useCallback(
    async (targetSessionId: string) => {
      try {
        const session = await invoke<SessionView>("load_session", {
          sessionId: targetSessionId,
        });
        loadSessionAction(session);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        if (!message.includes("session not found")) {
          setConnection(
            {
              connected: false,
              state: null,
              error: message,
            },
            "error",
          );
        }
      }
    },
    [loadSessionAction, setConnection],
  );

  useEffect(() => {
    const unlistenPromise = listen<RuntimeEvent>("daemon-event", (event) => {
      applyEvent(event.payload);
    });

    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [applyEvent]);

  useEffect(() => {
    const setup = async () => {
      await refreshDaemonState();
      await listSessions();
    };
    void setup();
  }, [refreshDaemonState, listSessions]);

  useEffect(() => {
    if (initialLoadDoneRef.current) return;
    const latest = sessions[0];
    if (latest) {
      initialLoadDoneRef.current = true;
      void loadSession(latest.sessionId);
    }
    if (sessions.length === 0 && currentSessionId !== null) {
      resetSession();
    }
  }, [sessions, currentSessionId, loadSession, resetSession]);

  const handleSend = useCallback(async () => {
    const trimmed = input.trim();
    if (!trimmed || runState.isSending || !currentSessionId) return;

    sendMessage(`user-${Date.now()}`, trimmed);
    setInput("");

    try {
      await invoke("send_message", {
        sessionId: currentSessionId,
        message: trimmed,
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConnection(
        {
          connected: false,
          state: null,
          error: message,
        },
        "error",
      );
    }
  }, [input, runState.isSending, currentSessionId, sendMessage, setConnection]);

  const pickWorkspace = async (): Promise<string | null> => {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择代码工作区",
    });
    if (selected === null) return null;
    return Array.isArray(selected) ? (selected[0] ?? null) : selected;
  };

  const createSessionInWorkspace = useCallback(
    async (workspace: string) => {
      const newSessionId = await invoke<string>("new_session", {
        workspace,
      });
      setCurrentSessionId(newSessionId);
      resetSession();
      setInput("");
      await listSessions();
    },
    [listSessions, resetSession, setCurrentSessionId],
  );

  const handleNewChat = useCallback(async () => {
    try {
      const workspace = await pickWorkspace();
      if (workspace === null) return;
      await createSessionInWorkspace(workspace);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConnection(
        {
          connected: false,
          state: null,
          error: message,
        },
        "error",
      );
    }
  }, [createSessionInWorkspace, setConnection]);

  const handleOpenWorkspace = useCallback(async () => {
    try {
      const workspace = await pickWorkspace();
      if (workspace === null) return;
      await createSessionInWorkspace(workspace);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConnection(
        {
          connected: false,
          state: null,
          error: message,
        },
        "error",
      );
    }
  }, [createSessionInWorkspace, setConnection]);

  const handleSelectSession = useCallback(
    async (sessionId: string) => {
      if (sessionId === currentSessionId) return;
      setCurrentSessionId(sessionId);
      await loadSession(sessionId);
    },
    [currentSessionId, loadSession, setCurrentSessionId],
  );

  const handleDeleteSession = useCallback(
    async (sessionId: string) => {
      if (!confirm("确定要删除这个会话吗？此操作不可恢复。")) return;

      try {
        await invoke("delete_session", { sessionId });
        removeSession(sessionId);
        if (currentSessionId === sessionId) {
          setCurrentSessionId(null);
          resetSession();
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setConnection(
          {
            connected: false,
            state: null,
            error: message,
          },
          "error",
        );
      }
    },
    [
      currentSessionId,
      removeSession,
      resetSession,
      setConnection,
      setCurrentSessionId,
    ],
  );

  const isConnected = connection.connected;

  return (
    <main className="app-shell">
      <aside className="left-sidebar" aria-label="主导航">
        <div className="sidebar-header">
          <button
            type="button"
            className="nav-item new-chat-item"
            onClick={() => void handleNewChat()}
          >
            <span className="nav-item-icon" aria-hidden="true">
              <Plus size={16} strokeWidth={2} />
            </span>
            新对话
          </button>
          <button
            type="button"
            className="nav-item open-workspace-item"
            onClick={() => void handleOpenWorkspace()}
          >
            <span className="nav-item-icon" aria-hidden="true">
              <FolderOpen size={16} strokeWidth={2} />
            </span>
            打开工作区
          </button>
        </div>

        <nav className="session-list" aria-label="会话列表">
          {sessions.length === 0 ? (
            <p className="session-list-empty">暂无会话</p>
          ) : (
            sessions.map((session) => (
              <div
                key={session.sessionId}
                className={`nav-item session-item ${currentSessionId === session.sessionId ? "active" : ""}`}
              >
                <button
                  type="button"
                  className="session-row"
                  onClick={() => void handleSelectSession(session.sessionId)}
                  title={session.workspace ?? session.sessionId}
                >
                  <span className="nav-item-icon" aria-hidden="true">
                    <MessageSquare size={14} strokeWidth={2} />
                  </span>
                  <span className="session-item-title">
                    {sessionTitle(session)}
                  </span>
                </button>
                <button
                  type="button"
                  className="session-delete"
                  onClick={() => void handleDeleteSession(session.sessionId)}
                  aria-label="删除会话"
                  title="删除会话"
                >
                  <Trash2 size={14} strokeWidth={2} />
                </button>
              </div>
            ))
          )}
        </nav>

        <div className="nav-footer">
          <div className="connection-row">
            <span
              className={`status-dot ${isConnected ? "online" : "offline"}`}
            />
            <span>{isConnected ? "已连接" : "未连接"}</span>
          </div>
          {connection.state && (
            <span className="version-meta">
              v{connection.state.daemon_version} · 协议{" "}
              {connection.state.protocol_version}
            </span>
          )}
          <button
            type="button"
            className="refresh-link"
            onClick={refreshDaemonState}
            disabled={loadState === "loading"}
          >
            {loadState === "loading" ? "检查中…" : "刷新状态"}
          </button>
        </div>

        <nav className="nav-menu nav-menu--bottom">
          <button
            type="button"
            className={`nav-item ${showSettings ? "active" : ""}`}
            onClick={() => setShowSettings((current) => !current)}
          >
            <span className="nav-item-icon" aria-hidden="true">
              <Settings size={16} strokeWidth={2} />
            </span>
            设置
          </button>
        </nav>
      </aside>

      <section className="main-area">
        {messages.length === 0 ? (
          <div className="hero-empty">
            <div className="hero-logo">
              <span className="hero-logo-mark" aria-hidden="true">
                <Sparkles size={20} strokeWidth={2} />
              </span>
              Byte Agent
            </div>
            <h1 className="hero-title">有什么可以帮你写？</h1>
            <p className="hero-subtitle">
              本地编码助手，对话即可生成、调试和理解代码。
            </p>
            {currentSessionId && (
              <WorkspaceInstructionsCard
                instructions={workspaceInstructions}
                error={workspaceInstructionsError}
              />
            )}
            <div className="input-card hero-input-card">
              <InputField
                input={input}
                setInput={setInput}
                handleSend={handleSend}
                runState={runState}
                isConnected={isConnected}
                disabled={!currentSessionId}
              />
            </div>
          </div>
        ) : (
          <div className="chat-view">
            {currentSessionId && (
              <WorkspaceInstructionsCard
                instructions={workspaceInstructions}
                error={workspaceInstructionsError}
              />
            )}
            <div className="chat-messages">
              {timelineItems.map((item) => {
                if (item.type === "summary") {
                  return (
                    <div
                      key={item.id}
                      className="chat-message chat-message--summary"
                    >
                      <div className="chat-message__avatar" aria-hidden="true">
                        <FileText size={16} strokeWidth={2} />
                      </div>
                      <div className="chat-message__body">
                        <div className="chat-message__summary-header">
                          会话摘要
                        </div>
                        <div className="chat-message__content chat-message__content--summary">
                          {item.message.content}
                        </div>
                      </div>
                    </div>
                  );
                }

                return item.type === "message" ? (
                  <div
                    key={item.id}
                    className={`chat-message chat-message--${item.message.role}`}
                  >
                    <div className="chat-message__avatar" aria-hidden="true">
                      {item.message.role === "developer" ? (
                        <User size={16} strokeWidth={2} />
                      ) : item.message.role === "tool" ? (
                        <Wrench size={16} strokeWidth={2} />
                      ) : (
                        <Bot size={18} strokeWidth={2} />
                      )}
                    </div>
                    <div className="chat-message__body">
                      <div className="chat-message__content">
                        {item.message.content ||
                          (item.message.status === "streaming" ? "…" : "")}
                        {item.message.status === "streaming" && (
                          <span className="chat-cursor" aria-hidden="true" />
                        )}
                      </div>
                      {item.message.status === "error" && (
                        <div className="chat-message__error" role="alert">
                          {item.message.error ?? "出错了"}
                        </div>
                      )}
                    </div>
                  </div>
                ) : (
                  <ToolCallCard
                    key={item.id}
                    toolCall={toolCalls[item.toolCallId]}
                  />
                );
              })}
            </div>

            <div className="input-card chat-input-card">
              <InputField
                input={input}
                setInput={setInput}
                handleSend={handleSend}
                runState={runState}
                isConnected={isConnected}
                disabled={!currentSessionId}
              />
            </div>
          </div>
        )}
      </section>

      {showSettings && (
        <aside className="right-drawer" aria-label="右侧面板">
          <div className="drawer-header">
            <h3>设置</h3>
            <button
              type="button"
              className="drawer-close"
              onClick={() => setShowSettings(false)}
              aria-label="关闭"
            >
              <X size={18} strokeWidth={2} />
            </button>
          </div>

          <div className="drawer-body">
            <p className="settings-placeholder">
              模型与连接设置由本地配置文件管理：
              <br />
              <code>~/.config/byte/config.toml</code>
            </p>
            {connection.state && (
              <dl className="status-badges">
                <div>
                  <dt>状态</dt>
                  <dd>
                    {connection.state.status === "ready"
                      ? "就绪"
                      : connection.state.status}
                  </dd>
                </div>
                <div>
                  <dt>版本</dt>
                  <dd>{connection.state.daemon_version}</dd>
                </div>
                <div>
                  <dt>协议</dt>
                  <dd>{connection.state.protocol_version}</dd>
                </div>
              </dl>
            )}
            {connection.error && (
              <div className="drawer-error" role="alert">
                {connection.error}
              </div>
            )}
          </div>
        </aside>
      )}
    </main>
  );
}

function WorkspaceInstructionsCard({
  instructions,
  error,
}: {
  instructions: string | null;
  error: string | null;
}) {
  if (!instructions && !error) return null;

  return (
    <div className="workspace-instructions-card">
      <div className="workspace-instructions-header">
        Workspace Instructions
      </div>
      {error && (
        <div className="workspace-instructions-error" role="alert">
          {error}
        </div>
      )}
      {instructions && (
        <pre className="workspace-instructions-body">{instructions}</pre>
      )}
    </div>
  );
}

function InputField({
  input,
  setInput,
  handleSend,
  runState,
  isConnected,
  disabled,
}: {
  input: string;
  setInput: (value: string) => void;
  handleSend: () => Promise<void>;
  runState: ChatRunState;
  isConnected: boolean;
  disabled?: boolean;
}) {
  return (
    <>
      <textarea
        className="input-card-textarea"
        placeholder={disabled ? "请先选择一个会话" : "输入消息…"}
        value={input}
        onChange={(event) => setInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault();
            void handleSend();
          }
        }}
        disabled={runState.isSending || !isConnected || disabled}
        aria-label="消息"
        rows={1}
      />
      <div className="input-card-footer">
        <div className="input-card-tools">
          <button
            type="button"
            className="tool-button"
            disabled
            title="即将上线"
            aria-label="添加附件"
          >
            <Plus size={16} strokeWidth={2} />
          </button>
          <button
            type="button"
            className="tool-button"
            disabled
            title="即将上线"
          >
            <MessageSquare size={14} strokeWidth={2} />
            <span>Ask</span>
          </button>
        </div>
        <div className="input-card-actions">
          <span className="mode-badge">Chat</span>
          <button
            type="button"
            className="send-button"
            onClick={() => void handleSend()}
            disabled={
              runState.isSending || !isConnected || disabled || !input.trim()
            }
            aria-label="发送"
          >
            <ArrowUp size={18} strokeWidth={2.5} />
          </button>
        </div>
      </div>
    </>
  );
}
