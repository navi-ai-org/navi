<script lang="ts">
  import { models, activeSession } from "../lib/stores";
  import { listModels, listProviderAccounts, setSessionModel } from "../lib/api";
  import type { ModelInfo } from "../lib/types";

  type ModelGroup = {
    providerId: string;
    providerLabel: string;
    models: ModelInfo[];
  };

  let loaded = $state(false);
  let localError = $state("");
  let loadingModels = $state(false);
  let changing = $state(false);
  let modalOpen = $state(false);
  let searchQuery = $state("");
  let searchInput: HTMLInputElement | null = $state(null);
  let recentIds = $state<string[]>([]);
  const recentStorageKey = "navi-recent-models";

  let currentSelection = $derived(
    $activeSession?.provider && $activeSession?.model
      ? `${$activeSession.provider}:${$activeSession.model}`
      : "",
  );

  let selectedModel = $derived(
    $models.find((model) => model.id === currentSelection),
  );

  let displayModel = $derived(
    selectedModel?.name ?? $activeSession?.model ?? "Modelo",
  );

  let modelGroups = $derived(groupModels($models, searchQuery));
  let recentModels = $derived(
    recentIds
      .map((id) => $models.find((model) => model.id === id))
      .filter((model): model is ModelInfo => Boolean(model))
      .filter((model) => matchesModel(model, searchQuery)),
  );

  function matchesModel(model: ModelInfo, query: string): boolean {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return true;
    return [model.name, model.id, model.providerId, model.providerLabel]
      .filter(Boolean)
      .join(" ")
      .toLowerCase()
      .includes(normalized);
  }

  function groupModels(list: ModelInfo[], query: string): ModelGroup[] {
    const groups = new Map<string, ModelGroup>();

    for (const model of list) {
      if (!matchesModel(model, query)) continue;

      const group = groups.get(model.providerId) ?? {
        providerId: model.providerId,
        providerLabel: model.providerLabel ?? model.providerId,
        models: [],
      };
      group.models.push(model);
      groups.set(model.providerId, group);
    }

    return [...groups.values()]
      .map((group) => ({
        ...group,
        models: [...group.models].sort((a, b) => a.name.localeCompare(b.name)),
      }))
      .sort((a, b) => a.providerLabel.localeCompare(b.providerLabel));
  }

  function rememberModel(modelId: string) {
    recentIds = [modelId, ...recentIds.filter((id) => id !== modelId)].slice(0, 6);
    localStorage.setItem(recentStorageKey, JSON.stringify(recentIds));
  }

  async function loadModels() {
    if (loaded || loadingModels) return;
    loadingModels = true;
    localError = "";
    try {
      const [list, accounts] = await Promise.all([
        listModels(),
        listProviderAccounts(),
      ]);
      const connectedProviders = new Set(
        accounts
          .filter((account) => account.status?.configured || account.hasStoredKey)
          .map((account) => account.providerId),
      );
      const connectedModels = list.filter((model) =>
        connectedProviders.has(model.providerId),
      );
      models.set(connectedModels);
      if (connectedModels.length === 0) {
        localError = "Nenhum provider conectado";
      }
      loaded = true;
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao carregar modelos";
    } finally {
      loadingModels = false;
    }
  }

  function openModal() {
    if (!$activeSession || loadingModels || changing) return;
    modalOpen = true;
    searchQuery = "";
  }

  function closeModal() {
    if (changing) return;
    modalOpen = false;
    searchQuery = "";
  }

  async function selectModel(model: ModelInfo) {
    if (!$activeSession || changing) return;
    const separator = model.id.indexOf(":");
    if (separator < 1) return;

    const provider = model.id.slice(0, separator);
    const name = model.id.slice(separator + 1);
    changing = true;
    localError = "";
    try {
      await setSessionModel($activeSession.id, provider, name);
      activeSession.update((session) =>
        session ? { ...session, provider, model: name } : session,
      );
      rememberModel(model.id);
      modalOpen = false;
      searchQuery = "";
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao selecionar modelo";
    } finally {
      changing = false;
    }
  }

  $effect(() => {
    try {
      const stored = JSON.parse(localStorage.getItem(recentStorageKey) ?? "[]");
      if (Array.isArray(stored)) {
        recentIds = stored.filter((id): id is string => typeof id === "string").slice(0, 6);
      }
    } catch {
      recentIds = [];
    }
    loadModels();
  });

  $effect(() => {
    if (!modalOpen) return;
    queueMicrotask(() => searchInput?.focus());
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeModal();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });
</script>

<div class="model-selector">
  <button
    type="button"
    class="model-trigger"
    onclick={openModal}
    disabled={loadingModels || changing || !$activeSession}
    aria-haspopup="dialog"
    aria-expanded={modalOpen}
    title={displayModel}
  >
    {#if loadingModels || changing}
      <span class="trigger-spinner spinner"></span>
    {/if}
    <span class="model-trigger-label">{displayModel}</span>
    <svg class="trigger-chevron" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <polyline points="6 9 12 15 18 9"/>
    </svg>
  </button>
  {#if localError}
    <span class="error text-xs">{localError}</span>
  {/if}
</div>

{#if modalOpen}
  <div class="model-modal-layer">
    <button type="button" class="model-modal-backdrop" onclick={closeModal} aria-label="Fechar seletor de modelo"></button>
    <div class="model-modal" role="dialog" aria-modal="true" aria-labelledby="model-picker-title">
      <div class="model-modal-handle" aria-hidden="true"></div>
      <header class="model-modal-header">
        <div>
          <h2 id="model-picker-title">Escolher modelo</h2>
          <p>Selecione um modelo conectado para esta conversa.</p>
        </div>
        <button type="button" class="model-modal-close" onclick={closeModal} aria-label="Fechar">×</button>
      </header>

      <label class="model-search">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7"/>
          <line x1="16.5" y1="16.5" x2="21" y2="21"/>
        </svg>
        <input bind:this={searchInput} bind:value={searchQuery} type="search" placeholder="Buscar modelo ou provider" aria-label="Buscar modelo ou provider" />
        {#if searchQuery}
          <button type="button" class="model-search-clear" onclick={() => searchQuery = ""} aria-label="Limpar busca">×</button>
        {/if}
      </label>

      <div class="model-modal-list">
        {#if modelGroups.length === 0}
          <div class="model-empty">
            <strong>Nenhum modelo encontrado</strong>
            <span>Tente buscar por outro nome.</span>
          </div>
        {:else}
          {#if recentModels.length > 0}
            <div class="model-group recent-model-group">
              <div class="model-group-heading">
                <span>Recentes</span>
                <span class="model-group-count">{recentModels.length}</span>
              </div>
              <div class="model-group-list">
                {#each recentModels as model}
                  <button
                    type="button"
                    class="model-option"
                    class:selected={model.id === currentSelection}
                    onclick={() => selectModel(model)}
                    disabled={changing}
                  >
                    <span class="model-option-copy">
                      <span class="model-option-name">{model.name}</span>
                      <span class="model-option-id">{model.id}</span>
                    </span>
                    <span class="model-option-capabilities">
                      {#if model.supportsThinking}<span>Thinking</span>{/if}
                    </span>
                    {#if model.id === currentSelection}
                      <svg class="model-check" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-label="Modelo atual"><polyline points="20 6 9 17 4 12"/></svg>
                    {/if}
                  </button>
                {/each}
              </div>
            </div>
          {/if}

          {#each modelGroups as group}
            <div class="model-group">
              <div class="model-group-heading">
                <span>{group.providerLabel}</span>
                <span class="model-group-count">{group.models.length}</span>
              </div>
              <div class="model-group-list">
                {#each group.models as model}
                  <button
                    type="button"
                    class="model-option"
                    class:selected={model.id === currentSelection}
                    onclick={() => selectModel(model)}
                    disabled={changing}
                  >
                    <span class="model-option-copy">
                      <span class="model-option-name">{model.name}</span>
                      <span class="model-option-id">{model.id}</span>
                    </span>
                    <span class="model-option-capabilities">
                      {#if model.supportsThinking}<span>Thinking</span>{/if}
                    </span>
                    {#if model.id === currentSelection}
                      <svg class="model-check" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-label="Modelo atual"><polyline points="20 6 9 17 4 12"/></svg>
                    {/if}
                  </button>
                {/each}
              </div>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .model-selector {
    display: flex;
    min-width: 0;
    max-width: 100%;
  }

  .model-trigger {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    max-width: min(280px, 100%);
    padding: 0.3rem 0.45rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-xs);
    color: var(--text-muted);
    font-size: 0.78rem;
    font-weight: 500;
    text-align: left;
  }

  .model-trigger:hover:not(:disabled),
  .model-trigger[aria-expanded="true"] {
    background: var(--bg-tertiary);
    border-color: var(--border);
    color: var(--text);
  }

  .model-trigger-label {
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: normal;
  }

  .trigger-chevron {
    flex: 0 0 auto;
    color: var(--text-muted);
  }

  .trigger-spinner {
    width: 11px;
    height: 11px;
    flex: 0 0 11px;
    border-width: 1.5px;
  }

  .error {
    position: absolute;
    transform: translateY(1.8rem);
    color: var(--danger);
    white-space: nowrap;
  }

  .model-modal-layer {
    position: fixed;
    inset: 0;
    z-index: 2000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
  }

  .model-modal-backdrop {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 0;
    background: rgba(0, 0, 0, 0.62);
    border: none;
    border-radius: 0;
  }

  .model-modal {
    position: relative;
    display: flex;
    flex-direction: column;
    width: min(100%, 560px);
    max-height: min(78dvh, 680px);
    overflow: hidden;
    background: var(--bg-secondary);
    border: 1px solid var(--border-bright);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
  }

  .model-modal-handle {
    display: none;
  }

  .model-modal-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.1rem 1.15rem 0.85rem;
    border-bottom: 1px solid var(--border);
  }

  .model-modal-header h2 {
    color: var(--text);
    font-size: 1rem;
    font-weight: 600;
  }

  .model-modal-header p {
    margin-top: 0.2rem;
    color: var(--text-muted);
    font-size: 0.78rem;
  }

  .model-modal-close {
    width: 2rem;
    height: 2rem;
    padding: 0;
    background: transparent;
    color: var(--text-muted);
    font-size: 1.35rem;
    line-height: 1;
  }

  .model-modal-close:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .model-search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0.85rem 1.15rem 0.45rem;
    padding: 0.55rem 0.65rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
  }

  .model-search:focus-within {
    border-color: var(--accent);
  }

  .model-search input {
    flex: 1;
    min-width: 0;
    padding: 0;
    background: transparent;
    border: none;
    box-shadow: none;
    color: var(--text);
    font-size: 0.84rem;
  }

  .model-search input:focus {
    border: none;
    box-shadow: none;
  }

  .model-search input::-webkit-search-cancel-button {
    display: none;
  }

  .model-search-clear {
    padding: 0 0.15rem;
    background: transparent;
    color: var(--text-muted);
    font-size: 1rem;
    line-height: 1;
  }

  .model-modal-list {
    min-height: 0;
    overflow-y: auto;
    padding: 0.35rem 0.65rem 0.9rem;
  }

  .model-group + .model-group {
    margin-top: 0.9rem;
  }

  .model-group-heading {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.45rem 0.5rem 0.3rem;
    color: var(--text-muted);
    font-size: 0.72rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .recent-model-group .model-group-heading {
    color: var(--accent);
  }

  .model-group-count {
    min-width: 1.25rem;
    padding: 0.05rem 0.3rem;
    border-radius: var(--radius-xs);
    background: var(--bg-tertiary);
    color: var(--text-faint);
    font-size: 0.66rem;
    text-align: center;
  }

  .model-group-list {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }

  .model-option {
    display: flex;
    align-items: center;
    gap: 0.65rem;
    width: 100%;
    min-height: 3rem;
    padding: 0.55rem 0.65rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    text-align: left;
  }

  .model-option:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--border);
    color: var(--text);
  }

  .model-option.selected {
    background: var(--accent-subtle);
    border-color: var(--accent);
    color: var(--text);
  }

  .model-option-copy {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }

  .model-option-name {
    overflow: hidden;
    font-size: 0.84rem;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .model-option-id {
    overflow: hidden;
    margin-top: 0.12rem;
    color: var(--text-faint);
    font-family: var(--font-mono);
    font-size: 0.66rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .model-option-capabilities {
    color: var(--accent-hover);
    font-size: 0.66rem;
    white-space: nowrap;
  }

  .model-check {
    flex: 0 0 auto;
    color: var(--success);
  }

  .model-empty {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 2rem 1rem;
    color: var(--text-muted);
    text-align: center;
  }

  .model-empty strong {
    color: var(--text-secondary);
    font-size: 0.88rem;
  }

  .model-empty span {
    font-size: 0.78rem;
  }

  @media (max-width: 560px) {
    .model-trigger {
      max-width: min(230px, 100%);
    }

    .model-modal-layer {
      align-items: flex-end;
      padding: 0;
    }

    .model-modal {
      width: 100%;
      max-height: 88dvh;
      border-right: none;
      border-bottom: none;
      border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    }

    .model-modal-handle {
      display: block;
      width: 2.4rem;
      height: 0.22rem;
      margin: 0.6rem auto 0;
      border-radius: 99px;
      background: var(--border-bright);
    }

    .model-modal-header {
      padding-top: 0.7rem;
    }

    .model-option {
      min-height: 3.25rem;
    }
  }
</style>
