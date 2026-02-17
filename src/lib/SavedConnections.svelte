<script>
import CopyButton from './CopyButton.svelte'
import { maskPassword } from './utils.js'

const {
  savedConnections = [],
  activeConnections = [],
  projects = [],
  connectingId = null,
  onConnect,
  onDisconnect,
  onDelete,
} = $props()

let expandedId = $state(null)

function getProjectName(projectKey) {
  const project = projects.find((p) => p.key === projectKey)
  return project?.name || projectKey
}

function getActiveConnection(savedConnection) {
  return activeConnections.find(
    (ac) =>
      ac.savedConnectionId === savedConnection.id ||
      (ac.projectKey === savedConnection.projectKey &&
        ac.profile === savedConnection.profile),
  )
}

function formatLastUsed(timestamp) {
  if (!timestamp) return 'Never'
  const date = new Date(parseInt(timestamp, 10))
  const now = new Date()
  const diff = now - date

  if (diff < 60000) return 'Just now'
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`
  if (diff < 604800000) return `${Math.floor(diff / 86400000)}d ago`

  return date.toLocaleDateString()
}

function toggleExpand(id) {
  expandedId = expandedId === id ? null : id
}

function handleConnect(connection) {
  onConnect?.(connection)
}

function handleDisconnect(activeConn) {
  onDisconnect?.(activeConn.id)
}

function handleDelete(connection) {
  onDelete?.(connection)
}

function handleHeaderKeydown(e, activeConn, connectionId) {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault()
    if (activeConn) toggleExpand(connectionId)
  }
}
</script>

{#if savedConnections.length > 0}
  <div class="saved-connections-card">
    <div class="card-header">
      <div class="header-left">
        <div class="card-icon">
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
            <path d="M5 3h10a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V5a2 2 0 012-2z" stroke="currentColor" stroke-width="1.5"/>
            <path d="M7 8l2 2 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>
        <span class="card-title">Connections</span>
      </div>
      {#if activeConnections.length > 0}
        <span class="active-count">{activeConnections.length} active</span>
      {/if}
    </div>

    <div class="connections-list">
      {#each savedConnections as connection (connection.id)}
        {@const activeConn = getActiveConnection(connection)}
        {@const isConnecting = connectingId === connection.id}
        <div class="connection-item" class:active={activeConn} class:connecting={isConnecting} class:expanded={expandedId === connection.id}>
          <div
            class="connection-header"
            role="button"
            tabindex="0"
            onclick={() => activeConn && toggleExpand(connection.id)}
            onkeydown={(e) => handleHeaderKeydown(e, activeConn, connection.id)}
          >
            {#if isConnecting}
              <div class="connection-status">
                <span class="connecting-spinner"></span>
              </div>
            {:else if activeConn}
              <div class="connection-status">
                <span class="status-dot"></span>
              </div>
            {/if}
            <div class="connection-info">
              <div class="connection-name-row">
                <span class="connection-name">{connection.name}</span>
                {#if activeConn}
                  <span class="connection-port">:{activeConn.localPort}</span>
                {/if}
              </div>
              {#if isConnecting}
                <span class="connecting-text">Connecting...</span>
              {:else}
                <span class="connection-meta">
                  {getProjectName(connection.projectKey)} / {connection.profile}
                </span>
                {#if !activeConn}
                  <span class="connection-last-used">
                    Last used: {formatLastUsed(connection.lastUsedAt)}
                  </span>
                {/if}
              {/if}
            </div>
            <div class="connection-actions">
              {#if activeConn}
                <button
                  class="btn-expand"
                  disabled={!!connectingId}
                  aria-label={expandedId === connection.id ? 'Collapse credentials' : 'Show credentials'}
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" class:rotated={expandedId === connection.id}>
                    <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
                <button
                  class="btn-disconnect"
                  disabled={!!connectingId}
                  onclick={(e) => { e.stopPropagation(); handleDisconnect(activeConn); }}
                  aria-label="Disconnect {connection.name}"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                    <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                  </svg>
                </button>
              {:else}
                <button
                  class="btn-connect"
                  disabled={!!connectingId}
                  onclick={(e) => { e.stopPropagation(); handleConnect(connection); }}
                  aria-label="Connect to {connection.name}"
                >
                  {#if isConnecting}
                    <span class="btn-spinner"></span>
                  {:else}
                    <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                      <path d="M5 3l8 5-8 5V3z" fill="currentColor"/>
                    </svg>
                  {/if}
                </button>
                <button
                  class="btn-delete"
                  disabled={!!connectingId}
                  onclick={(e) => { e.stopPropagation(); handleDelete(connection); }}
                  aria-label="Delete {connection.name}"
                >
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                    <path d="M2.5 5h11M6 5V3.5a.5.5 0 01.5-.5h3a.5.5 0 01.5.5V5M12 5v8.5a1 1 0 01-1 1H5a1 1 0 01-1-1V5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
              {/if}
            </div>
          </div>

          {#if expandedId === connection.id && activeConn?.connectionInfo}
            <div class="connection-details">
              <div class="detail-row">
                <span class="detail-label">Host</span>
                <code class="detail-value">{activeConn.connectionInfo.host}</code>
                <CopyButton value={activeConn.connectionInfo.host} label="Copy host" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Port</span>
                <code class="detail-value">{activeConn.connectionInfo.port}</code>
                <CopyButton value={String(activeConn.connectionInfo.port)} label="Copy port" />
              </div>
              <div class="detail-row">
                <span class="detail-label">User</span>
                <code class="detail-value">{activeConn.connectionInfo.username}</code>
                <CopyButton value={activeConn.connectionInfo.username} label="Copy username" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Password</span>
                <code class="detail-value password">{maskPassword(activeConn.connectionInfo.password)}</code>
                <CopyButton value={activeConn.connectionInfo.password} label="Copy password" />
              </div>
              <div class="detail-row">
                <span class="detail-label">Database</span>
                <code class="detail-value">{activeConn.connectionInfo.database}</code>
                <CopyButton value={activeConn.connectionInfo.database} label="Copy database" />
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  .saved-connections-card {
    background: var(--glass-bg);
    -webkit-backdrop-filter: var(--glass-blur);
    backdrop-filter: var(--glass-blur);
    border: 1px solid var(--glass-border);
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
    background: linear-gradient(135deg, rgba(251, 191, 36, 0.2) 0%, rgba(245, 158, 11, 0.2) 100%);
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #fbbf24;
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .active-count {
    font-size: 0.75rem;
    font-weight: 500;
    color: #34d399;
    background: rgba(52, 211, 153, 0.1);
    padding: 4px 10px;
    border-radius: 12px;
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
    transition: border-color 0.2s;
  }

  .connection-item.active {
    border: 1px solid rgba(52, 211, 153, 0.2);
  }

  .connection-item.connecting {
    border: 1px solid rgba(99, 102, 241, 0.3);
  }

  .connection-header {
    display: flex;
    align-items: center;
    padding: 12px 16px;
    transition: background 0.2s;
  }

  .connection-item.active .connection-header {
    cursor: pointer;
  }

  .connection-item.active .connection-header:hover {
    background: rgba(255, 255, 255, 0.02);
  }

  .connection-status {
    margin-right: 12px;
  }

  .status-dot {
    display: block;
    width: 8px;
    height: 8px;
    background: #34d399;
    border-radius: 50%;
    box-shadow: 0 0 8px rgba(52, 211, 153, 0.5);
    animation: pulse 2s ease-in-out infinite;
    will-change: opacity;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.6; }
  }

  .connection-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
  }

  .connection-name-row {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }

  .connection-name {
    font-size: 0.95rem;
    font-weight: 500;
    color: #e4e4e7;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .connection-port {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.85rem;
    color: #34d399;
    font-weight: 500;
  }

  .connection-meta {
    font-size: 0.75rem;
    color: #a5b4fc;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .connection-last-used {
    font-size: 0.7rem;
    color: #9e9ea7;
  }

  .connection-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .btn-connect, .btn-delete, .btn-disconnect, .btn-expand {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
  }

  .btn-connect {
    color: #34d399;
  }

  .btn-connect:hover {
    background: rgba(52, 211, 153, 0.1);
    border-color: rgba(52, 211, 153, 0.3);
  }

  .btn-connect:active {
    transform: var(--press-scale);
  }

  .btn-delete {
    color: #71717a;
  }

  .btn-delete:hover {
    color: #f87171;
    background: rgba(239, 68, 68, 0.1);
    border-color: rgba(239, 68, 68, 0.3);
  }

  .btn-disconnect {
    color: #f87171;
  }

  .btn-disconnect:hover {
    background: rgba(239, 68, 68, 0.1);
    border-color: rgba(239, 68, 68, 0.3);
  }

  .btn-expand {
    color: #71717a;
    border: none;
  }

  .btn-expand:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }

  .btn-expand svg {
    transition: transform 0.2s;
  }

  .btn-expand svg.rotated {
    transform: rotate(180deg);
  }

  .btn-connect:disabled, .btn-delete:disabled, .btn-disconnect:disabled, .btn-expand:disabled {
    opacity: 0.4;
    cursor: not-allowed;
    pointer-events: none;
  }

  .connecting-spinner {
    display: block;
    width: 8px;
    height: 8px;
    border: 1.5px solid rgba(99, 102, 241, 0.3);
    border-top-color: #6366f1;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    will-change: transform;
  }

  .connecting-text {
    font-size: 0.75rem;
    color: #a5b4fc;
    animation: fadeIn 0.3s ease-out;
  }

  .btn-spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 1.5px solid rgba(52, 211, 153, 0.3);
    border-top-color: #34d399;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    will-change: transform;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  @keyframes fadeIn {
    from { opacity: 0; }
    to { opacity: 1; }
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
    color: #9e9ea7;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .detail-value {
    flex: 1;
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.8rem;
    color: #a5b4fc;
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .detail-value.password {
    color: #fbbf24;
    letter-spacing: 0.1em;
  }
</style>
