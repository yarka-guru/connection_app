<script>
const { updateInfo = null, isUpdating = false, onInstall, onDismiss } = $props()

function handleInstall() {
  onInstall?.()
}

function handleDismiss() {
  onDismiss?.()
}
</script>

{#if updateInfo?.updateAvailable}
  <div class="update-banner">
    <div class="update-icon">
      {#if isUpdating}
        <div class="spinner"></div>
      {:else}
        <svg width="18" height="18" viewBox="0 0 18 18" fill="none">
          <path d="M9 2v8M9 10l-3-3M9 10l3-3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          <path d="M3 13v1a2 2 0 002 2h8a2 2 0 002-2v-1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
        </svg>
      {/if}
    </div>
    <div class="update-text">
      {#if isUpdating}
        <span class="update-message">Downloading update...</span>
      {:else}
        <span class="update-message">
          Update available: <strong>v{updateInfo.latestVersion}</strong>
        </span>
        <span class="current-version">Current: v{updateInfo.currentVersion}</span>
      {/if}
    </div>
    <div class="update-actions">
      {#if !isUpdating}
        <button class="btn-install" onclick={handleInstall}>
          Install & Restart
        </button>
        <button class="btn-dismiss" onclick={handleDismiss} aria-label="Dismiss update notification">
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <path d="M3 3l8 8M11 3l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
        </button>
      {/if}
    </div>
  </div>
{/if}

<style>
  .update-banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid rgba(212, 168, 83, 0.3);
    border-radius: 14px;
    box-shadow: var(--glass-inner-glow);
    animation: slideIn 0.3s ease-out;
  }

  @keyframes slideIn {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .update-icon {
    width: 32px;
    height: 32px;
    background: rgba(212, 168, 83, 0.2);
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #e2c87a;
    flex-shrink: 0;
  }

  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid rgba(226, 200, 122, 0.3);
    border-top-color: #e2c87a;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .update-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .update-message {
    font-size: 0.875rem;
    color: #d5ddd3;
  }

  .update-message strong {
    color: #e2c87a;
  }

  .current-version {
    font-size: 0.7rem;
    color: #8a9488;
  }

  .update-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .btn-install {
    padding: 8px 16px;
    font-size: 0.8rem;
    font-weight: 600;
    color: white;
    background: linear-gradient(135deg, #d4a853 0%, #7aab6d 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: transform 0.2s, box-shadow 0.2s;
  }

  .btn-install:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(212, 168, 83, 0.3);
  }

  .btn-install:active {
    transform: var(--press-scale);
  }

  .btn-dismiss {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: #6b7d6a;
    cursor: pointer;
    border-radius: 6px;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-dismiss:hover {
    background: rgba(200, 220, 195, 0.05);
    color: #9baa98;
  }
</style>
