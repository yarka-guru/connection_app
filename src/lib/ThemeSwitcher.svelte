<script>
const { scheme = 'dark', onSchemeChange } = $props()

const options = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
]
</script>

<div class="scheme-switcher">
  <span class="scheme-label">Appearance</span>
  <div class="segmented-control">
    {#each options as opt}
      <button
        class="segment"
        class:active={scheme === opt.value}
        onclick={() => onSchemeChange?.(opt.value)}
      >
        {#if opt.value === 'light'}
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="5"/>
            <line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/>
            <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/>
            <line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/>
            <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/>
          </svg>
        {:else if opt.value === 'dark'}
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
          </svg>
        {:else}
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/>
          </svg>
        {/if}
        {opt.label}
      </button>
    {/each}
  </div>
</div>

<style>
  .scheme-switcher {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 0 16px;
    border-bottom: 1px solid var(--glass-border);
    margin-bottom: 16px;
  }

  .scheme-label {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
  }

  .segmented-control {
    display: flex;
    gap: 2px;
    background: rgba(var(--glass-rgb), 0.05);
    padding: 3px;
    border-radius: 10px;
    border: 1px solid var(--glass-border);
  }

  .segment {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 6px 12px;
    font-size: 0.8rem;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
    white-space: nowrap;
  }

  .segment:hover {
    color: var(--text-hover);
    background: rgba(var(--glass-rgb), 0.06);
  }

  .segment.active {
    background: rgba(var(--accent-primary-rgb), 0.15);
    color: var(--accent-primary);
    font-weight: 600;
  }
</style>
