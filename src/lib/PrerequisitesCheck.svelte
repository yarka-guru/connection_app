<script>
import { trapFocus } from './utils.js'

const { prerequisites = [], onDismiss, onOpenUrl } = $props()

function handleKeydown(e) {
  if (e.key === 'Escape') {
    onDismiss?.()
  }
}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="prerequisites-modal" role="dialog" aria-label="Missing prerequisites" tabindex="-1" onkeydown={handleKeydown}>
  <div class="modal-content" use:trapFocus>
    <div class="modal-header">
      <div class="warning-icon">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none">
          <path d="M12 9v4M12 17h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
        </svg>
      </div>
      <h2>Missing Prerequisites</h2>
      <p>The following tools are required to use this app:</p>
    </div>

    <div class="prerequisites-list">
      {#each prerequisites as prereq}
        <div class="prereq-item" class:installed={prereq.installed} class:missing={!prereq.installed}>
          <div class="prereq-status">
            {#if prereq.installed}
              <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
                <path d="M16.707 5.293a1 1 0 010 1.414l-8 8a1 1 0 01-1.414 0l-4-4a1 1 0 011.414-1.414L8 12.586l7.293-7.293a1 1 0 011.414 0z" fill="currentColor"/>
              </svg>
            {:else}
              <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
                <path d="M4.293 4.293a1 1 0 011.414 0L10 8.586l4.293-4.293a1 1 0 111.414 1.414L11.414 10l4.293 4.293a1 1 0 01-1.414 1.414L10 11.414l-4.293 4.293a1 1 0 01-1.414-1.414L8.586 10 4.293 5.707a1 1 0 010-1.414z" fill="currentColor"/>
              </svg>
            {/if}
          </div>
          <div class="prereq-info">
            <div class="prereq-name">{prereq.name}</div>
            {#if prereq.installed}
              <div class="prereq-version">{prereq.version || 'Installed'}</div>
            {:else}
              <div class="prereq-actions">
                {#if prereq.installCommand}
                  <code class="install-command">{prereq.installCommand}</code>
                {/if}
                <button class="link-btn" onclick={() => onOpenUrl?.(prereq.installUrl)}>
                  Installation Guide
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                    <path d="M3.5 8.5l5-5M8.5 3.5H4.5M8.5 3.5v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
              </div>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    <div class="modal-footer">
      <button class="btn-continue" onclick={onDismiss}>
        Continue Anyway
      </button>
    </div>
  </div>
</div>

<style>
  .prerequisites-modal {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.8);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 24px;
  }

  .modal-content {
    background: rgba(26, 26, 46, 0.85);
    -webkit-backdrop-filter: var(--glass-blur-heavy);
    backdrop-filter: var(--glass-blur-heavy);
    border: 1px solid var(--glass-border);
    border-radius: 20px;
    padding: 28px;
    max-width: 440px;
    width: 100%;
    box-shadow: var(--glass-inner-glow), var(--glass-shadow);
    animation: slideUp 0.3s ease-out;
  }

  @keyframes slideUp {
    from {
      opacity: 0;
      transform: translateY(20px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .modal-header {
    text-align: center;
    margin-bottom: 24px;
  }

  .warning-icon {
    width: 48px;
    height: 48px;
    background: rgba(251, 191, 36, 0.15);
    border-radius: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #fbbf24;
    margin: 0 auto 16px;
  }

  .modal-header h2 {
    margin: 0 0 8px;
    font-size: 1.25rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .modal-header p {
    margin: 0;
    font-size: 0.875rem;
    color: #9e9ea7;
  }

  .prerequisites-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 24px;
  }

  .prereq-item {
    display: flex;
    gap: 12px;
    padding: 14px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 12px;
  }

  .prereq-item.installed {
    border-color: rgba(34, 197, 94, 0.2);
  }

  .prereq-item.missing {
    border-color: rgba(239, 68, 68, 0.2);
    background: rgba(239, 68, 68, 0.05);
  }

  .prereq-status {
    flex-shrink: 0;
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .prereq-item.installed .prereq-status {
    color: #22c55e;
  }

  .prereq-item.missing .prereq-status {
    color: #ef4444;
  }

  .prereq-info {
    flex: 1;
    min-width: 0;
  }

  .prereq-name {
    font-weight: 500;
    color: #e4e4e7;
    margin-bottom: 4px;
  }

  .prereq-version {
    font-size: 0.75rem;
    color: #22c55e;
  }

  .prereq-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .install-command {
    font-size: 0.75rem;
    background: rgba(0, 0, 0, 0.3);
    padding: 6px 10px;
    border-radius: 6px;
    color: #a5b4fc;
    font-family: ui-monospace, monospace;
  }

  .link-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 0.75rem;
    color: #6366f1;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    text-decoration: none;
  }

  .link-btn:hover {
    color: #818cf8;
    text-decoration: underline;
  }

  .modal-footer {
    display: flex;
    justify-content: center;
  }

  .btn-continue {
    padding: 12px 24px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #a1a1aa;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 10px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-continue:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #e4e4e7;
  }

  .btn-continue:active {
    transform: var(--press-scale);
  }
</style>
