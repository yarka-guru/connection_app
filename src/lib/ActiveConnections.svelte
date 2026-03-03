<script>
import CopyButton from './CopyButton.svelte'
import { maskPassword } from './utils.js'

const {
  connections = [],
  projects = [],
  onDisconnect,
  onDisconnectAll,
} = $props()

let expandedId = $state(null)

function getProjectName(projectKey) {
  const project = projects.find((p) => p.key === projectKey)
  return project?.name || projectKey
}

function toggleExpand(id) {
  expandedId = expandedId === id ? null : id
}

function handleDisconnect(connection) {
  onDisconnect?.(connection.id)
}

function handleDisconnectAll() {
  onDisconnectAll?.()
}

function handleHeaderKeydown(e, connectionId) {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault()
    toggleExpand(connectionId)
  }
}
</script>

{#if connections.length > 0}
  <div class="active-connections-card">
    <div class="card-header">
      <div class="header-left">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
            <circle cx="10" cy="10" r="3" fill="currentColor"/>
            <circle cx="10" cy="10" r="7" stroke="currentColor" stroke-width="1.5" stroke-dasharray="4 2"/>
          </svg>
        </div>
        <span class="card-title">Active Connections ({connections.length})</span>
      </div>
      {#if connections.length > 1}
        <button class="btn-disconnect-all" onclick={handleDisconnectAll}>
          Disconnect All
        </button>
      {/if}
    </div>

    <div class="connections-list">
      {#each connections as connection (connection.id)}
        <div class="connection-item" class:expanded={expandedId === connection.id}>
          <div
            class="connection-header"
            role="button"
            tabindex="0"
            onclick={() => toggleExpand(connection.id)}
            onkeydown={(e) => handleHeaderKeydown(e, connection.id)}
          >
            <div class="connection-status">
              <span class="status-dot"></span>
            </div>
            <div class="connection-info">
              <span class="connection-name">
                {connection.profile}
                <span class="connection-port">:{connection.localPort}</span>
              </span>
              <span class="connection-meta">
                {getProjectName(connection.projectKey)}
              </span>
            </div>
            <div class="connection-actions">
              <button
                class="btn-expand"
                aria-label={expandedId === connection.id ? 'Collapse credentials' : 'Show credentials'}
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none" class:rotated={expandedId === connection.id}>
                  <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              </button>
              <button
                class="btn-disconnect"
                onclick={(e) => { e.stopPropagation(); handleDisconnect(connection); }}
                aria-label="Disconnect {connection.profile}"
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                  <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                </svg>
              </button>
            </div>
          </div>

          {#if expandedId === connection.id && connection.connectionInfo}
            <div class="connection-details">
              <div class="detail-row">
                <span class="detail-label">Host</span>
                <code class="detail-value">{connection.connectionInfo.host}</code>
                <CopyButton value={connection.connectionInfo.host} label="Copy host" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Port</span>
                <code class="detail-value">{connection.connectionInfo.port}</code>
                <CopyButton value={String(connection.connectionInfo.port)} label="Copy port" />
              </div>
              <div class="detail-row">
                <span class="detail-label">User</span>
                <code class="detail-value">{connection.connectionInfo.username}</code>
                <CopyButton value={connection.connectionInfo.username} label="Copy username" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Password</span>
                <code class="detail-value password">{maskPassword(connection.connectionInfo.password)}</code>
                <CopyButton value={connection.connectionInfo.password} label="Copy password" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Database</span>
                <code class="detail-value">{connection.connectionInfo.database}</code>
                <CopyButton value={connection.connectionInfo.database} label="Copy database" />
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .active-connections-card {
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid rgba(var(--accent-secondary-rgb), 0.3);
    border-radius: 20px;
    padding: 24px;
    box-shadow: var(--glass-inner-glow);
  }

  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .card-icon {
    width: 36px;
    height: 36px;
    background: linear-gradient(135deg, rgba(var(--accent-secondary-rgb), 0.2) 0%, rgba(var(--accent-secondary-rgb), 0.15) 100%);
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-secondary);
    animation: pulse 2s ease-in-out infinite;
    will-change: opacity;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.7; }
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .btn-disconnect-all {
    padding: 8px 14px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--color-error-soft);
    background: rgba(var(--color-error-rgb), 0.1);
    border: 1px solid rgba(var(--color-error-rgb), 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s;
  }

  .btn-disconnect-all:hover {
    background: rgba(var(--color-error-rgb), 0.15);
    border-color: rgba(var(--color-error-rgb), 0.3);
  }

  .btn-disconnect-all:active {
    transform: var(--press-scale);
  }

  .connections-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .connection-item {
    background: rgba(0, 0, 0, 0.2);
    border-radius: 12px;
    overflow: hidden;
  }

  .connection-item.expanded {
    background: rgba(0, 0, 0, 0.3);
  }

  .connection-header {
    display: flex;
    align-items: center;
    padding: 12px 16px;
    cursor: pointer;
    transition: background 0.2s;
  }

  .connection-header:hover {
    background: rgba(255, 255, 255, 0.02);
  }

  .connection-status {
    margin-right: 12px;
  }

  .status-dot {
    display: block;
    width: 8px;
    height: 8px;
    background: var(--accent-secondary);
    border-radius: 50%;
    box-shadow: 0 0 8px rgba(var(--accent-secondary-rgb), 0.5);
  }

  .connection-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
  }

  .connection-name {
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-primary);
  }

  .connection-port {
    color: var(--accent-secondary);
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.85rem;
  }

  .connection-meta {
    font-size: 0.75rem;
    color: var(--text-secondary);
  }

  .connection-actions {
    display: flex;
    gap: 4px;
  }

  .btn-expand, .btn-disconnect {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-expand {
    color: var(--text-muted);
  }

  .btn-expand:hover {
    background: rgba(var(--glass-rgb), 0.05);
    color: var(--text-hover);
  }

  .btn-expand svg {
    transition: transform 0.2s;
  }

  .btn-expand svg.rotated {
    transform: rotate(180deg);
  }

  .btn-disconnect {
    color: var(--text-muted);
  }

  .btn-disconnect:hover {
    background: rgba(var(--color-error-rgb), 0.1);
    color: var(--color-error-soft);
  }

  .connection-details {
    padding: 0 16px 16px;
    animation: slideDown 0.2s ease-out;
  }

  @keyframes slideDown {
    from {
      opacity: 0;
      transform: translateY(-8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .detail-row {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.02);
    border-radius: 6px;
    margin-bottom: 4px;
  }

  .detail-row:last-child {
    margin-bottom: 0;
  }

  .detail-label {
    width: 70px;
    font-size: 0.7rem;
    font-weight: 500;
    color: var(--text-secondary);
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .detail-value {
    flex: 1;
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.8rem;
    color: var(--accent-primary-light);
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .detail-value.password {
    color: var(--accent-primary);
    letter-spacing: 0.1em;
  }
</style>
