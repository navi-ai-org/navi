<script lang="ts">
  import type { PendingApproval } from "../lib/types";

  let {
    approval,
    onApprove,
    onDeny,
  }: {
    approval: PendingApproval;
    onApprove: () => void;
    onDeny: () => void;
  } = $props();
</script>

<div class="approval-card fade-in-up">
  <div class="header">
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
      <line x1="12" y1="9" x2="12" y2="13"/>
      <line x1="12" y1="17" x2="12.01" y2="17"/>
    </svg>
    <span class="title">Permissão necessária</span>
  </div>
  <div class="body">
    <span class="tool-name text-mono">{approval.toolName}</span>
    <p class="description">{approval.description}</p>
  </div>
  <div class="actions">
    <button class="btn-secondary" onclick={onDeny}>Recusar</button>
    <button class="btn-success" onclick={onApprove}>Aceitar</button>
  </div>
</div>

<style>
  .approval-card {
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
    margin-bottom: 0.6rem;
    color: var(--warning);
  }

  .title {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .body {
    margin-bottom: 0.75rem;
  }

  .tool-name {
    display: inline-block;
    font-size: 0.8rem;
    color: var(--accent);
    background: var(--accent-subtle);
    padding: 0.15rem 0.5rem;
    border-radius: var(--radius-xs);
    margin-bottom: 0.4rem;
  }

  .description {
    font-size: 0.85rem;
    color: var(--text-secondary);
    white-space: pre-wrap;
    line-height: 1.5;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }

  .actions button {
    padding: 0.5rem 1.2rem;
    font-size: 0.9rem;
    font-weight: 500;
  }
</style>
