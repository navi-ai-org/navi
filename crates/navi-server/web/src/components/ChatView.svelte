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
  import type { RuntimeEvent, ChatMessage, ToolCallInfo } from "../lib/types";
  import ApprovalCard from "./ApprovalCard.svelte";
  import QuestionCard from "./QuestionCard.svelte";

  let inputText = $state("");
  let scrollContainer: HTMLDivElement | null = $state(null);
  let eventStream: EventStream | null = null;

  // ── Event handling ──────────────────────────────────────────────────────

  function handleEvent(event: RuntimeEvent) {
    const k = event.kind as { kind: string; [key: string]: unknown };
    switch (k.kind) {
      case "AssistantDelta": {
        const text = (k as { text: string }).text;
        appendToLastAssistant(text);
        break;
      }
      case "AssistantThinkingDelta": {
        // Thinking deltas are shown as a system message (optional).
        break;
      }
      case "ToolStarted": {
        const toolName = (k as { tool_name: string }).tool_name;
        addToolCall(toolName, "started");
        break;
      }
      case "ToolCompleted": {
        const toolName = (k as { tool_name: string }).tool_name;
        const success = (k as { success: boolean }).success;
        addToolCall(toolName, success ? "completed" : "failed");
        break;
      }
      case "ApprovalRequired": {
        pendingApproval.set({
          requestId: (k as { request_id: string }).request_id,
          toolName: (k as { tool_name: string }).tool_name,
          description: (k as { description: string }).description,
        });
        break;
      }
      case "QuestionRequired": {
        pendingQuestion.set({
          questionId: (k as { question_id: string }).question_id,
          question: (k as { question: string }).question,
          options: (k as { options?: string[] }).options,
        });
        break;
      }
      case "TurnCompleted": {
        const text = (k as { text: string }).text;
        finalizeLastAssistant(text);
        isStreaming.set(false);
        break;
      }
      case "Error": {
        error.set((k as { message: string }).message);
        isStreaming.set(false);
        break;
      }
      case "SessionTitleUpdated": {
        const title = (k as { title: string }).title;
        activeSession.update((s) =>
          s ? { ...s, title } : s,
        );
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
      // Create new assistant message
      return [...msgs, makeMessage("assistant", text, true)];
    });
    scrollToBottom();
  }

  function finalizeLastAssistant(finalText: string) {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last && last.role === "assistant" && last.streaming) {
        // Replace with final text if provided and non-empty.
        if (finalText) {
          last.text = finalText;
        }
        last.streaming = false;
        return [...msgs];
      }
      // If no streaming message was created, add the final text.
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

    // Disconnect previous stream
    if (eventStream) {
      eventStream.disconnect();
      eventStream = null;
    }

    // Connect to new session's event stream
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

    // Add user message to UI immediately
    messages.update((m) => [...m, makeMessage("user", text)]);
    inputText = "";
    isStreaming.set(true);
    error.set(null);

    try {
      // The turn runs detached on the server. Events stream via WebSocket.
      // The HTTP response is just a confirmation.
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
      <div class="message" class:user={msg.role === "user"} class:assistant={msg.role === "assistant"}>
        <div class="bubble">
          <p>{msg.text}{#if msg.streaming}<span class="cursor">▋</span>{/if}</p>
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
