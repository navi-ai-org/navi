<script lang="ts">
  import type { PendingPlanReview } from "../lib/types";
  import Markdown from "./Markdown.svelte";

  let {
    review,
    onApprove,
    onRequestChanges,
    onQuit,
  }: {
    review: PendingPlanReview;
    onApprove: () => void;
    onRequestChanges: (freeform: string) => void;
    onQuit: () => void;
  } = $props();

  let showFeedback = $state(false);
  let feedbackText = $state("");
</script>

<div class="plan-card fade-in-up">
  <div class="header">
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M9 11l3 3L22 4"/>
      <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>
    </svg>
    <span class="title">Plan Review</span>
  </div>

  <h4 class="plan-title">{review.title}</h4>
  {#if review.description}
    <p class="plan-description text-sm text-muted">{review.description}</p>
  {/if}

  {#if review.steps.length > 0}
    <ol class="plan-steps">
      {#each review.steps as step, i}
        <li>{step}</li>
      {/each}
    </ol>
  {/if}

  {#if review.bodyMarkdown}
    <div class="plan-body">
      <Markdown content={review.bodyMarkdown} />
    </div>
  {/if}

  {#if showFeedback}
    <textarea
      bind:value={feedbackText}
      placeholder="Feedback for changes..."
      rows="3"
      class="feedback-input"
    ></textarea>
  {/if}

  <div class="actions">
    <button class="btn-secondary" onclick={onQuit}>Quit</button>
    {#if showFeedback}
      <button
        class="btn-secondary"
        onclick={() => onRequestChanges(feedbackText)}
      >
        Send Feedback
      </button>
    {:else}
      <button class="btn-secondary" onclick={() => showFeedback = true}>
        Request Changes
      </button>
    {/if}
    <button class="btn-success" onclick={onApprove}>Approve</button>
  </div>
</div>

<style>
  .plan-card {
    margin: 0 1rem 0.5rem;
    padding: 0.9rem;
    background: var(--accent-subtle);
    border: 1px solid var(--accent);
    border-left: 4px solid var(--accent);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
    max-height: 60vh;
    overflow-y: auto;
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

  .plan-title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text);
    margin-bottom: 0.3rem;
  }

  .plan-description {
    margin-bottom: 0.6rem;
  }

  .plan-steps {
    margin: 0.5rem 0;
    padding-left: 1.3rem;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }

  .plan-steps li {
    margin: 0.25rem 0;
    line-height: 1.5;
  }

  .plan-body {
    margin: 0.5rem 0;
    padding: 0.6rem;
    background: var(--bg);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    font-size: 0.82rem;
  }

  .feedback-input {
    width: 100%;
    margin: 0.5rem 0;
    font-size: 0.85rem;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
    margin-top: 0.5rem;
  }

  .actions button {
    padding: 0.5rem 1rem;
    font-size: 0.85rem;
    font-weight: 500;
  }
</style>
