<script lang="ts">
  import type { PendingSudoPrompt } from "../lib/types";

  let {
    prompt,
    onSubmit,
    onCancel,
  }: {
    prompt: PendingSudoPrompt;
    onSubmit: (password: string) => void;
    onCancel: () => void;
  } = $props();

  let password = $state("");
  let inputEl: HTMLInputElement | null = $state(null);

  $effect(() => {
    inputEl?.focus();
  });

  function submit(e: Event) {
    e.preventDefault();
    if (password) {
      onSubmit(password);
      password = "";
    }
  }
</script>

<div class="sudo-card fade-in-up">
  <div class="header">
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
    </svg>
    <span class="title">Sudo Password Required</span>
  </div>
  <p class="command text-mono text-sm">{prompt.commandSummary}</p>
  <form onsubmit={submit}>
    <input
      bind:this={inputEl}
      type="password"
      bind:value={password}
      placeholder="Password"
      autocomplete="off"
    />
    <div class="actions">
      <button type="button" class="btn-secondary" onclick={onCancel}>Cancel</button>
      <button type="submit" class="btn-primary" disabled={!password}>Submit</button>
    </div>
  </form>
</div>

<style>
  .sudo-card {
    margin: 0 1rem 0.5rem;
    padding: 0.9rem;
    background: var(--warning-subtle);
    border: 1px solid var(--warning);
    border-left: 4px solid var(--warning);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.5rem;
    color: var(--warning);
  }

  .title {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .command {
    background: var(--bg);
    padding: 0.4rem 0.6rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle);
    margin-bottom: 0.6rem;
    word-break: break-all;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  input {
    width: 100%;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }

  .actions button {
    padding: 0.5rem 1rem;
    font-size: 0.85rem;
    font-weight: 500;
  }
</style>
