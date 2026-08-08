<script lang="ts">
  import { permissionMode } from "../lib/stores";
  import { getPermissionMode, setPermissionMode } from "../lib/api";
  import type { PermissionMode } from "../lib/types";

  let loaded = $state(false);
  let changing = $state(false);

  const modes: { value: PermissionMode; label: string; icon: string; color: string }[] = [
    { value: "restricted", label: "Restricted", icon: "🔒", color: "var(--danger)" },
    { value: "accept_edits", label: "Accept Edits", icon: "✏️", color: "var(--warning)" },
    { value: "auto", label: "Auto", icon: "⚡", color: "var(--accent)" },
    { value: "yolo", label: "Yolo", icon: "🚀", color: "var(--success)" },
  ];

  async function load() {
    if (loaded) return;
    try {
      const mode = await getPermissionMode();
      permissionMode.set(mode);
      loaded = true;
    } catch {
      // Keep default
    }
  }

  async function cycle() {
    if (changing) return;
    const current = $permissionMode;
    const idx = modes.findIndex((m) => m.value === current);
    const next = modes[(idx + 1) % modes.length];
    changing = true;
    try {
      const result = await setPermissionMode(next.value);
      permissionMode.set(result);
    } catch {
      // Revert on error
    } finally {
      changing = false;
    }
  }

  $effect(() => {
    load();
  });

  let currentMode = $derived(modes.find((m) => m.value === $permissionMode) ?? modes[0]);
</script>

<button
  class="perm-btn"
  onclick={cycle}
  disabled={changing}
  style="--mode-color: {currentMode.color}"
  aria-label="Permission mode: {currentMode.label}"
  title="Permission mode: {currentMode.label} (click to cycle)"
>
  {#if changing}
    <span class="spinner spinner-tiny"></span>
  {:else}
    <span class="perm-icon">{currentMode.icon}</span>
  {/if}
  <span class="perm-label text-xs">{currentMode.label}</span>
</button>

<style>
  .perm-btn {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.25rem 0.5rem;
    background: var(--bg-tertiary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    cursor: pointer;
    transition: border-color var(--transition), background var(--transition);
    white-space: nowrap;
  }

  .perm-btn:hover:not(:disabled) {
    border-color: var(--mode-color);
  }

  .perm-icon {
    font-size: 0.85rem;
    line-height: 1;
  }

  .perm-label {
    color: var(--mode-color);
    font-weight: 500;
  }

  .spinner-tiny {
    width: 11px;
    height: 11px;
    border-width: 1.5px;
  }

  @media (max-width: 480px) {
    .perm-label {
      display: none;
    }
  }
</style>
