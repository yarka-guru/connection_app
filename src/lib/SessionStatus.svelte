<script>
const { connectionStatus = 'disconnected', statusMessage = '' } = $props()

const STATUS_CONFIGS = {
  disconnected: {
    label: 'Ready',
    color: '#71717a',
    bgColor: 'rgba(113, 113, 122, 0.1)',
    borderColor: 'rgba(113, 113, 122, 0.2)',
    icon: 'circle',
  },
  connecting: {
    label: 'Connecting',
    color: '#fbbf24',
    bgColor: 'rgba(251, 191, 36, 0.1)',
    borderColor: 'rgba(251, 191, 36, 0.2)',
    icon: 'loading',
  },
  connected: {
    label: 'Connected',
    color: '#34d399',
    bgColor: 'rgba(52, 211, 153, 0.1)',
    borderColor: 'rgba(52, 211, 153, 0.2)',
    icon: 'check',
  },
}

const statusConfig = $derived(STATUS_CONFIGS[connectionStatus])
</script>

<div class="status-bar" role="status" aria-live="polite" style="--status-color: {statusConfig.color}; --status-bg: {statusConfig.bgColor}; --status-border: {statusConfig.borderColor}">
  <div class="status-badge">
    <div class="status-icon">
      {#if statusConfig.icon === 'loading'}
        <svg class="spinning" width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path d="M7 1v2M7 11v2M1 7h2M11 7h2M2.76 2.76l1.41 1.41M9.83 9.83l1.41 1.41M2.76 11.24l1.41-1.41M9.83 4.17l1.41-1.41" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
        </svg>
      {:else if statusConfig.icon === 'check'}
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <path d="M3 7l3 3 5-6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      {:else}
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <circle cx="7" cy="7" r="3" fill="currentColor"/>
        </svg>
      {/if}
    </div>
    <span class="status-label">{statusConfig.label}</span>
  </div>

  {#if statusMessage}
    <p class="status-detail">{statusMessage}</p>
  {/if}
</div>

<style>
  .status-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: var(--status-bg);
    border: 1px solid var(--status-border);
    border-radius: 12px;
    transition: background-color 0.3s ease, border-color 0.3s ease;
  }

  .status-badge {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-icon {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--status-color);
  }

  .spinning {
    animation: spin 1.5s linear infinite;
    will-change: transform;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .status-label {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--status-color);
  }

  .status-detail {
    margin: 0;
    font-size: 0.8rem;
    color: #9e9ea7;
  }
</style>
