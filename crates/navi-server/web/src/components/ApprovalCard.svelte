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

<div class="approval-card">
  <div class="header">
    <span class="icon">⚠</span>
    <span class="title">Approval Required</span>
  </div>
  <p class="tool-name text-mono">{approval.toolName}</p>
  <p class="description">{approval.description}</p>
  <div class="actions">
    <button class="btn-danger" onclick={onDeny}>Deny</button>
    <button class="btn-success" onclick={onApprove}>Approve</button>
  </div>
</div>

<style>
  .approval-card {
    margin: 0 1rem 0.5rem;
    padding: 0.75rem;
    background: rgba(210, 153, 34, 0.1);
    border: 1px solid var(--warning);
    border-radius: var(--radius);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.5rem;
  }

  .icon {
    color: var(--warning);
    font-size: 1.1rem;
  }

  .title {
    font-weight: 600;
    color: var(--warning);
  }

  .tool-name {
    font-size: 0.85rem;
    color: var(--accent);
    margin-bottom: 0.25rem;
  }

  .description {
    font-size: 0.85rem;
    color: var(--text);
    margin-bottom: 0.75rem;
    white-space: pre-wrap;
  }

  .actions {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }

  .actions button {
    padding: 0.4rem 1rem;
    font-size: 0.9rem;
  }
</style>
