<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    sessions,
    savedSessions,
    activeSession,
    showSidebar,
    clearChat,
    messages,
    eventsToMessages,
  } from "../lib/stores";
  import {
    listSessions,
    listSavedSessions,
    startSession,
    loadSavedSession,
    deleteSavedSession,
    getSessionSnapshot,
    getSessionInfo,
    closeSession,
    renameSession,
    approveTool,
  } from "../lib/api";
  import { EventStream } from "../lib/ws";
  import type {
    PendingApproval,
    RuntimeEvent,
    SavedSessionInfo,
    SessionInfo,
    SessionSnapshot,
  } from "../lib/types";

  let {
    onLogout,
    serverOnline,
  }: { onLogout: () => void; serverOnline: boolean } = $props();

  type SessionActivity = {
    state: "idle" | "working" | "needs_action";
    approval?: PendingApproval;
  };

  let loading = $state(false);
  let initialLoaded = $state(false);
  let localError = $state("");
  let confirmingDelete = $state<string | null>(null);
  let menuOpen = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let renameValue = $state("");
  let expandedTitle = $state<string | null>(null);
  let searchQuery = $state("");
  const sessionCacheKey = "navi-session-list";
  let savedLoaded = $state(false);
  let cachedSessions = $state(readCachedSessions());
  let activity = $state<Record<string, SessionActivity>>({});
  let titleHoldTimer: ReturnType<typeof setTimeout> | null = null;
  let longPressId: string | null = null;
  const monitors = new Map<string, EventStream>();
  const toolNames = new Map<string, Map<string, string>>();

  function readCachedSessions(): SavedSessionInfo[] {
    try {
      const parsed = JSON.parse(localStorage.getItem(sessionCacheKey) ?? "[]");
      if (!Array.isArray(parsed)) return [];
      return parsed.filter((session): session is SavedSessionInfo =>
        session && typeof session.id === "string" && typeof session.updatedAt === "number",
      );
    } catch {
      return [];
    }
  }

  function cacheSessions(list: SavedSessionInfo[]) {
    cachedSessions = list.map(({ id, title, createdAt, updatedAt }) => ({
      id,
      title,
      project: "",
      createdAt,
      updatedAt,
    }));
    try {
      localStorage.setItem(sessionCacheKey, JSON.stringify(cachedSessions));
    } catch {
      return;
    }
  }

  async function refreshSavedSessions() {
    const saved = await listSavedSessions();
    savedSessions.set(saved);
    cacheSessions(saved);
    savedLoaded = true;
  }

  async function refresh() {
    loading = true;
    localError = "";
    const results = await Promise.allSettled([
      listSessions(),
      listSavedSessions(),
    ]);
    const loadedResult = results[0];
    const savedResult = results[1];

    if (loadedResult.status === "fulfilled") {
      sessions.set(loadedResult.value);
    } else {
      localError = loadedResult.reason instanceof Error
        ? loadedResult.reason.message
        : "Falha ao carregar sessões em andamento";
    }

    if (savedResult.status === "fulfilled") {
      savedSessions.set(savedResult.value);
      cacheSessions(savedResult.value);
      savedLoaded = true;
    } else if (!localError) {
      localError = savedResult.reason instanceof Error
        ? savedResult.reason.message
        : "Falha ao carregar conversas";
    }

    loading = false;
    initialLoaded = true;
  }

  function setActivity(
    sessionId: string,
    state: SessionActivity["state"],
    approval?: PendingApproval,
  ) {
    activity = {
      ...activity,
      [sessionId]: { state, approval },
    };
  }

  function upsertSessionTitle(sessionId: string, title: string) {
    const source = savedLoaded ? $savedSessions : cachedSessions;
    const now = Math.floor(Date.now() / 1000);
    const existing = source.find((session) => session.id === sessionId);
    const updated = existing
      ? source.map((session) =>
          session.id === sessionId
            ? { ...session, title, updatedAt: now }
            : session,
        )
      : [
          ...source,
          {
            id: sessionId,
            title,
            project: $activeSession?.projectDir ?? "",
            createdAt: now,
            updatedAt: now,
          },
        ];

    cachedSessions = updated;
    cacheSessions(updated);
    if (savedLoaded) savedSessions.set(updated);
  }

  function handleSessionEvent(sessionId: string, event: RuntimeEvent) {
    const kind = event.kind as Record<string, unknown>;
    const variants = Object.keys(kind);
    if (variants.length !== 1) return;

    const variant = variants[0];
    const payload = (kind[variant] ?? {}) as Record<string, unknown>;

    switch (variant) {
      case "TurnStarted":
      case "AssistantDelta":
      case "AssistantThinkingDelta":
      case "ToolStarted":
      case "ToolCompleted":
      case "ApprovalResolved":
      case "QuestionResolved":
      case "PlanReviewResolved":
        setActivity(sessionId, "working");
        break;
      case "ToolRequested": {
        const id = String(payload.id ?? "");
        const name = String(payload.tool_name ?? "tool");
        const names = toolNames.get(sessionId) ?? new Map<string, string>();
        names.set(id, name);
        toolNames.set(sessionId, names);
        setActivity(sessionId, "working");
        break;
      }
      case "TurnCompleted":
      case "SessionFinished":
      case "Error":
      case "AutoCompactFailed":
        setActivity(sessionId, "idle");
        break;
      case "SessionSaved":
        setActivity(sessionId, "idle");
        void refreshSavedSessions().catch(() => {});
        break;
      case "SessionTitleUpdated": {
        const title = String(payload.title ?? "").trim();
        if (title) upsertSessionTitle(sessionId, title);
        break;
      }
      case "ApprovalRequired": {
        const requestId = String(payload.id ?? "");
        const toolName = toolNames.get(sessionId)?.get(requestId) ?? "Ação protegida";
        setActivity(sessionId, "needs_action", {
          requestId,
          toolName,
          description: String(payload.summary ?? "Permissão necessária"),
        });
        break;
      }
      case "QuestionRequired":
      case "PlanReviewRequired":
      case "SudoPasswordRequired":
        setActivity(sessionId, "needs_action");
        break;
    }
  }

  function syncMonitors(ids: string[]) {
    const wanted = new Set(ids);
    for (const [id, stream] of monitors) {
      if (!wanted.has(id)) {
        stream.disconnect();
        monitors.delete(id);
      }
    }

    for (const id of ids) {
      if (monitors.has(id)) continue;
      const stream = new EventStream(id);
      stream.onEvent((event) => handleSessionEvent(id, event));
      stream.connect();
      monitors.set(id, stream);
      if (!activity[id]) setActivity(id, "idle");
    }
  }

  function mergeSessions(
    saved: SavedSessionInfo[],
    loaded: string[],
  ): SavedSessionInfo[] {
    const byId = new Map(saved.map((session) => [session.id, session]));
    for (const id of loaded) {
      if (!byId.has(id)) {
        const now = Math.floor(Date.now() / 1000);
        byId.set(id, {
          id,
          title: null,
          project: "",
          createdAt: now,
          updatedAt: now,
        });
      }
    }
    return [...byId.values()].sort((a, b) => b.updatedAt - a.updatedAt);
  }

  let allSessions = $derived(
    mergeSessions(savedLoaded ? $savedSessions : cachedSessions, $sessions),
  );
  let sessionGroups = $derived([
    {
      label: "Em andamento",
      live: true,
      sessions: allSessions.filter((session) => activity[session.id]?.state !== "idle"),
    },
    {
      label: "Conversas",
      live: false,
      sessions: allSessions.filter((session) => activity[session.id]?.state === "idle" || !activity[session.id]),
    },
  ].filter((group) => group.sessions.length > 0));
  type WorkspaceGroup = {
    key: string;
    label: string;
    project: string;
    sessions: SavedSessionInfo[];
  };

  function workspaceLabel(project: string): string {
    const normalized = project.replace(/[\\/]+$/, "");
    const parts = normalized.split(/[\\/]/).filter(Boolean);
    return parts.at(-1) ?? "Workspace local";
  }

  function groupByWorkspace(list: SavedSessionInfo[]): WorkspaceGroup[] {
    const groups = new Map<string, WorkspaceGroup>();
    for (const session of list) {
      const key = session.project || "__local__";
      const group = groups.get(key) ?? {
        key,
        label: session.project ? workspaceLabel(session.project) : "Workspace local",
        project: session.project,
        sessions: [],
      };
      group.sessions.push(session);
      groups.set(key, group);
    }
    return [...groups.values()].sort(
      (a, b) => (b.sessions[0]?.updatedAt ?? 0) - (a.sessions[0]?.updatedAt ?? 0),
    );
  }

  let filteredGroups = $derived(
    sessionGroups
      .map((group) => ({
        ...group,
        sessions: group.sessions.filter((session) => {
          const query = searchQuery.trim().toLowerCase();
          if (!query) return true;
          return [session.title, session.id, session.project]
            .filter(Boolean)
            .some((value) => String(value).toLowerCase().includes(query));
        }),
      }))
      .filter((group) => group.sessions.length > 0),
  );

  let workspaceGroups = $derived(
    filteredGroups.map((group) => ({
      ...group,
      workspaces: groupByWorkspace(group.sessions),
    })),
  );

  $effect(() => {
    refresh();
  });

  $effect(() => {
    const ids = $sessions;
    const timer = setTimeout(() => syncMonitors(ids), 0);
    return () => clearTimeout(timer);
  });

  onDestroy(() => {
    endTitleHold();
    for (const stream of monitors.values()) stream.disconnect();
    monitors.clear();
  });

  async function handleNewSession() {
    loading = true;
    localError = "";
    try {
      const info = await startSession();
      await selectSession(info, null);
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao criar conversa";
    } finally {
      loading = false;
    }
  }

  async function handleOpenSession(id: string) {
    if ($activeSession?.id === id) {
      showSidebar.set(false);
      return;
    }

    loading = true;
    localError = "";
    try {
      if ($sessions.includes(id)) {
        const [info, snapshot] = await Promise.all([
          getSessionInfo(id),
          getSessionSnapshot(id),
        ]);
        await selectSession(info, snapshot);
      } else {
        const info = await loadSavedSession(id);
        await selectSession(info, info.snapshot ?? null);
      }
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao abrir conversa";
    } finally {
      loading = false;
    }
  }

  async function handleRowApproval(
    sessionId: string,
    approved: boolean,
    event: Event,
  ) {
    event.stopPropagation();
    const approval = activity[sessionId]?.approval;
    if (!approval) return;

    try {
      await approveTool(sessionId, approval.requestId, approved);
      setActivity(sessionId, "working");
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao responder à permissão";
    }
  }

  async function handleDeleteSaved(id: string, e: Event) {
    e.stopPropagation();
    menuOpen = null;
    if (confirmingDelete === id) {
      try {
        await deleteSavedSession(id);
        confirmingDelete = null;
        await refresh();
      } catch (err) {
        localError = err instanceof Error ? err.message : "Falha ao excluir conversa";
      }
    } else {
      confirmingDelete = id;
    }
  }

  async function handleCloseActive(id: string, e: Event) {
    e.stopPropagation();
    menuOpen = null;
    try {
      await closeSession(id);
      setActivity(id, "idle");
      if ($activeSession?.id === id) {
        activeSession.set(null);
        clearChat();
      }
      await refresh();
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao fechar conversa";
    }
  }

  function openRename(id: string, currentTitle: string) {
    menuOpen = null;
    renaming = id;
    renameValue = currentTitle;
  }

  function startRename(id: string, currentTitle: string, e: Event) {
    e.stopPropagation();
    openRename(id, currentTitle);
  }

  function beginTitleHold(id: string, currentTitle: string, e: PointerEvent) {
    e.stopPropagation();
    if (titleHoldTimer) clearTimeout(titleHoldTimer);
    titleHoldTimer = setTimeout(() => {
      longPressId = id;
      openRename(id, currentTitle);
    }, 550);
  }

  function endTitleHold() {
    if (titleHoldTimer) clearTimeout(titleHoldTimer);
    titleHoldTimer = null;
  }

  function toggleTitle(id: string, e: Event) {
    e.stopPropagation();
    endTitleHold();
    if (longPressId === id) {
      longPressId = null;
      return;
    }
    expandedTitle = expandedTitle === id ? null : id;
  }

  async function confirmRename(id: string, e: Event) {
    e.stopPropagation();
    e.preventDefault();
    if (!renameValue.trim()) {
      renaming = null;
      longPressId = null;
      return;
    }

    try {
      await renameSession(id, renameValue.trim());
      if ($activeSession?.id === id) {
        activeSession.update((session) =>
          session ? { ...session, title: renameValue.trim() } : session,
        );
      }
      renaming = null;
      longPressId = null;
      await refresh();
    } catch (err) {
      localError = err instanceof Error ? err.message : "Falha ao renomear conversa";
      renaming = null;
      longPressId = null;
    }
  }

  function toggleMenu(id: string, e: Event) {
    e.stopPropagation();
    confirmingDelete = null;
    menuOpen = menuOpen === id ? null : id;
  }

  async function selectSession(
    info: SessionInfo,
    snapshot: SessionSnapshot | null,
  ) {
    clearChat();
    activeSession.set(info);
    showSidebar.set(false);
    if (snapshot?.events?.length) {
      messages.set(eventsToMessages(snapshot.events));
    }
    await refresh();
  }

  function formatRelativeTime(timestamp: number): string {
    if (!timestamp) return "";
    const date = new Date(timestamp < 1_000_000_000_000 ? timestamp * 1000 : timestamp);
    const diff = Date.now() - date.getTime();
    const mins = Math.floor(diff / 60000);
    const hours = Math.floor(diff / 3600000);
    const days = Math.floor(diff / 86400000);
    if (mins < 1) return "agora";
    if (mins < 60) return `${mins} min`;
    if (hours < 24) return `${hours} h`;
    if (days < 7) return `${days} d`;
    return date.toLocaleDateString("pt-BR");
  }
</script>

<div class="session-list">
  <div class="header">
    <div class="brand">
      <h2>NAVI</h2>
    </div>
    <button class="btn-ghost btn-sm" onclick={onLogout} aria-label="Sair">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/>
        <polyline points="16 17 21 12 16 7"/>
        <line x1="21" y1="12" x2="9" y2="12"/>
      </svg>
    </button>
  </div>

  <div class="status-bar">
    <span class="dot" class:online={serverOnline} class:offline={!serverOnline}></span>
    <span class="text-sm text-muted">{serverOnline ? "Servidor conectado" : "Servidor offline"}</span>
  </div>

  <button class="btn-primary new-session" onclick={handleNewSession} disabled={loading}>
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
      <line x1="12" y1="5" x2="12" y2="19"/>
      <line x1="5" y1="12" x2="19" y2="12"/>
    </svg>
    Nova conversa
  </button>

  <label class="search-box">
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <circle cx="11" cy="11" r="7"/>
      <line x1="16.5" y1="16.5" x2="21" y2="21"/>
    </svg>
    <input bind:value={searchQuery} type="search" placeholder="Buscar conversas" aria-label="Buscar conversas" />
    {#if searchQuery}
      <button type="button" class="search-clear" onclick={() => searchQuery = ""} aria-label="Limpar busca">×</button>
    {/if}
  </label>

  {#if localError}
    <p class="error text-sm fade-in" role="alert">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      {localError}
    </p>
  {/if}

  {#if !initialLoaded && loading && !cachedSessions.length}
    <div class="section">
      <div class="skeleton skeleton-header"></div>
      {#each Array(4) as _, i}
        <div class="skeleton skeleton-item" style="animation-delay: {i * 0.1}s"></div>
      {/each}
    </div>
  {/if}

  {#each workspaceGroups as group}
    <div class="section" class:live-section={group.live}>
      <h3>{group.label} <span class="count">{group.sessions.length}</span></h3>
      {#each group.workspaces as workspace}
        <div class="workspace-group" title={workspace.project || workspace.label}>
          <div class="workspace-heading">
            <span class="workspace-name">{workspace.label}</span>
            <span class="workspace-count">{workspace.sessions.length}</span>
          </div>
          {#each workspace.sessions as session}
        {#if renaming === session.id}
          <form class="rename-form" onsubmit={(e) => confirmRename(session.id, e)}>
            <input bind:value={renameValue} placeholder="Nome da conversa" />
            <button type="submit" class="btn-primary btn-sm">Salvar</button>
            <button type="button" class="btn-secondary btn-sm" onclick={() => { renaming = null; longPressId = null; }}>Cancelar</button>
          </form>
        {:else}
          <div
            class="session-item"
            class:active={$activeSession?.id === session.id}
            class:working={activity[session.id]?.state === "working"}
            class:needs-action={activity[session.id]?.state === "needs_action"}
          >
            <div
              class="session-open"
              role="button"
              tabindex="0"
              onclick={() => handleOpenSession(session.id)}
              onkeydown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); handleOpenSession(session.id); } }}
              aria-label={`Abrir ${session.title || "Nova sessão"}`}
            >
              <span class="session-status" class:working={activity[session.id]?.state === "working"} class:needs-action={activity[session.id]?.state === "needs_action"}>
                {#if activity[session.id]?.state === "working"}
                  <span class="activity-spinner"></span>
                {/if}
              </span>
              <span class="session-info">
                <button
                  type="button"
                  class="session-title title-toggle"
                  class:expanded={expandedTitle === session.id}
                  onclick={(e) => toggleTitle(session.id, e)}
                  onpointerdown={(e) => beginTitleHold(session.id, session.title || "Nova sessão", e)}
                  onpointerup={endTitleHold}
                  onpointercancel={endTitleHold}
                  onpointerleave={endTitleHold}
                  oncontextmenu={(e) => { e.preventDefault(); openRename(session.id, session.title || "Nova sessão"); }}
                  aria-label="Expandir título. Pressione e segure para renomear"
                >
                  {session.title || "Nova sessão"}
                </button>
                <span class="session-meta">
                  <span class="text-xs text-faint">{formatRelativeTime(session.updatedAt)}</span>
                  {#if $sessions.includes(session.id)}
                    <span class="text-xs text-faint">· aberta</span>
                  {/if}
                </span>
              </span>
            </div>

            <button class="menu-toggle" onclick={(e) => toggleMenu(session.id, e)} aria-label="Opções da conversa">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/></svg>
            </button>

            {#if menuOpen === session.id}
              <div class="ctx-menu fade-in-up" role="menu" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
                <button class="ctx-item" role="menuitem" onclick={(e) => startRename(session.id, session.title || "Nova sessão", e)}>
                  Renomear
                </button>
                {#if $sessions.includes(session.id)}
                  <button class="ctx-item danger" role="menuitem" onclick={(e) => handleCloseActive(session.id, e)}>
                    Fechar sessão
                  </button>
                {:else}
                  <button class="ctx-item danger" role="menuitem" onclick={(e) => handleDeleteSaved(session.id, e)}>
                    Excluir conversa
                  </button>
                {/if}
              </div>
            {/if}

            {#if confirmingDelete === session.id && menuOpen !== session.id}
              <button class="btn-delete confirming" onclick={(e) => handleDeleteSaved(session.id, e)} aria-label="Confirmar exclusão">
                Confirmar
              </button>
            {/if}

            {#if activity[session.id]?.approval}
              <div class="approval-inline" role="group" aria-label="Permissão necessária">
                <span class="approval-inline-label">
                  {activity[session.id]?.approval?.toolName} precisa de permissão
                </span>
                <span class="approval-inline-description truncate">
                  {activity[session.id]?.approval?.description}
                </span>
                <span class="approval-inline-actions">
                  <button class="approval-deny" onclick={(e) => handleRowApproval(session.id, false, e)}>Recusar</button>
                  <button class="approval-accept" onclick={(e) => handleRowApproval(session.id, true, e)}>Aceitar</button>
                </span>
              </div>
            {/if}
          </div>
        {/if}
          {/each}
        </div>
      {/each}
    </div>
  {/each}

  {#if initialLoaded && searchQuery && !filteredGroups.length}
    <div class="empty-state fade-in">
      <p class="text-muted text-sm">Nenhuma conversa encontrada</p>
      <p class="text-faint text-xs">Tente outro título ou caminho</p>
    </div>
  {:else if initialLoaded && !allSessions.length}
    <div class="empty-state fade-in">
      <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="var(--text-faint)" stroke-width="1.5">
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
      </svg>
      <p class="text-muted text-sm">Nenhuma conversa ainda</p>
      <p class="text-faint text-xs">Crie uma para começar</p>
    </div>
  {/if}
</div>

<style>
  .session-list {
    display: flex;
    flex-direction: column;
    padding: 0.85rem;
    gap: 0.85rem;
    height: 100%;
    overflow-y: auto;
    background: var(--bg-secondary);
  }

  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.2rem 0.2rem 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  h2 {
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--text);
    letter-spacing: -0.01em;
  }

  .btn-sm {
    padding: 0.35rem;
  }

  .status-bar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0 0.2rem 0.25rem;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }

  .dot.online {
    background: var(--success);
  }

  .dot.offline {
    background: var(--text-faint);
  }

  .new-session {
    width: 100%;
    padding: 0.65rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    font-weight: 500;
    border-radius: var(--radius-sm);
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-height: 2rem;
    padding: 0 0.55rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-faint);
  }

  .search-box:focus-within {
    border-color: var(--accent);
  }

  .search-box input {
    flex: 1;
    min-width: 0;
    padding: 0;
    background: transparent;
    border: none;
    box-shadow: none;
    color: var(--text);
    font-size: 0.8rem;
  }

  .search-box input:focus {
    border: none;
    box-shadow: none;
  }

  .search-box input::-webkit-search-cancel-button {
    display: none;
  }

  .search-clear {
    padding: 0 0.15rem;
    background: transparent;
    border: none;
    color: var(--text-muted);
    font-size: 1rem;
    line-height: 1;
  }

  .search-clear:hover {
    background: transparent;
    color: var(--text);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .live-section {
    padding-bottom: 0.65rem;
    border-bottom: 1px solid var(--border);
  }

  .live-section h3 {
    color: var(--accent);
  }

  .workspace-group + .workspace-group {
    margin-top: 0.55rem;
  }

  .workspace-heading {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    padding: 0.2rem 0.35rem;
    color: var(--text-muted);
  }

  .workspace-heading::before {
    content: "";
    width: 5px;
    height: 5px;
    flex: 0 0 5px;
    border: 1px solid var(--text-faint);
    border-radius: 1px;
  }

  .workspace-name {
    overflow: hidden;
    font-size: 0.73rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    text-overflow: ellipsis;
    text-transform: none;
    white-space: nowrap;
  }

  .workspace-count {
    color: var(--text-faint);
    font-size: 0.66rem;
  }

  .live-section .workspace-heading {
    color: var(--accent-hover);
  }

  h3 {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin-top: 0.45rem;
    padding: 0 0.3rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .count {
    background: var(--bg-tertiary);
    padding: 0.08rem 0.4rem;
    border-radius: 4px;
    font-size: 0.65rem;
    color: var(--text-muted);
  }

  .session-item {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    align-items: stretch;
    gap: 0.2rem;
    padding: 0.1rem 0.2rem;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    transition: background-color var(--transition), border-color var(--transition), color var(--transition);
  }

  .session-item:hover,
  .session-item.active {
    background: var(--bg-card);
    border-color: var(--border);
  }

  .session-item.active {
    border-left-color: var(--accent);
  }

  .session-item.working {
    border-left-color: var(--accent);
  }

  .session-item.needs-action {
    border-color: var(--warning);
  }

  .session-open {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex: 1;
    min-width: 0;
    padding: 0.55rem 0.35rem;
    background: transparent;
    border: none;
    border-radius: var(--radius-xs);
    color: inherit;
    text-align: left;
    cursor: pointer;
  }

  .session-open:hover {
    color: var(--text);
  }

  .session-status {
    width: 8px;
    height: 8px;
    flex: 0 0 8px;
    margin-left: 0.15rem;
    border: 1px solid var(--text-faint);
    border-radius: 50%;
  }

  .session-status.needs-action {
    background: var(--warning);
    border-color: var(--warning);
  }

  .activity-spinner {
    display: block;
    width: 8px;
    height: 8px;
    border: 1.5px solid var(--accent);
    border-right-color: transparent;
    border-radius: 50%;
    animation: spin 0.75s linear infinite;
  }

  .session-info {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    flex: 1;
    min-width: 0;
  }

  .session-title {
    display: block;
    width: 100%;
    min-width: 0;
    padding: 0;
    overflow: hidden;
    background: transparent;
    border: none;
    color: var(--text-secondary);
    font-size: 0.88rem;
    font-weight: 500;
    line-height: 1.35;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .session-title:hover {
    color: var(--accent-hover);
  }

  .session-title.expanded {
    overflow: visible;
    text-overflow: clip;
    white-space: normal;
    overflow-wrap: anywhere;
  }

  .session-item.active .session-title,
  .session-item.working .session-title {
    color: var(--text);
  }

  .session-meta {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }

  .menu-toggle {
    align-self: center;
    background: transparent;
    border: none;
    color: var(--text-faint);
    padding: 0.3rem;
    border-radius: var(--radius-xs);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0.55;
    transition: opacity var(--transition), color var(--transition), background-color var(--transition);
  }

  .session-item:hover .menu-toggle,
  .session-item.active .menu-toggle,
  .session-item.needs-action .menu-toggle {
    opacity: 1;
  }

  .menu-toggle:hover {
    color: var(--text);
    background: var(--bg-hover);
  }

  .ctx-menu {
    position: absolute;
    right: 0.35rem;
    top: 2.5rem;
    z-index: 50;
    background: var(--bg-card);
    border: 1px solid var(--border-bright);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow-lg);
    padding: 0.3rem;
    min-width: 150px;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }

  .ctx-item {
    padding: 0.45rem 0.65rem;
    background: transparent;
    border: none;
    border-radius: var(--radius-xs);
    color: var(--text-secondary);
    font-size: 0.82rem;
    cursor: pointer;
    text-align: left;
    width: 100%;
  }

  .ctx-item:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .ctx-item.danger:hover {
    color: var(--danger);
    background: var(--danger-subtle);
  }

  .approval-inline {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 0.15rem 0.5rem;
    width: 100%;
    padding: 0.55rem 0.35rem 0.6rem 1.5rem;
    border-top: 1px solid var(--border-subtle);
  }

  .approval-inline-label {
    color: var(--warning);
    font-size: 0.76rem;
    font-weight: 600;
  }

  .approval-inline-description {
    grid-column: 1 / -1;
    color: var(--text-muted);
    font-size: 0.72rem;
  }

  .approval-inline-actions {
    display: flex;
    gap: 0.35rem;
    grid-column: 2;
    grid-row: 1;
  }

  .approval-inline-actions button {
    padding: 0.25rem 0.45rem;
    border-radius: var(--radius-xs);
    font-size: 0.72rem;
  }

  .approval-deny {
    color: var(--text-muted);
    background: transparent;
    border: 1px solid var(--border);
  }

  .approval-accept {
    color: var(--bg);
    background: var(--warning);
    border: 1px solid var(--warning);
  }

  .rename-form {
    display: flex;
    gap: 0.3rem;
    padding: 0.4rem;
    background: var(--bg-tertiary);
    border-radius: var(--radius-sm);
    border: 1px solid var(--accent);
  }

  .rename-form input {
    flex: 1;
    font-size: 0.85rem;
  }

  .btn-delete.confirming {
    width: 100%;
    padding: 0.3rem;
    color: var(--danger);
    background: var(--danger-subtle);
    border: 1px solid var(--danger);
    font-size: 0.75rem;
  }

  .error {
    color: var(--danger);
    padding: 0.58rem 0.75rem;
    background: var(--danger-subtle);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .empty-state {
    text-align: center;
    padding: 2rem 1rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.4rem;
  }

  .skeleton-header {
    height: 20px;
    width: 80px;
    margin: 0.5rem 0.25rem;
  }

  .skeleton-item {
    height: 48px;
    width: 100%;
  }
</style>
