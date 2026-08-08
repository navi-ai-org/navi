<script lang="ts">
  import {
    activeSession,
    messages,
    isStreaming,
    pendingApproval,
    pendingQuestion,
    wsStatus,
    error,
    makeMessage,
    clearChat,
  } from "../lib/stores";
  import { sendTurn, cancelTurn, approveTool, answerQuestion } from "../lib/api";
  import { EventStream } from "../lib/ws";
  import type { RuntimeEvent, ToolCallInfo } from "../lib/types";
  import ApprovalCard from "./ApprovalCard.svelte";
  import QuestionCard from "./QuestionCard.svelte";
  import Markdown from "./Markdown.svelte";

  let inputText = $state("");
  let scrollContainer: HTMLDivElement | null = $state(null);
  let eventStream: EventStream | null = null;

  // ── Event handling ──────────────────────────────────────────────────────
  //
  // RuntimeEventKind is an externally-tagged Rust enum:
  //   { "AssistantDelta": { "text": "..." } }
  // The variant name is the single key of the `kind` object.

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
        // Could be shown in a collapsible "thinking" section.
        // For now, ignored — not critical for MVP.
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
    }
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
        if (finalText) {
          last.text = finalText;
        }
        last.streaming = false;
        return [...msgs];
      }
      if (finalText) {
        return [...msgs, makeMessage("assistant", finalText)];
      }
      return msgs;
    });
  }

  function addToolCall(name: string, status: ToolCallInfo["status"]) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant") {
        const calls = last.toolCalls ?? [];
        const existing = calls.find((c) => c.name === name);
        if (existing) {
          existing.status = status;
        } else {
          calls.push({ name, status });
        }
        last.toolCalls = [...calls];
        return [...msgs];
      }
      return msgs;
    });
  }

  function scrollToBottom() {
    queueMicrotask(() => {
      if (scrollContainer) {
        scrollContainer.scrollTop = scrollContainer.scrollHeight;
      }
    });
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

  // Auto-scroll when messages change.
  $effect(() => {
    $messages;
    scrollToBottom();
  });
</script>

<div class="chat-view">
  <!-- Messages -->
  <div class="messages" bind:this={scrollContainer}>
    {#each $messages as msg (msg.id)}
      <div class="message" class:user={msg.role === "user"} class:assistant={msg.role === "assistant"} class:system={msg.role === "system"}>
        <div class="bubble">
          {#if msg.role === "assistant" && msg.toolName}
            <!-- Tool result message -->
            <div class="tool-result">
              <span class="tool-icon" class:ok={msg.toolResult?.ok} class:fail={!msg.toolResult?.ok}>
                {msg.toolResult?.ok ? "✓" : "✗"}
              </span>
              <span class="tool-label text-mono text-sm">{msg.toolName}</span>
            </div>
          {:else if msg.role === "assistant"}
            <!-- Assistant message with markdown -->
            <Markdown content={msg.text} />
            {#if msg.streaming}<span class="cursor">▋</span>{/if}
          {:else}
            <!-- User / system message -->
            <p>{msg.text}</p>
          {/if}
          {#if msg.toolCalls && msg.toolCalls.length > 0}
            <div class="tool-calls">
              {#each msg.toolCalls as tc}
                <span class="tool-call" class:done={tc.status === "completed"} class:failed={tc.status === "failed"}>
                  {tc.name}: {tc.status}
                </span>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/each}

    {#if $messages.length === 0}
      <div class="empty-chat">
        <p class="text-muted">Send a message to start the conversation.</p>
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
    <QuestionCard
      question={$pendingQuestion}
      onAnswer={handleAnswer}
    />
  {/if}

  <!-- Error banner -->
  {#if $error}
    <div class="error-banner">
      <span>{$error}</span>
      <button onclick={() => error.set(null)}>×</button>
    </div>
  {/if}

  <!-- Input -->
  <form class="input-bar" onsubmit={handleSend}>
    <textarea
      bind:value={inputText}
      placeholder="Type a message..."
      rows="1"
      onkeydown={(e) => {
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          handleSend();
        }
      }}
    ></textarea>
    {#if $isStreaming}
      <button type="button" class="btn-danger" onclick={handleCancel}>
        Cancel
      </button>
    {:else}
      <button type="submit" class="btn-primary" disabled={!inputText.trim()}>
        Send
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
    gap: 0.75rem;
  }

  .message {
    display: flex;
    max-width: 85%;
  }

  .message.user {
    align-self: flex-end;
  }

  .message.assistant {
    align-self: flex-start;
  }

  .message.system {
    align-self: center;
    max-width: 90%;
  }

  .bubble {
    padding: 0.6rem 0.9rem;
    border-radius: var(--radius);
    word-wrap: break-word;
    overflow-wrap: break-word;
    word-break: break-word;
  }

  .message.user .bubble {
    background: var(--accent);
    color: #fff;
    border-bottom-right-radius: 2px;
  }

  .message.assistant .bubble {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-bottom-left-radius: 2px;
  }

  .message.system .bubble {
    background: rgba(248, 81, 73, 0.1);
    border: 1px solid var(--danger);
    color: var(--danger);
    font-size: 0.85rem;
  }

  .bubble p {
    white-space: pre-wrap;
  }

  .cursor {
    animation: blink 1s infinite;
  }

  @keyframes blink {
    0%, 50% { opacity: 1; }
    51%, 100% { opacity: 0; }
  }

  .tool-result {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .tool-icon {
    font-size: 0.9rem;
    font-weight: bold;
  }

  .tool-icon.ok {
    color: var(--success);
  }

  .tool-icon.fail {
    color: var(--danger);
  }

  .tool-label {
    color: var(--accent);
  }

  .tool-calls {
    margin-top: 0.5rem;
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
  }

  .tool-call {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    padding: 0.15rem 0.4rem;
    background: var(--bg-tertiary);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
  }

  .tool-call.done {
    color: var(--success);
  }

  .tool-call.failed {
    color: var(--danger);
  }

  .empty-chat {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    text-align: center;
  }

  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem 0.75rem;
    background: rgba(248, 81, 73, 0.15);
    border-top: 1px solid var(--danger);
    color: var(--danger);
    font-size: 0.85rem;
  }

  .error-banner button {
    background: transparent;
    border: none;
    color: var(--danger);
    font-size: 1.2rem;
    padding: 0 0.3rem;
  }

  .input-bar {
    display: flex;
    gap: 0.5rem;
    padding: 0.75rem;
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
  }

  .input-bar textarea {
    flex: 1;
    max-height: 120px;
    min-height: 40px;
  }

  .input-bar button {
    padding: 0.5rem 1.2rem;
    align-self: flex-end;
  }
</style>
