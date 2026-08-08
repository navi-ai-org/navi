<script lang="ts">
  import type { PendingQuestion } from "../lib/types";

  let {
    question,
    onAnswer,
  }: {
    question: PendingQuestion;
    onAnswer: (answer: string) => void;
  } = $props();

  let customAnswer = $state("");

  function answer(text: string) {
    onAnswer(text);
  }

  function submitCustom(e: Event) {
    e.preventDefault();
    if (customAnswer.trim()) {
      answer(customAnswer.trim());
      customAnswer = "";
    }
  }
</script>

<div class="question-card fade-in-up">
  <div class="header">
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="10"/>
      <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/>
      <line x1="12" y1="17" x2="12.01" y2="17"/>
    </svg>
    <span class="title">Question</span>
  </div>
  <p class="question-text">{question.question}</p>

  {#if question.options && question.options.length > 0}
    <div class="options">
      {#each question.options as opt}
        <button class="btn-secondary option-btn" onclick={() => answer(opt)}>
          <span>{opt}</span>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="9 18 15 12 9 6"/>
          </svg>
        </button>
      {/each}
    </div>
  {/if}

  <form class="custom-answer" onsubmit={submitCustom}>
    <input
      type="text"
      placeholder="Or type a custom answer..."
      bind:value={customAnswer}
    />
    <button type="submit" class="btn-primary" disabled={!customAnswer.trim()}>
      Send
    </button>
  </form>
</div>

<style>
  .question-card {
    margin: 0 1rem 0.5rem;
    padding: 0.9rem;
    background: var(--accent-subtle);
    border: 1px solid var(--accent);
    border-left: 4px solid var(--accent);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.6rem;
    color: var(--accent);
  }

  .title {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .question-text {
    font-size: 0.9rem;
    color: var(--text);
    margin-bottom: 0.75rem;
    white-space: pre-wrap;
    line-height: 1.5;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin-bottom: 0.75rem;
  }

  .option-btn {
    text-align: left;
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.55rem 0.8rem;
  }

  .option-btn svg {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .option-btn:hover svg {
    color: var(--accent);
  }

  .custom-answer {
    display: flex;
    gap: 0.5rem;
  }

  .custom-answer input {
    flex: 1;
  }

  .custom-answer button {
    padding: 0.5rem 1rem;
    font-size: 0.9rem;
    font-weight: 500;
  }
</style>
