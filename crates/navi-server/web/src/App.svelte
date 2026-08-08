<script lang="ts">
  import { isAuthenticated, secret, clearAllOnLogout, showSidebar } from "./lib/stores";
  import { setSecret, clearSecret, checkHealth } from "./lib/api";
  import LoginScreen from "./components/LoginScreen.svelte";
  import MainView from "./components/MainView.svelte";

  let serverOnline = $state(false);
  let healthCheckTimer: ReturnType<typeof setInterval> | null = null;

  // Periodically check if the server is reachable.
  async function pollHealth() {
    serverOnline = await checkHealth();
  }

  $effect(() => {
    if ($isAuthenticated) {
      pollHealth();
      healthCheckTimer = setInterval(pollHealth, 30000);
    } else {
      if (healthCheckTimer) {
        clearInterval(healthCheckTimer);
        healthCheckTimer = null;
      }
    }
  });

  function handleLogin(newSecret: string) {
    setSecret(newSecret);
    secret.set(newSecret);
  }

  function handleLogout() {
    clearSecret();
    secret.set("");
    clearAllOnLogout();
    showSidebar.set(false);
  }
</script>

{#if !$isAuthenticated}
  <LoginScreen onLogin={handleLogin} serverOnline={serverOnline} />
{:else}
  <MainView onLogout={handleLogout} serverOnline={serverOnline} />
{/if}
