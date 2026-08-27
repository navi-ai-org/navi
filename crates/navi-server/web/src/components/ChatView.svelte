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
    startSession,
  } from "../lib/api";
  import { EventStream } from "../lib/ws";
  import type { RuntimeEvent, ToolCallInfo } from "../lib/types";
  import ApprovalCard from "./ApprovalCard.svelte";
  import QuestionCard from "./QuestionCard.svelte";
  import PlanReviewCard from "./PlanReviewCard.svelte";
  import SudoPromptCard from "./SudoPromptCard.svelte";
  import CompactNotification from "./CompactNotification.svelte";
  import Markdown from "./Markdown.svelte";
  import ModelSelector from "./ModelSelector.svelte";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import ToolCallRow from "./ToolCallRow.svelte";

  let inputText = $state("");
  let scrollContainer: HTMLDivElement | null = $state(null);
  let textareaEl: HTMLTextAreaElement | null = $state(null);
  let eventStream: EventStream | null = null;
  let showScrollBottom = $state(false);
  let toolNames = new Map<string, string>();

  function handleScroll() {
    if (!scrollContainer) return;
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    showScrollBottom = scrollHeight - scrollTop - clientHeight > 100;
  }

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
      case "ToolRequested": {
        closeLastThinkingForTool();
        const invocation = payload as { id: string; tool_name: string; input: unknown };
        toolNames.set(invocation.id, invocation.tool_name);
        upsertToolCall({
          id: invocation.id,
          name: invocation.tool_name,
          status: "requested",
          input: invocation.input,
        });
        break;
      }
      case "ToolStarted": {
        const invocation = payload as { id: string; tool_name: string; input: unknown };
        toolNames.set(invocation.id, invocation.tool_name);
        upsertToolCall({
          id: invocation.id,
          name: invocation.tool_name,
          status: "started",
          input: invocation.input,
        });
        break;
      }
      case "ToolCompleted": {
        const result = payload as { invocation_id: string; ok: boolean; output: unknown };
        upsertToolCall({
          id: result.invocation_id,
          name: toolNames.get(result.invocation_id) ?? "tool",
          status: result.ok ? "completed" : "failed",
          output: result.output,
        });
        break;
      }
      case "ApprovalRequired": {
        const req = payload as {
          id: string;
          summary: string;
          risk: string;
        };
        pendingApproval.set({
          requestId: req.id,
          toolName: toolNames.get(req.id) ?? "Ação protegida",
          description: req.summary,
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
        settleToolCalls("completed");
        finalizeLastAssistant(text);
        isStreaming.set(false);
        break;
      }
      case "SessionSaved":
      case "SessionFinished": {
        settleToolCalls("completed");
        isStreaming.set(false);
        break;
      }
      case "Error": {
        const message = (payload as { message: string })?.message ?? "Unknown error";
        settleToolCalls("failed");
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

  function closeLastThinkingForTool() {
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last?.role === "assistant" && last.thinking && last.streaming) {
        last.streaming = false;
        return [...msgs];
      }
      return msgs;
    });
  }

  function settleToolCalls(status: "completed" | "failed") {
    messages.update((msgs) => {
      let changed = false;
      const settled = msgs.map((message) => {
        if (!message.toolCalls?.length) return message;
        const calls = message.toolCalls.map((call) => {
          if (call.status !== "requested" && call.status !== "started") return call;
          changed = true;
          return { ...call, status };
        });
        return changed ? { ...message, toolCalls: calls } : message;
      });
      return changed ? settled : msgs;
    });
  }

  function completeTurnFromResponse(finalText: string) {
    settleToolCalls("completed");
    messages.update((msgs) => {
      const last = msgs[msgs.length - 1];
      if (last?.role === "assistant" && last.streaming) {
        if (finalText) last.text = finalText;
        last.streaming = false;
        return [...msgs];
      }
      if (!finalText) return msgs;
      const duplicate = [...msgs].reverse().find(
        (message) => message.role === "assistant" && message.text === finalText,
      );
      return duplicate ? msgs : [...msgs, makeMessage("assistant", finalText)];
    });
    isStreaming.set(false);
  }

  function upsertToolCall(call: ToolCallInfo) {
    messages.update((msgs) => {
      for (const message of msgs) {
        const existing = message.toolCalls?.find((item) => item.id === call.id);
        if (existing) {
          Object.assign(existing, call);
          message.toolCalls = [...message.toolCalls!];
          return [...msgs];
        }
      }

      const toolMessage = makeMessage("assistant", "");
      toolMessage.toolCalls = [call];
      return [...msgs, toolMessage];
    });
    scrollToBottom();
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
      textareaEl.style.height = Math.min(textareaEl.scrollHeight, 160) + "px";
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

    toolNames = new Map();
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

  $effect(() => {
    if (
      $activeSession
      && !$isStreaming
      && !$pendingApproval
      && !$pendingQuestion
      && !$pendingPlanReview
      && !$pendingSudo
    ) {
      settleToolCalls("completed");
    }
  });

  // ── Send message ────────────────────────────────────────────────────────

  async function handleSend(e?: Event) {
    e?.preventDefault();
    const text = inputText.trim();
    if (!text || $isStreaming) return;

    let session = $activeSession;
    if (!session) {
      isStreaming.set(true);
      error.set(null);
      try {
        session = await startSession();
        activeSession.set(session);
      } catch (err) {
        isStreaming.set(false);
        error.set(err instanceof Error ? err.message : "Failed to create session");
        return;
      }
    }

    messages.update((m) => [...m, makeMessage("user", text)]);
    inputText = "";
    if (textareaEl) textareaEl.style.height = "auto";
    isStreaming.set(true);
    error.set(null);

    try {
      const response = await sendTurn(session.id, text);
      completeTurnFromResponse(response.text ?? "");
    } catch (err) {
      isStreaming.set(false);
      error.set(err instanceof Error ? err.message : "Failed to send message");
    }
  }

  async function handleCancel() {
    if (!$activeSession) return;
    try {
      await cancelTurn($activeSession.id);
      settleToolCalls("failed");
      messages.update((msgs) => {
        const last = msgs[msgs.length - 1];
        if (last?.streaming) {
          last.streaming = false;
          return [...msgs];
        }
        return msgs;
      });
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
  <!-- Messages container -->
  <div class="messages-container" bind:this={scrollContainer} onscroll={handleScroll}>
    <div class="messages-inner">
      {#each $messages as msg (msg.id)}
        <div class="message-row fade-in-up" class:user={msg.role === "user"} class:assistant={msg.role === "assistant"} class:system={msg.role === "system"} class:tool-message={msg.role === "assistant" && Boolean(msg.toolCalls?.length) && !msg.text && !msg.thinking}>
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
              {#if msg.thinking || msg.streaming}
                <ThinkingBlock text={msg.thinking ?? ""} streaming={Boolean(msg.streaming && !msg.text)} />
              {/if}
              {#if msg.text || msg.streaming}
                <div class="bubble assistant-bubble">
                  {#if msg.text}
                    <Markdown content={msg.text} />
                  {/if}
                  {#if msg.streaming && !msg.text}
                    <span class="typing-cursor"></span>
                  {/if}
                </div>
              {/if}
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
                {#each msg.toolCalls as tc (tc.id)}
                  <ToolCallRow call={tc} />
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {/each}

      <!-- Typing indicator when streaming -->
      {#if $isStreaming && $messages[$messages.length - 1]?.role !== "assistant"}
        <div class="message-row assistant fade-in">
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
        <div class="welcome-screen fade-in">
          <p class="welcome-kicker">NAVI</p>
          <h1 class="welcome-heading">Em que posso ajudar?</h1>
          <p class="welcome-copy">Descreva o que você quer fazer no projeto. Você pode começar por uma destas ideias.</p>

          <div class="suggestion-list">
            <button
              type="button"
              class="suggestion-card"
              onclick={() => { inputText = "Ajude a criar e estruturar um novo projeto."; autoResize(); textareaEl?.focus(); }}
            >
              <span class="suggestion-title">Criar um projeto</span>
              <span class="suggestion-desc">Estruturar um aplicativo ou componente</span>
              <span class="suggestion-arrow" aria-hidden="true">↗</span>
            </button>

            <button
              type="button"
              class="suggestion-card"
              onclick={() => { inputText = "Explique o funcionamento e a arquitetura deste código."; autoResize(); textareaEl?.focus(); }}
            >
              <span class="suggestion-title">Entender código</span>
              <span class="suggestion-desc">Ler arquitetura, fluxo e decisões técnicas</span>
              <span class="suggestion-arrow" aria-hidden="true">↗</span>
            </button>

            <button
              type="button"
              class="suggestion-card"
              onclick={() => { inputText = "Encontre e corrija falhas ou erros no meu projeto."; autoResize(); textareaEl?.focus(); }}
            >
              <span class="suggestion-title">Investigar um problema</span>
              <span class="suggestion-desc">Analisar erros de execução ou de build</span>
              <span class="suggestion-arrow" aria-hidden="true">↗</span>
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- Cards / Overlays -->
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

  <!-- Floating Scroll-to-bottom button & Floating Input Bar (Claude-style) -->
  <div class="input-area-wrapper">
    {#if showScrollBottom}
      <button
        type="button"
        class="scroll-bottom-btn fade-in"
        onclick={scrollToBottom}
        aria-label="Rolar para o final"
        title="Rolar para o final"
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="var(--text-secondary)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="5" x2="12" y2="18"/>
          <polyline points="6 12 12 18 18 12"/>
        </svg>
      </button>
    {/if}

    <form class="floating-input-box" onsubmit={handleSend}>
      <textarea
        bind:this={textareaEl}
        bind:value={inputText}
        oninput={autoResize}
        placeholder="Escreva uma mensagem..."
        rows="1"
        onkeydown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            handleSend();
          }
        }}
      ></textarea>

      <div class="input-toolbar">
        <div class="toolbar-left">
          <ModelSelector />
        </div>

        <div class="toolbar-right">
          {#if $isStreaming}
            <button type="button" class="btn-send stop-btn" onclick={handleCancel} aria-label="Cancelar">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                <rect x="6" y="6" width="12" height="12" rx="2"/>
              </svg>
            </button>
          {:else}
            <button type="submit" class="btn-send" disabled={!inputText.trim()} aria-label="Enviar">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <line x1="12" y1="19" x2="12" y2="5"/>
                <polyline points="5 12 12 5 19 12"/>
              </svg>
            </button>
          {/if}
        </div>
      </div>
    </form>

  </div>
</div>

<style>
  .chat-view {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg);
    position: relative;
  }

  .messages-container {
    flex: 1;
    overflow-y: auto;
    padding: 1.5rem 1rem 1rem 1rem;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .messages-inner {
    width: 100%;
    max-width: 768px;
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .message-row {
    display: flex;
    gap: 0.8rem;
    width: 100%;
    align-items: flex-start;
  }

  .message-row.user {
    justify-content: flex-end;
  }

  .message-row.assistant {
    justify-content: flex-start;
  }

  .message-row.tool-message + .message-row.tool-message {
    margin-top: -1rem;
  }

  .message-row.system {
    justify-content: center;
  }

  .message-content {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    max-width: 100%;
    min-width: 0;
    flex: 1;
  }

  .message-row.user .message-content {
    align-items: flex-end;
    flex: initial;
    max-width: 82%;
  }

  .bubble {
    word-wrap: break-word;
    overflow-wrap: break-word;
    word-break: break-word;
  }

  .user-bubble {
    background: var(--bg-card);
    color: var(--text);
    border: 1px solid var(--border);
    padding: 0.75rem 1.1rem;
    border-radius: 20px;
    border-bottom-right-radius: 6px;
    font-size: 0.95rem;
  }

  .user-bubble p {
    white-space: pre-wrap;
  }

  .assistant-bubble {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 0.95rem;
    color: var(--text-secondary);
  }

  .system-bubble {
    background: var(--danger-subtle);
    border: 1px solid var(--danger);
    color: var(--danger);
    padding: 0.6rem 1rem;
    border-radius: var(--radius-sm);
    font-size: 0.85rem;
  }

  .typing-bubble {
    display: flex;
    gap: 5px;
    align-items: center;
    padding: 0.5rem 0;
  }

  .typing-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
    animation: typingBounce 1.4s infinite ease-in-out;
  }

  .typing-dot:nth-child(1) { animation-delay: -0.32s; }
  .typing-dot:nth-child(2) { animation-delay: -0.16s; }

  @keyframes typingBounce {
    0%, 80%, 100% { transform: scale(0.6); opacity: 0.4; }
    40% { transform: scale(1); opacity: 1; }
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

  .tool-result {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    min-height: 2.6rem;
    padding: 0.5rem 0.7rem;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-left: 2px solid var(--accent);
    border-radius: var(--radius-sm);
  }

  .tool-result .tool-icon.ok { color: var(--success); }
  .tool-result .tool-icon.fail { color: var(--danger); }
  .tool-result:has(.tool-icon.ok) { border-left-color: var(--success); }
  .tool-result:has(.tool-icon.fail) { border-left-color: var(--danger); }

  .tool-label {
    color: var(--accent-hover);
    font-size: 0.86rem;
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tool-calls {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    margin-top: 0.1rem;
  }

  .error-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 1rem;
    background: var(--danger-subtle);
    border-top: 1px solid var(--danger);
    color: var(--danger);
    font-size: 0.85rem;
  }

  .error-banner span { flex: 1; }
  .error-banner button {
    background: transparent;
    border: none;
    color: var(--danger);
    font-size: 1.3rem;
    padding: 0 0.3rem;
  }

  .input-area-wrapper {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 0.65rem 1rem max(0.65rem, env(safe-area-inset-bottom));
    position: relative;
    background: transparent;
    border-top: none;
  }

  .scroll-bottom-btn {
    position: absolute;
    top: -38px;
    left: 50%;
    width: 30px;
    height: 30px;
    border-radius: var(--radius-sm);
    background: var(--bg-secondary);
    border: 1px solid var(--border-bright);
    color: var(--text-secondary);
    display: flex;
    align-items: center;
    justify-content: center;
    transform: translateX(-50%);
    cursor: pointer;
    transition: background-color var(--transition), color var(--transition), transform var(--transition);
    z-index: 5;
  }

  .scroll-bottom-btn svg {
    display: block;
    width: 16px;
    height: 16px;
    flex: 0 0 16px;
  }

  .scroll-bottom-btn:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .floating-input-box {
    width: 100%;
    max-width: 768px;
    background: var(--bg);
    border: 1px solid var(--border-bright);
    border-radius: var(--radius);
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow: hidden;
    padding: 0.65rem 0.75rem 0.55rem;
    transition: border-color var(--transition), background-color var(--transition);
  }

  .floating-input-box:focus-within {
    border-color: var(--accent);
    background: var(--bg-secondary);
  }

  .floating-input-box textarea {
    width: 100%;
    background: transparent;
    border: none;
    color: var(--text);
    font-size: 0.95rem;
    line-height: 1.5;
    padding: 0;
    max-height: 160px;
    min-height: 44px;
    box-shadow: none !important;
  }

  .floating-input-box textarea::placeholder {
    color: var(--text-faint);
  }

  .input-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    min-width: 0;
    margin-top: 0.5rem;
    padding-top: 0.35rem;
  }

  .toolbar-left,
  .toolbar-right {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }

  .toolbar-left {
    flex: 1;
  }

  .toolbar-right {
    flex: 0 0 auto;
  }

  .btn-send {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--bg);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    transition: background-color var(--transition), transform var(--transition);
  }

  .btn-send:hover:not(:disabled) {
    background: var(--accent-hover);
    transform: translateY(-1px);
  }

  .btn-send:disabled {
    background: var(--bg-hover);
    color: var(--text-faint);
    opacity: 0.6;
  }

  .btn-send.stop-btn {
    background: var(--danger);
    color: #ffffff;
  }

  .btn-send.stop-btn:hover {
    background: #b9635c;
  }

  @media (max-width: 768px) {
    .messages-container {
      padding: 1rem 0.5rem 0.5rem 0.5rem;
    }

    .messages-inner {
      gap: 1.25rem;
    }

    .message-row {
      gap: 0.5rem;
    }

    .message-row.user .message-content {
      max-width: 90%;
    }

    .user-bubble {
      padding: 0.65rem 0.9rem;
      font-size: 0.9rem;
      border-radius: 16px;
      border-bottom-right-radius: 4px;
    }

    .assistant-bubble {
      font-size: 0.9rem;
    }

    .input-area-wrapper {
      padding: 0.55rem 0.5rem max(0.55rem, env(safe-area-inset-bottom));
    }

    .floating-input-box {
      border-radius: var(--radius-sm);
      padding: 0.6rem 0.7rem 0.5rem;
    }

    .floating-input-box textarea {
      font-size: 0.9rem;
      min-height: 38px;
      max-height: 120px;
    }

    .input-toolbar {
      gap: 0.2rem;
    }

    .toolbar-left,
    .toolbar-right {
      gap: 0.25rem;
    }

    .btn-send {
      width: 32px;
      height: 32px;
    }

    .scroll-bottom-btn {
      top: -42px;
      width: 32px;
      height: 32px;
    }
  }

  .welcome-screen {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    padding: 2rem 0;
    max-width: 640px;
    margin: 0 auto;
    width: 100%;
  }

  .welcome-kicker {
    color: var(--accent);
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    margin-bottom: 0.7rem;
  }

  .welcome-heading {
    font-size: clamp(1.7rem, 4vw, 2.25rem);
    line-height: 1.1;
    font-weight: 600;
    color: var(--text);
    letter-spacing: -0.035em;
    margin-bottom: 0.65rem;
  }

  .welcome-copy {
    max-width: 480px;
    color: var(--text-muted);
    font-size: 0.92rem;
    line-height: 1.55;
    margin-bottom: 1.6rem;
  }

  .suggestion-list {
    width: 100%;
    border-top: 1px solid var(--border);
  }

  .suggestion-card {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-rows: auto auto;
    column-gap: 1rem;
    align-items: center;
    width: 100%;
    text-align: left;
    padding: 0.85rem 0.15rem;
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    cursor: pointer;
    transition: color var(--transition), padding-left var(--transition), background-color var(--transition);
  }

  .suggestion-card:hover {
    background: var(--bg-secondary);
    padding-left: 0.65rem;
    color: var(--text);
  }

  .suggestion-title {
    font-size: 0.9rem;
    font-weight: 550;
    color: var(--text-secondary);
    grid-column: 1;
  }

  .suggestion-desc {
    font-size: 0.78rem;
    color: var(--text-muted);
    grid-column: 1;
    margin-top: 0.12rem;
  }

  .suggestion-arrow {
    grid-column: 2;
    grid-row: 1 / span 2;
    color: var(--text-faint);
    font-size: 1.1rem;
    transition: color var(--transition), transform var(--transition);
  }

  .suggestion-card:hover .suggestion-arrow {
    color: var(--accent);
    transform: translate(2px, -2px);
  }

  @media (max-width: 480px) {
    .messages-container {
      padding-left: 0.35rem;
      padding-right: 0.35rem;
    }

    .floating-input-box {
      padding: 0.6rem;
    }

    .welcome-screen {
      justify-content: flex-start;
      padding: 3.5rem 0.5rem 1rem;
    }

    .welcome-heading {
      font-size: 1.8rem;
    }

    .welcome-copy {
      font-size: 0.86rem;
      margin-bottom: 1.2rem;
    }
  }
</style>
