import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  Zap,
} from "lucide-react";
import type { DaemonConnectionView } from "./generated/DaemonConnectionView";
import type { SessionSummary } from "./generated/SessionSummary";
import type { SessionView } from "./generated/SessionView";
import { open } from "@tauri-apps/plugin-dialog";
import { MarkdownMessage } from "./MarkdownMessage";
import { ToolCallCard } from "./ToolCallCard";
import {
  useByteStore,
  buildTimelineItems,
  type ChatRunState,
  type RuntimeEvent,
  type RuntimeEventLogEntry,
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
  const [showRuntimeEvents, setShowRuntimeEvents] = useState(false);
  const [sidebarMode, setSidebarMode] = useState<"chat" | "work">("chat");
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
    events,
  } = state;

  const timelineItems = buildTimelineItems(messages);

  const currentSession = useMemo(
    () => sessions.find((session) => session.sessionId === currentSessionId),
    [sessions, currentSessionId],
  );
  const currentWorkspace = currentSession?.workspace ?? null;

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
        <div className="sidebar-brand">
          <span className="sidebar-brand-icon" aria-hidden="true">
            <Sparkles size={20} strokeWidth={2} />
          </span>
          <span className="sidebar-brand-title">Byte</span>
        </div>

        <div
          className="sidebar-mode-tabs"
          role="tablist"
          aria-label="侧边栏模式"
        >
          <button
            type="button"
            role="tab"
            aria-selected={sidebarMode === "chat"}
            className={`mode-tab ${sidebarMode === "chat" ? "active" : ""}`}
            onClick={() => setSidebarMode("chat")}
          >
            Chat
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={sidebarMode === "work"}
            className={`mode-tab ${sidebarMode === "work" ? "active" : ""}`}
            onClick={() => setSidebarMode("work")}
          >
            Work
          </button>
        </div>

        <div className="sidebar-tab-content" role="tabpanel">
          {sidebarMode === "chat" ? (
            <>
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
              <nav className="session-list" aria-label="会话列表">
                {sessions.length === 0 ? (
                  <p className="session-list-empty">暂无会话</p>
                ) : (
                  sessions.map((session) => (
                    <div
                      key={session.sessionId}
                      className={`nav-item session-item ${
                        currentSessionId === session.sessionId ? "active" : ""
                      }`}
                    >
                      <button
                        type="button"
                        className="session-row"
                        onClick={() =>
                          void handleSelectSession(session.sessionId)
                        }
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
                        onClick={() =>
                          void handleDeleteSession(session.sessionId)
                        }
                        aria-label="删除会话"
                        title="删除会话"
                      >
                        <Trash2 size={14} strokeWidth={2} />
                      </button>
                    </div>
                  ))
                )}
              </nav>
            </>
          ) : (
            <div className="work-panel">
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
              {currentSessionId ? (
                <>
                  <div className="work-panel-info">
                    <div className="work-panel-label">当前工作区</div>
                    <div
                      className="work-panel-path"
                      title={currentWorkspace ?? "未设置"}
                    >
                      {currentWorkspace && currentSession
                        ? sessionTitle(currentSession)
                        : "未设置"}
                    </div>
                  </div>
                  <WorkspaceInstructionsCard
                    instructions={workspaceInstructions}
                    error={workspaceInstructionsError}
                  />
                </>
              ) : (
                <p className="work-panel-empty">选择一个会话以查看工作区</p>
              )}
            </div>
          )}
        </div>

        <nav className="nav-menu" aria-label="运行时">
          <button
            type="button"
            className={`nav-item ${showRuntimeEvents ? "active" : ""}`}
            onClick={() => setShowRuntimeEvents((current) => !current)}
          >
            <span className="nav-item-icon" aria-hidden="true">
              <Zap size={16} strokeWidth={2} />
            </span>
            运行时
          </button>
        </nav>

        <nav className="nav-menu nav-menu--bottom" aria-label="设置">
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
            <span className="beta-badge">Beta Preview</span>
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
                      <MarkdownMessage
                        content={item.message.content}
                        status={item.message.status}
                      />
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

      {(showRuntimeEvents || showSettings) && (
        <aside className="right-drawer" aria-label="右侧面板">
          <div className="drawer-header">
            <h3>
              {showRuntimeEvents && showSettings
                ? "运行时与设置"
                : showRuntimeEvents
                  ? "运行时事件"
                  : "设置"}
            </h3>
            <button
              type="button"
              className="drawer-close"
              onClick={() => {
                setShowRuntimeEvents(false);
                setShowSettings(false);
              }}
              aria-label="关闭"
            >
              <X size={18} strokeWidth={2} />
            </button>
          </div>

          <div className="drawer-body">
            {showRuntimeEvents && <RuntimeEventsPanel events={events} />}
            {showSettings && (
              <div className="drawer-panel">
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
            )}
          </div>
        </aside>
      )}
    </main>
  );
}

type CollapsedEventItem =
  | { kind: "raw"; event: RuntimeEventLogEntry }
  | {
      kind: "state_changed_group";
      status: string;
      receivedAt: Date;
      count: number;
    };

function collapseEvents(events: RuntimeEventLogEntry[]): CollapsedEventItem[] {
  const result: CollapsedEventItem[] = [];
  let current: CollapsedEventItem | null = null;

  for (const event of events) {
    if (event.type === "state_changed") {
      const status = event.state.status;
      if (
        current?.kind === "state_changed_group" &&
        current.status === status
      ) {
        current.count += 1;
      } else {
        if (current) result.push(current);
        current = {
          kind: "state_changed_group",
          status,
          receivedAt: event.receivedAt,
          count: 1,
        };
      }
    } else {
      if (current) result.push(current);
      current = { kind: "raw", event };
    }
  }
  if (current) result.push(current);
  return result;
}

function eventLabel(event: RuntimeEventLogEntry): string {
  const base = event as RuntimeEvent;
  switch (base.type) {
    case "daemon_started":
      return "Daemon 启动";
    case "state_changed":
      return "状态变更";
    case "error":
      return "错误";
    case "run_started":
      return "运行开始";
    case "run_finished":
      return "运行结束";
    case "message_started":
      return "消息开始";
    case "message_delta":
      return "消息增量";
    case "message_completed":
      return "消息完成";
    case "tool_started":
      return "工具开始";
    case "tool_finished":
      return "工具结束";
    case "run_cancelled":
      return "运行取消";
    case "session_changed":
      return "会话变更";
    default:
      return (base as RuntimeEvent).type;
  }
}

function formatTime(date: Date): string {
  return date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function getEventDetails(event: RuntimeEventLogEntry): React.ReactNode {
  const base = event as RuntimeEvent;
  switch (base.type) {
    case "daemon_started":
    case "state_changed":
      return `status: ${base.state.status}`;
    case "error":
      return base.message;
    case "run_started":
      return `run ${base.run_id.slice(0, 8)}`;
    case "run_finished":
      return `status: ${base.status}${base.error ? ` · ${base.error}` : ""}`;
    case "message_started":
      return `role: ${base.role}`;
    case "message_delta":
      return `block ${base.block_index}`;
    case "message_completed":
      return `message ${base.message_id.slice(0, 8)}`;
    case "tool_started":
      return base.name;
    case "tool_finished": {
      const output = base.is_error
        ? (base.output ?? "工具出错")
        : (base.output ?? "");
      if (output.includes("\n")) {
        return <pre>{output}</pre>;
      }
      return output;
    }
    case "run_cancelled":
      return `run ${base.run_id.slice(0, 8)}`;
    case "session_changed":
      return `action: ${base.action}`;
    default:
      return null;
  }
}

function isErrorEvent(event: RuntimeEventLogEntry): boolean {
  const base = event as RuntimeEvent;
  if (base.type === "error") return true;
  if (base.type === "tool_finished" && base.is_error) return true;
  if (base.type === "run_finished" && base.status === "failed") return true;
  return false;
}

function RuntimeEventsPanel({ events }: { events: RuntimeEventLogEntry[] }) {
  const collapsed = useMemo(() => collapseEvents(events), [events]);

  if (events.length === 0) {
    return (
      <div className="drawer-panel">
        <div className="runtime-events-empty">暂无运行时事件</div>
      </div>
    );
  }

  return (
    <div className="drawer-panel">
      <div className="runtime-events-header">
        <span className="runtime-events-title">运行时事件</span>
        <span className="runtime-events-count">{events.length} 条</span>
      </div>
      <ul className="runtime-events-list">
        {collapsed.map((item, index) => {
          if (item.kind === "state_changed_group") {
            return (
              <li
                key={`group-${index}`}
                className="runtime-event-item runtime-event-item--collapsed"
              >
                <div className="runtime-event-meta">
                  <span className="runtime-event-badge">状态变更</span>
                  <span className="runtime-event-time">
                    {formatTime(item.receivedAt)}
                  </span>
                </div>
                <div className="runtime-event-body">
                  <span className="runtime-event-status">{item.status}</span>
                  {item.count > 1 && (
                    <span className="runtime-event-count">×{item.count}</span>
                  )}
                </div>
              </li>
            );
          }

          const event = item.event;
          const isError = isErrorEvent(event);

          return (
            <li
              key={`event-${event.sequence}-${index}`}
              className={`runtime-event-item ${isError ? "runtime-event-item--error" : ""}`}
            >
              <div className="runtime-event-meta">
                <span
                  className={`runtime-event-badge runtime-event-badge--${event.type}`}
                >
                  {eventLabel(event)}
                </span>
                <span className="runtime-event-time">
                  {formatTime(event.receivedAt)}
                </span>
              </div>
              <div className="runtime-event-body">{getEventDetails(event)}</div>
            </li>
          );
        })}
      </ul>
    </div>
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
