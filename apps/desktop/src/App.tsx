import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
	ArrowUp,
	Bot,
	MessageSquare,
	Plus,
	Settings,
	Sparkles,
	User,
	X,
	Zap,
} from "lucide-react";
import type { SessionView } from "./generated/SessionView";
import {
	groupEvents,
	useByteStore,
	type ChatRunState,
	type RuntimeEvent,
	type RuntimeEventLogEntry,
} from "./store";

type RightPanel = "events" | "settings" | null;

function App() {
	const {
		state,
		applyEvent,
		loadSession: loadSessionAction,
		resetSession,
		sendMessage,
		setConnection,
	} = useByteStore();

	const [input, setInput] = useState("");
	const [sessionId, setSessionId] = useState("");
	const [rightPanel, setRightPanel] = useState<RightPanel>(null);

	const { loadState, connection, events, messages, runState } = state;

	const refreshDaemonState = useCallback(async () => {
		setConnection(connection, "loading");
		try {
			const nextConnection = await invoke<{
				connected: boolean;
				state: {
					status: "ready";
					daemon_version: string;
					protocol_version: number;
				} | null;
				error: string | null;
			}>("get_daemon_state");
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
	}, [connection, setConnection]);

	const loadSession = useCallback(
		async (targetSessionId: string) => {
			try {
				const session = await invoke<SessionView>("load_session", {
					sessionId: targetSessionId,
				});
				loadSessionAction(session);
			} catch {
				// Session may not exist yet; leave the chat empty.
			}
		},
		[loadSessionAction],
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
		void refreshDaemonState().then(() => {
			const defaultSessionId = "default";
			setSessionId(defaultSessionId);
			void loadSession(defaultSessionId);
		});
	}, [refreshDaemonState, loadSession]);

	const handleSend = useCallback(async () => {
		const trimmed = input.trim();
		if (!trimmed || runState.isSending) return;

		sendMessage(`user-${Date.now()}`, trimmed);
		setInput("");

		try {
			await invoke("send_message", { sessionId, message: trimmed });
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
	}, [input, runState.isSending, sessionId, sendMessage, setConnection]);

	const handleNewChat = useCallback(async () => {
		const newSessionId = `session-${Date.now()}`;
		try {
			await invoke("new_session", { sessionId: newSessionId });
			setSessionId(newSessionId);
			resetSession();
			setInput("");
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
	}, [resetSession, setConnection]);

	const groupedEvents = useMemo(() => groupEvents(events), [events]);
	const isConnected = connection.connected;

	return (
		<main className="app-shell">
			<aside className="left-sidebar" aria-label="主导航">
				<div className="nav-tabs">
					<button type="button" className="nav-tab active">
						Chat
					</button>
					<button type="button" className="nav-tab" disabled title="即将上线">
						Work
					</button>
				</div>

				<nav className="nav-menu">
					<button type="button" className="nav-item" onClick={handleNewChat}>
						<span className="nav-item-icon" aria-hidden="true">
							<Plus size={16} strokeWidth={2} />
						</span>
						新对话
					</button>
					<button
						type="button"
						className={`nav-item ${rightPanel === "events" ? "active" : ""}`}
						onClick={() =>
							setRightPanel((current) =>
								current === "events" ? null : "events",
							)
						}
					>
						<span className="nav-item-icon" aria-hidden="true">
							<Zap size={16} strokeWidth={2} />
						</span>
						运行时
						{events.length > 0 && (
							<span className="nav-item-badge">{events.length}</span>
						)}
					</button>
					<button
						type="button"
						className={`nav-item ${rightPanel === "settings" ? "active" : ""}`}
						onClick={() =>
							setRightPanel((current) =>
								current === "settings" ? null : "settings",
							)
						}
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
						<p className="hero-subtitle">
							本地编码助手，对话即可生成、调试和理解代码。
						</p>
						<div className="input-card hero-input-card">
							<InputField
								input={input}
								setInput={setInput}
								handleSend={handleSend}
								runState={runState}
								isConnected={isConnected}
							/>
						</div>
					</div>
				) : (
					<div className="chat-view">
						<div className="chat-messages">
							{messages.map((message) => (
								<div
									key={message.id}
									className={`chat-message chat-message--${message.role}`}
								>
									<div className="chat-message__avatar" aria-hidden="true">
										{message.role === "developer" ? (
											<User size={16} strokeWidth={2} />
										) : (
											<Bot size={18} strokeWidth={2} />
										)}
									</div>
									<div className="chat-message__body">
										<div className="chat-message__content">
											{message.content ||
												(message.status === "streaming" ? "…" : "")}
											{message.status === "streaming" && (
												<span className="chat-cursor" aria-hidden="true" />
											)}
										</div>
										{message.status === "error" && (
											<div className="chat-message__error" role="alert">
												{message.error ?? "出错了"}
											</div>
										)}
									</div>
								</div>
							))}
						</div>

						<div className="input-card chat-input-card">
							<InputField
								input={input}
								setInput={setInput}
								handleSend={handleSend}
								runState={runState}
								isConnected={isConnected}
							/>
						</div>
					</div>
				)}
			</section>

			{rightPanel && (
				<aside className="right-drawer" aria-label="右侧面板">
					<div className="drawer-header">
						<h3>{rightPanel === "events" ? "运行时事件" : "设置"}</h3>
						<button
							type="button"
							className="drawer-close"
							onClick={() => setRightPanel(null)}
							aria-label="关闭"
						>
							<X size={18} strokeWidth={2} />
						</button>
					</div>

					{rightPanel === "events" ? (
						<div className="drawer-body">
							{groupedEvents.length > 0 ? (
								<ul className="event-list">
									{groupedEvents.map(({ event, count }) => (
										<li key={`${event.sequence}-${count}`}>
											<span className="event-time">
												{formatTime(event.receivedAt)}
												{count > 1 && (
													<span className="event-count">×{count}</span>
												)}
											</span>
											<span
												className={`event-type event-type--${eventTone(event.type)}`}
											>
												{event.type}
											</span>
											<span className="event-detail">{eventDetail(event)}</span>
										</li>
									))}
								</ul>
							) : (
								<p className="event-empty">暂无运行时事件。</p>
							)}
						</div>
					) : (
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
					)}
				</aside>
			)}
		</main>
	);
}

function InputField({
	input,
	setInput,
	handleSend,
	runState,
	isConnected,
}: {
	input: string;
	setInput: (value: string) => void;
	handleSend: () => Promise<void>;
	runState: ChatRunState;
	isConnected: boolean;
}) {
	return (
		<>
			<textarea
				className="input-card-textarea"
				placeholder="输入消息…"
				value={input}
				onChange={(event) => setInput(event.target.value)}
				onKeyDown={(event) => {
					if (event.key === "Enter" && !event.shiftKey) {
						event.preventDefault();
						void handleSend();
					}
				}}
				disabled={runState.isSending || !isConnected}
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
						disabled={runState.isSending || !isConnected || !input.trim()}
						aria-label="发送"
					>
						<ArrowUp size={18} strokeWidth={2.5} />
					</button>
				</div>
			</div>
		</>
	);
}

function formatTime(date: Date): string {
	const time = date.toLocaleTimeString();
	const ms = String(date.getMilliseconds()).padStart(3, "0");
	return `${time}.${ms}`;
}

function eventTone(type: RuntimeEvent["type"]): string {
	switch (type) {
		case "daemon_started":
			return "success";
		case "state_changed":
			return "muted";
		case "error":
			return "danger";
		case "run_started":
		case "run_finished":
			return "info";
		case "message_started":
		case "message_completed":
			return "message";
		case "message_delta":
			return "delta";
	}
}

function eventDetail(event: RuntimeEvent): string {
	switch (event.type) {
		case "daemon_started":
			return `守护进程 ${event.state.daemon_version} 已启动`;
		case "state_changed":
			return `状态为 ${event.state.status}`;
		case "error":
			return event.message;
		case "run_started":
			return `运行 ${shortId(event.run_id)} 已启动`;
		case "run_finished":
			return `运行 ${shortId(event.run_id)} ${event.status === "succeeded" ? "成功" : "失败"}`;
		case "message_started":
			return `${event.role === "developer" ? "开发者" : "助手"} 消息 ${shortId(event.message_id)}`;
		case "message_delta":
			return event.delta.length > 28
				? `${event.delta.slice(0, 28)}…`
				: event.delta;
		case "message_completed":
			return `消息 ${shortId(event.message_id)} 已完成`;
	}
}

function shortId(id: string): string {
	const parts = id.split("-");
	return parts.length >= 2
		? `${parts[0]}…${parts[parts.length - 1].slice(-4)}`
		: id.slice(0, 8);
}

export default App;
