<script>
  let {
    connections = [],
    projects = [],
    onDisconnect,
    onDisconnectAll
  } = $props()

  let expandedId = $state(null)
  let copiedField = $state('')

  function getProjectName(projectKey) {
    const project = projects.find(p => p.key === projectKey)
    return project?.name || projectKey
  }

  function toggleExpand(id) {
    expandedId = expandedId === id ? null : id
  }

  async function copyToClipboard(value, field) {
    try {
      await navigator.clipboard.writeText(value)
      copiedField = field
      setTimeout(() => {
        copiedField = ''
      }, 2000)
    } catch (err) {
      console.error('Failed to copy:', err)
    }
  }

  function handleDisconnect(connection) {
    onDisconnect?.(connection.id)
  }

  function handleDisconnectAll() {
    onDisconnectAll?.()
  }

  function maskPassword(password) {
    if (!password) return ''
    return '*'.repeat(Math.min(password.length, 20))
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
            onkeydown={(e) => e.key === 'Enter' && toggleExpand(connection.id)}
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
                title={expandedId === connection.id ? 'Collapse' : 'Expand'}
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none" class:rotated={expandedId === connection.id}>
                  <path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              </button>
              <button
                class="btn-disconnect"
                onclick={(e) => { e.stopPropagation(); handleDisconnect(connection); }}
                title="Disconnect"
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
                <button
                  class="copy-btn"
                  class:copied={copiedField === `host-${connection.id}`}
                  onclick={() => copyToClipboard(connection.connectionInfo.host, `host-${connection.id}`)}
                >
                  {#if copiedField === `host-${connection.id}`}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
                    </svg>
                  {/if}
                </button>
              </div>
              <div class="detail-row">
                <span class="detail-label">Port</span>
                <code class="detail-value">{connection.connectionInfo.port}</code>
                <button
                  class="copy-btn"
                  class:copied={copiedField === `port-${connection.id}`}
                  onclick={() => copyToClipboard(connection.connectionInfo.port, `port-${connection.id}`)}
                >
                  {#if copiedField === `port-${connection.id}`}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
                    </svg>
                  {/if}
                </button>
              </div>
              <div class="detail-row">
                <span class="detail-label">User</span>
                <code class="detail-value">{connection.connectionInfo.username}</code>
                <button
                  class="copy-btn"
                  class:copied={copiedField === `user-${connection.id}`}
                  onclick={() => copyToClipboard(connection.connectionInfo.username, `user-${connection.id}`)}
                >
                  {#if copiedField === `user-${connection.id}`}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
                    </svg>
                  {/if}
                </button>
              </div>
              <div class="detail-row">
                <span class="detail-label">Password</span>
                <code class="detail-value password">{maskPassword(connection.connectionInfo.password)}</code>
                <button
                  class="copy-btn"
                  class:copied={copiedField === `pass-${connection.id}`}
                  onclick={() => copyToClipboard(connection.connectionInfo.password, `pass-${connection.id}`)}
                >
                  {#if copiedField === `pass-${connection.id}`}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
                    </svg>
                  {/if}
                </button>
              </div>
              <div class="detail-row">
                <span class="detail-label">Database</span>
                <code class="detail-value">{connection.connectionInfo.database}</code>
                <button
                  class="copy-btn"
                  class:copied={copiedField === `db-${connection.id}`}
                  onclick={() => copyToClipboard(connection.connectionInfo.database, `db-${connection.id}`)}
                >
                  {#if copiedField === `db-${connection.id}`}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                      <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
                    </svg>
                  {/if}
                </button>
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
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(52, 211, 153, 0.2);
    border-radius: 20px;
    padding: 24px;
    backdrop-filter: blur(10px);
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
    background: linear-gradient(135deg, rgba(52, 211, 153, 0.2) 0%, rgba(16, 185, 129, 0.2) 100%);
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #34d399;
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.7; }
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .btn-disconnect-all {
    padding: 8px 14px;
    font-size: 0.75rem;
    font-weight: 500;
    color: #f87171;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-disconnect-all:hover {
    background: rgba(239, 68, 68, 0.15);
    border-color: rgba(239, 68, 68, 0.3);
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
    background: #34d399;
    border-radius: 50%;
    box-shadow: 0 0 8px rgba(52, 211, 153, 0.5);
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
    color: #e4e4e7;
  }

  .connection-port {
    color: #34d399;
    font-family: 'SF Mono', 'Monaco', monospace;
    font-size: 0.85rem;
  }

  .connection-meta {
    font-size: 0.75rem;
    color: #71717a;
  }

  .connection-actions {
    display: flex;
    gap: 4px;
  }

  .btn-expand, .btn-disconnect {
    width: 28px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-expand {
    color: #71717a;
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

  .btn-disconnect {
    color: #71717a;
  }

  .btn-disconnect:hover {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
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
    color: #71717a;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .detail-value {
    flex: 1;
    font-family: 'SF Mono', 'Monaco', monospace;
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

  .copy-btn {
    width: 26px;
    height: 26px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: #71717a;
    cursor: pointer;
    transition: all 0.2s;
    flex-shrink: 0;
  }

  .copy-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }

  .copy-btn.copied {
    color: #34d399;
  }
</style>
