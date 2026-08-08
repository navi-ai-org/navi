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
    }
  }
</script>

<div class="question-card">
  <div class="header">
    <span class="icon">?</span>
    <span class="title">Question</span>
  </div>
  <p class="question-text">{question.question}</p>

  {#if question.options && question.options.length > 0}
    <div class="options">
      {#each question.options as opt}
        <button class="btn-secondary option-btn" onclick={() => answer(opt)}>
          {opt}
        </button>
      {/each}
    </div>
  {/if}

  <form class="custom-answer" onsubmit={submitCustom}>
    <input
      type="text"
      placeholder="Custom answer..."
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
    padding: 0.75rem;
    background: rgba(88, 166, 255, 0.1);
    border: 1px solid var(--accent);
    border-radius: var(--radius);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.5rem;
  }

  .icon {
    color: var(--accent);
    font-size: 1.1rem;
    font-weight: bold;
  }

  .title {
    font-weight: 600;
    color: var(--accent);
  }

  .question-text {
    font-size: 0.9rem;
    margin-bottom: 0.75rem;
    white-space: pre-wrap;
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
  }

  .custom-answer {
    display: flex;
    gap: 0.5rem;
  }

  .custom-answer input {
    flex: 1;
  }

  .custom-answer button {
    padding: 0.4rem 0.8rem;
    font-size: 0.9rem;
  }
</style>
