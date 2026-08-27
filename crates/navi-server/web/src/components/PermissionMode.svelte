<script lang="ts">
  import { permissionMode } from "../lib/stores";
  import { getPermissionMode, setPermissionMode } from "../lib/api";
  import type { PermissionMode } from "../lib/types";

  let loaded = $state(false);
  let changing = $state(false);

  const modes: { value: PermissionMode; label: string; color: string }[] = [
    { value: "restricted", label: "Restricted", color: "var(--danger)" },
    { value: "accept_edits", label: "Accept edits", color: "var(--warning)" },
    { value: "auto", label: "Auto", color: "var(--accent)" },
    { value: "yolo", label: "Yolo", color: "var(--success)" },
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
    <span class="perm-dot" aria-hidden="true"></span>
  {/if}
  <span class="perm-label text-xs">{currentMode.label}</span>
</button>

<style>
  .perm-btn {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.25rem 0.4rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-xs);
    color: var(--text-secondary);
    cursor: pointer;
    transition: border-color var(--transition), background-color var(--transition);
    white-space: nowrap;
  }

  .perm-btn:hover:not(:disabled) {
    border-color: var(--border);
    background: var(--bg-secondary);
  }

  .perm-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--mode-color);
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
