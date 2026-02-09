<script>
  let {
    updateInfo = null,
    isUpdating = false,
    onInstall,
    onDismiss
  } = $props()

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
        <button class="btn-dismiss" onclick={handleDismiss} title="Dismiss">
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
    background: linear-gradient(135deg, rgba(99, 102, 241, 0.15) 0%, rgba(139, 92, 246, 0.15) 100%);
    border: 1px solid rgba(99, 102, 241, 0.3);
    border-radius: 14px;
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
    background: rgba(99, 102, 241, 0.2);
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #a5b4fc;
    flex-shrink: 0;
  }

  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid rgba(165, 180, 252, 0.3);
    border-top-color: #a5b4fc;
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
    color: #e4e4e7;
  }

  .update-message strong {
    color: #a5b4fc;
  }

  .current-version {
    font-size: 0.7rem;
    color: #71717a;
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
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-install:hover {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.3);
  }

  .btn-dismiss {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: #71717a;
    cursor: pointer;
    border-radius: 6px;
    transition: all 0.2s;
  }

  .btn-dismiss:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }
</style>
