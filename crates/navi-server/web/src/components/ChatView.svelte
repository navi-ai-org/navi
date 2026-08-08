<script lang="ts">
  import {
    activeSession,
    messages,
    isStreaming,
    pendingApproval,
    pendingQuestion,
    pendingPlanReview,
    pendingSudo,
    wsStatus,
    error,
    tokenUsage,
    compactNotification,
    agentMode,
    makeMessage,
  } from "../lib/stores";
  import {
    sendTurn,
    cancelTurn,
    approveTool,
    answerQuestion,
    submitPlanReview,
    submitSudoPassword,
    cancelSudoPrompt,
  } from "../lib/api";
  import { EventStream } from "../lib/ws";
  import type { RuntimeEvent, ToolCallInfo } from "../lib/types";
  import ApprovalCard from "./ApprovalCard.svelte";
  import QuestionCard from "./QuestionCard.svelte";
  import PlanReviewCard from "./PlanReviewCard.svelte";
  import SudoPromptCard from "./SudoPromptCard.svelte";
  import CompactNotification from "./CompactNotification.svelte";
  import Markdown from "./Markdown.svelte";

  let inputText = $state("");
  let scrollContainer: HTMLDivElement | null = $state(null);
  let textareaEl: HTMLTextAreaElement | null = $state(null);
  let eventStream: EventStream | null = null;

  // ── Event handling ──────────────────────────────────────────────────────

  function handleEvent(event: RuntimeEvent) {
    const kind = event.kind;
    const keys = Object.keys(kind);
    if (keys.length !== 1) return;
    const variant = keys[0];
    const payload = (kind as Record<string, Record<string, unknown>>)[variant];

    switch (variant) {
      case "AssistantDelta": {
        const text = (payload?.text as string) ?? "";
        appendToLastAssistant(text);
        break;
      }
      case "AssistantThinkingDelta": {
        const text = (payload?.text as string) ?? "";
        appendToLastAssistantThinking(text);
        break;
      }
      case "ToolStarted": {
        const toolName = (payload as { tool_name: string })?.tool_name ?? "tool";
        addToolCall(toolName, "started");
        break;
      }
      case "ToolCompleted": {
        const toolName = (payload as { tool_name?: string })?.tool_name ?? "tool";
        const ok = (payload as { ok?: boolean })?.ok ?? true;
        addToolCall(toolName, ok ? "completed" : "failed");
        break;
      }
      case "ApprovalRequired": {
        const req = payload as {
          request_id: string;
          tool_name: string;
          description: string;
        };
        pendingApproval.set({
          requestId: req.request_id,
          toolName: req.tool_name,
          description: req.description,
        });
        break;
      }
      case "ApprovalResolved": {
        pendingApproval.set(null);
        break;
      }
      case "QuestionRequired": {
        const q = payload as {
          question_id: string;
          question: string;
          options?: string[];
        };
        pendingQuestion.set({
          questionId: q.question_id,
          question: q.question,
          options: q.options,
        });
        break;
      }
      case "QuestionResolved": {
        pendingQuestion.set(null);
        break;
      }
      case "PlanReviewRequired": {
        const req = payload as {
          id: string;
          plan_id: string;
          title: string;
          description: string;
          steps: string[];
          body_markdown?: string;
        };
        pendingPlanReview.set({
          id: req.id,
          planId: req.plan_id,
          title: req.title,
          description: req.description,
          steps: req.steps,
          bodyMarkdown: req.body_markdown,
        });
        break;
      }
      case "PlanReviewResolved": {
        pendingPlanReview.set(null);
        break;
      }
      case "SudoPasswordRequired": {
        const req = payload as {
          id: string;
          command_summary: string;
        };
        pendingSudo.set({
          id: req.id,
          commandSummary: req.command_summary,
        });
        break;
      }
      case "TokensUpdated": {
        const t = payload as {
          input_tokens: number;
          output_tokens: number;
        };
        tokenUsage.set({
          inputTokens: t.input_tokens,
          outputTokens: t.output_tokens,
        });
        break;
      }
      case "TurnCompleted": {
        const text = (payload as { text: string })?.text ?? "";
        finalizeLastAssistant(text);
        isStreaming.set(false);
        break;
      }
      case "Error": {
        const message = (payload as { message: string })?.message ?? "Unknown error";
        error.set(message);
        isStreaming.set(false);
        break;
      }
      case "SessionTitleUpdated": {
        const title = (payload as { title: string })?.title;
        if (title) {
          activeSession.update((s) => (s ? { ...s, title } : s));
        }
        break;
      }
      case "SessionFinished": {
        isStreaming.set(false);
        break;
      }
      case "AutoCompactStarted": {
        compactNotification.set({ type: "started" });
        break;
      }
      case "AutoCompactCompleted": {
        const c = payload as {
          tokens_saved: number;
          summary: string;
        };
        compactNotification.set({
          type: "completed",
          tokensSaved: c.tokens_saved,
          summary: c.summary,
        });
        break;
      }
      case "AutoCompactFailed": {
        const r = payload as { reason: string };
        compactNotification.set({ type: "failed", reason: r.reason });
        break;
      }
      case "PlanProposed": {
        // Plan proposed in plan mode — show as system message
        const title = (payload as { title: string })?.title ?? "Plan";
        const steps = (payload as { steps: string[] })?.steps ?? [];
        const planText = `**Plan: ${title}**\n\n${steps.map((s, i) => `${i + 1}. ${s}`).join("\n")}`;
        messages.update((m) => [
          ...m,
          makeMessage("assistant", planText),
        ]);
        break;
      }
      case "AgentModeChanged": {
        const mode = (payload as { mode: string })?.mode ?? "default";
        agentMode.set(mode as "default" | "plan");
        break;
      }
    }
  }

  function appendToLastAssistantThinking(text: string) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant" && last.streaming) {
        last.thinking = (last.thinking ?? "") + text;
        return [...msgs];
      }
      // Create a new streaming assistant message with thinking
      const msg = makeMessage("assistant", "", true);
      msg.thinking = text;
      return [...msgs, msg];
    });
  }

  function appendToLastAssistant(text: string) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant" && last.streaming) {
        last.text += text;
        return [...msgs];
      }
      return [...msgs, makeMessage("assistant", text, true)];
    });
    scrollToBottom();
  }

  function finalizeLastAssistant(finalText: string) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant" && last.streaming) {
        if (finalText) last.text = finalText;
        last.streaming = false;
        return [...msgs];
      }
      if (finalText) return [...msgs, makeMessage("assistant", finalText)];
      return msgs;
    });
  }

  function addToolCall(name: string, status: ToolCallInfo["status"]) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant") {
        const calls = last.toolCalls ?? [];
        const existing = calls.find((c) => c.name === name);
        if (existing) existing.status = status;
        else calls.push({ name, status });
        last.toolCalls = [...calls];
        return [...msgs];
      }
      return msgs;
    });
  }

  function scrollToBottom() {
    queueMicrotask(() => {
      if (scrollContainer) {
        scrollContainer.scrollTo({ top: scrollContainer.scrollHeight, behavior: "smooth" });
      }
    });
  }

  function autoResize() {
    if (textareaEl) {
      textareaEl.style.height = "auto";
      textareaEl.style.height = Math.min(textareaEl.scrollHeight, 140) + "px";
    }
  }

  function formatTime(ts: number): string {
    if (!ts) return "";
    try {
      return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    } catch {
      return "";
    }
  }

  // ── WebSocket lifecycle ─────────────────────────────────────────────────

  $effect(() => {
    const session = $activeSession;
    if (!session) return;

    if (eventStream) {
      eventStream.disconnect();
      eventStream = null;
    }

    eventStream = new EventStream(session.id);
    const unsubEvents = eventStream.onEvent(handleEvent);
    const unsubStatus = eventStream.onStatus((s) => wsStatus.set(s));
    eventStream.connect();

    return () => {
      unsubEvents();
      unsubStatus();
      eventStream?.disconnect();
      eventStream = null;
    };
  });

  // ── Send message ────────────────────────────────────────────────────────

  async function handleSend(e?: Event) {
    e?.preventDefault();
    const text = inputText.trim();
    if (!text || $isStreaming || !$activeSession) return;

    messages.update((m) => [...m, makeMessage("user", text)]);
    inputText = "";
    if (textareaEl) textareaEl.style.height = "auto";
    isStreaming.set(true);
    error.set(null);

    try {
      await sendTurn($activeSession.id, text);
    } catch (err) {
      isStreaming.set(false);
      error.set(err instanceof Error ? err.message : "Failed to send message");
    }
  }

  async function handleCancel() {
    if (!$activeSession) return;
    try {
      await cancelTurn($activeSession.id);
      isStreaming.set(false);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Failed to cancel");
    }
  }

  // ── Approval ────────────────────────────────────────────────────────────

  async function handleApprove(approved: boolean) {
    if (!$activeSession || !$pendingApproval) return;
    const req = $pendingApproval;
    pendingApproval.set(null);
    try {
      await approveTool($activeSession.id, req.requestId, approved);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Approval failed");
    }
  }

  // ── Question ────────────────────────────────────────────────────────────

  async function handleAnswer(answer: string) {
    if (!$activeSession || !$pendingQuestion) return;
    const q = $pendingQuestion;
    pendingQuestion.set(null);
    try {
      await answerQuestion($activeSession.id, q.questionId, answer);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Answer failed");
    }
  }

  // ── Plan review ─────────────────────────────────────────────────────────

  async function handlePlanReview(
    decision: "approve" | "request_changes" | "quit",
    freeform?: string,
  ) {
    if (!$activeSession || !$pendingPlanReview) return;
    const p = $pendingPlanReview;
    pendingPlanReview.set(null);
    try {
      await submitPlanReview(
        $activeSession.id,
        p.id,
        p.planId,
        decision,
        freeform,
      );
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Plan review failed");
    }
  }

  // ── Sudo password ───────────────────────────────────────────────────────

  async function handleSudoSubmit(password: string) {
    if (!$activeSession || !$pendingSudo) return;
    const s = $pendingSudo;
    pendingSudo.set(null);
    try {
      await submitSudoPassword($activeSession.id, s.id, password);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Sudo submit failed");
    }
  }

  async function handleSudoCancel() {
    if (!$activeSession || !$pendingSudo) return;
    const s = $pendingSudo;
    pendingSudo.set(null);
    try {
      await cancelSudoPrompt($activeSession.id, s.id);
    } catch (err) {
      error.set(err instanceof Error ? err.message : "Sudo cancel failed");
    }
  }

  $effect(() => {
    $messages;
    scrollToBottom();
  });
</script>

<div class="chat-view">
  <!-- Messages -->
  <div class="messages" bind:this={scrollContainer}>
    {#each $messages as msg (msg.id)}
      <div class="message-row fade-in-up" class:user={msg.role === "user"} class:assistant={msg.role === "assistant"} class:system={msg.role === "system"}>
        {#if msg.role === "assistant"}
          <div class="avatar assistant-avatar">
            <svg width="16" height="16" viewBox="0 0 32 32">
              <rect width="32" height="32" rx="7" fill="var(--accent)" />
              <text x="16" y="22" font-family="system-ui, sans-serif" font-size="18" font-weight="bold" fill="#fff" text-anchor="middle">N</text>
            </svg>
          </div>
        {/if}

        <div class="message-content">
          {#if msg.role === "assistant" && msg.toolName}
            <!-- Tool result -->
            <div class="tool-result">
              <span class="tool-icon" class:ok={msg.toolResult?.ok} class:fail={!msg.toolResult?.ok}>
                {#if msg.toolResult?.ok}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><polyline points="20 6 9 17 4 12"/></svg>
                {:else}
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                {/if}
              </span>
              <span class="tool-label text-mono text-sm">{msg.toolName}</span>
            </div>
          {:else if msg.role === "assistant"}
            {#if msg.thinking}
              <details class="thinking-block">
                <summary class="thinking-summary">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.5.8a.6.6 0 01-.997.057L12 12.5l-1.06 1.743a.6.6 0 01-.998-.057l-.5-.8z"/></svg>
                  <span>Thinking</span>
                </summary>
                <div class="thinking-content text-sm text-muted">{msg.thinking}</div>
              </details>
            {/if}
            <div class="bubble assistant-bubble">
              {#if msg.text}
                <Markdown content={msg.text} />
              {/if}
              {#if msg.streaming && !msg.text}
                <span class="typing-cursor"></span>
              {/if}
            </div>
          {:else if msg.role === "user"}
            <div class="bubble user-bubble">
              <p>{msg.text}</p>
            </div>
          {:else}
            <div class="bubble system-bubble">
              <p>{msg.text}</p>
            </div>
          {/if}

          {#if msg.toolCalls && msg.toolCalls.length > 0}
            <div class="tool-calls">
              {#each msg.toolCalls as tc}
                <span class="tool-call" class:done={tc.status === "completed"} class:failed={tc.status === "failed"} class:active={tc.status === "started" || tc.status === "requested"}>
                  {#if tc.status === "started" || tc.status === "requested"}
                    <span class="spinner spinner-tiny"></span>
                  {/if}
                  {tc.name}
                </span>
              {/each}
            </div>
          {/if}

          <span class="msg-time text-xs text-faint">{formatTime(msg.timestamp)}</span>
        </div>

        {#if msg.role === "user"}
          <div class="avatar user-avatar">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/>
              <circle cx="12" cy="7" r="4"/>
            </svg>
          </div>
        {/if}
      </div>
    {/each}

    <!-- Typing indicator when streaming but no message yet -->
    {#if $isStreaming && $messages[$messages.length - 1]?.role !== "assistant"}
      <div class="message-row assistant fade-in">
        <div class="avatar assistant-avatar">
          <svg width="16" height="16" viewBox="0 0 32 32">
            <rect width="32" height="32" rx="7" fill="var(--accent)" />
            <text x="16" y="22" font-family="system-ui, sans-serif" font-size="18" font-weight="bold" fill="#fff" text-anchor="middle">N</text>
          </svg>
        </div>
        <div class="message-content">
          <div class="bubble assistant-bubble typing-bubble">
            <span class="typing-dot"></span>
            <span class="typing-dot"></span>
            <span class="typing-dot"></span>
          </div>
        </div>
      </div>
    {/if}

    {#if $messages.length === 0 && !$isStreaming}
      <div class="empty-chat">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" stroke-width="1.5">
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
        </svg>
        <p class="text-muted">Start a conversation</p>
        <p class="text-faint text-sm">Type a message below</p>
      </div>
    {/if}
  </div>

  <!-- Pending approval / question -->
  {#if $pendingApproval}
    <ApprovalCard
      approval={$pendingApproval}
      onApprove={() => handleApprove(true)}
      onDeny={() => handleApprove(false)}
    />
  {/if}

  {#if $pendingQuestion}
    <QuestionCard question={$pendingQuestion} onAnswer={handleAnswer} />
  {/if}

  {#if $pendingPlanReview}
    <PlanReviewCard
      review={$pendingPlanReview}
      onApprove={() => handlePlanReview("approve")}
      onRequestChanges={(freeform) => handlePlanReview("request_changes", freeform)}
      onQuit={() => handlePlanReview("quit")}
    />
  {/if}

  {#if $pendingSudo}
    <SudoPromptCard
      prompt={$pendingSudo}
      onSubmit={handleSudoSubmit}
      onCancel={handleSudoCancel}
    />
  {/if}

  {#if $compactNotification}
    <CompactNotification
      notification={$compactNotification}
      onDismiss={() => compactNotification.set(null)}
    />
  {/if}

  <!-- Error banner -->
  {#if $error}
    <div class="error-banner fade-in-up">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      <span>{$error}</span>
      <button onclick={() => error.set(null)} aria-label="Dismiss error">×</button>
    </div>
  {/if}

  <!-- Input -->
  <form class="input-bar" onsubmit={handleSend}>
    <textarea
      bind:this={textareaEl}
      bind:value={inputText}
      oninput={autoResize}
      placeholder="Message NAVI..."
      rows="1"
      onkeydown={(e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          handleSend();
        }
      }}
    ></textarea>
    {#if $isStreaming}
      <button type="button" class="btn-danger send-btn" onclick={handleCancel} aria-label="Cancel">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="6" y="6" width="12" height="12" rx="2"/>
        </svg>
      </button>
    {:else}
      <button type="submit" class="btn-primary send-btn" disabled={!inputText.trim()} aria-label="Send">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <line x1="22" y1="2" x2="11" y2="13"/>
          <polygon points="22 2 15 22 11 13 2 9 22 2"/>
        </svg>
      </button>
    {/if}
  </form>
</div>

<style>
  .chat-view {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }

  .message-row {
    display: flex;
    gap: 0.6rem;
    max-width: 85%;
    align-items: flex-start;
  }

  .message-row.user {
    align-self: flex-end;
    flex-direction: row-reverse;
  }

  .message-row.assistant {
    align-self: flex-start;
  }

  .message-row.system {
    align-self: center;
    max-width: 90%;
  }

  .avatar {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    overflow: hidden;
  }

  .assistant-avatar {
    background: var(--bg-tertiary);
  }

  .user-avatar {
    background: var(--bg-tertiary);
    color: var(--text-muted);
  }

  .message-content {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }

  .message-row.user .message-content {
    align-items: flex-end;
  }

  .bubble {
    padding: 0.65rem 0.9rem;
    border-radius: var(--radius);
    word-wrap: break-word;
    overflow-wrap: break-word;
    word-break: break-word;
  }

  .user-bubble {
    background: var(--accent);
    color: #fff;
    border-bottom-right-radius: var(--radius-xs);
  }

  .user-bubble p {
    white-space: pre-wrap;
  }

  .assistant-bubble {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-bottom-left-radius: var(--radius-xs);
  }

  /* Thinking block */
  .thinking-block {
    margin-bottom: 0.3rem;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    background: var(--bg);
    overflow: hidden;
  }

  .thinking-summary {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.4rem 0.7rem;
    cursor: pointer;
    font-size: 0.78rem;
    color: var(--text-muted);
    user-select: none;
    list-style: none;
  }

  .thinking-summary::-webkit-details-marker {
    display: none;
  }

  .thinking-summary::before {
    content: "▸";
    font-size: 0.7rem;
    transition: transform var(--transition);
  }

  .thinking-block[open] .thinking-summary::before {
    transform: rotate(90deg);
  }

  .thinking-content {
    padding: 0.5rem 0.7rem;
    border-top: 1px solid var(--border-subtle);
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    line-height: 1.5;
    max-height: 200px;
    overflow-y: auto;
  }

  .system-bubble {
    background: var(--danger-subtle);
    border: 1px solid var(--danger);
    color: var(--danger);
    font-size: 0.85rem;
  }

  .msg-time {
    padding: 0 0.2rem;
  }

  /* Typing indicator */
  .typing-bubble {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 0.7rem 0.9rem;
  }

  .typing-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
    animation: typingBounce 1.4s infinite ease-in-out;
  }

  .typing-dot:nth-child(1) {
    animation-delay: -0.32s;
  }
  .typing-dot:nth-child(2) {
    animation-delay: -0.16s;
  }

  @keyframes typingBounce {
    0%, 80%, 100% {
      transform: scale(0.6);
      opacity: 0.4;
    }
    40% {
      transform: scale(1);
      opacity: 1;
    }
  }

  .typing-cursor {
    display: inline-block;
    width: 2px;
    height: 1em;
    background: var(--accent);
    margin-left: 2px;
    vertical-align: text-bottom;
    animation: blink 1s infinite;
  }

  /* Tool results */
  .tool-result {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.7rem;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    border-left: 3px solid;
    border-left-color: var(--text-muted);
  }

  .tool-result .tool-icon.ok {
    color: var(--success);
  }

  .tool-result .tool-icon.fail {
    color: var(--danger);
  }

  .tool-result:has(.tool-icon.ok) {
    border-left-color: var(--success);
  }

  .tool-result:has(.tool-icon.fail) {
    border-left-color: var(--danger);
  }

  .tool-label {
    color: var(--accent);
  }

  .tool-calls {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin-top: 0.3rem;
  }

  .tool-call {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-family: var(--font-mono);
    font-size: 0.72rem;
    padding: 0.15rem 0.5rem;
    background: var(--bg-tertiary);
    border-radius: 12px;
    color: var(--text-muted);
  }

  .tool-call.done {
    color: var(--success);
  }

  .tool-call.failed {
    color: var(--danger);
  }

  .tool-call.active {
    color: var(--accent);
  }

  .spinner-tiny {
    width: 10px;
    height: 10px;
    border-width: 1.5px;
  }

  .empty-chat {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    gap: 0.3rem;
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 0.8rem;
    background: var(--danger-subtle);
    border-top: 1px solid var(--danger);
    color: var(--danger);
    font-size: 0.85rem;
  }

  .error-banner span {
    flex: 1;
  }

  .error-banner button {
    background: transparent;
    border: none;
    color: var(--danger);
    font-size: 1.3rem;
    padding: 0 0.3rem;
    line-height: 1;
  }

  .input-bar {
    display: flex;
    gap: 0.5rem;
    padding: 0.75rem;
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
    align-items: flex-end;
  }

  .input-bar textarea {
    flex: 1;
    max-height: 140px;
    min-height: 42px;
    line-height: 1.4;
  }

  .send-btn {
    padding: 0.6rem;
    width: 42px;
    height: 42px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
</style>
