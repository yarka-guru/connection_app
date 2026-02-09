<script>
  let { connectionInfo = null } = $props()

  let copiedField = $state('')
  let showPassword = $state(false)

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

  function copyConnectionString() {
    if (!connectionInfo) return
    const connStr = `postgresql://${connectionInfo.username}:${encodeURIComponent(connectionInfo.password)}@${connectionInfo.host}:${connectionInfo.port}/${connectionInfo.database}`
    copyToClipboard(connStr, 'connStr')
  }

  function copyPsqlCommand() {
    if (!connectionInfo) return
    const cmd = `PGPASSWORD='${connectionInfo.password}' psql -h ${connectionInfo.host} -p ${connectionInfo.port} -U ${connectionInfo.username} -d ${connectionInfo.database}`
    copyToClipboard(cmd, 'psql')
  }

  function togglePassword() {
    showPassword = !showPassword
  }

  function maskPassword(password) {
    if (!password) return ''
    return '*'.repeat(Math.min(password.length, 20))
  }
</script>

{#if connectionInfo}
  <div class="credentials-card">
    <div class="card-header">
      <div class="card-icon">
        <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
          <rect x="3" y="8" width="14" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
          <path d="M6 8V6a4 4 0 118 0v2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          <circle cx="10" cy="12.5" r="1.5" fill="currentColor"/>
        </svg>
      </div>
      <span class="card-title">Connection Details</span>
    </div>

    <div class="credentials-list">
      <div class="credential-item">
        <div class="credential-info">
          <span class="credential-label">Host</span>
          <code class="credential-value">{connectionInfo.host}</code>
        </div>
        <button
          class="copy-btn"
          class:copied={copiedField === 'host'}
          onclick={() => copyToClipboard(connectionInfo.host, 'host')}
        >
          {#if copiedField === 'host'}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          {:else}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
              <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
            </svg>
          {/if}
        </button>
      </div>

      <div class="credential-item">
        <div class="credential-info">
          <span class="credential-label">Port</span>
          <code class="credential-value">{connectionInfo.port}</code>
        </div>
        <button
          class="copy-btn"
          class:copied={copiedField === 'port'}
          onclick={() => copyToClipboard(connectionInfo.port, 'port')}
        >
          {#if copiedField === 'port'}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          {:else}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
              <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
            </svg>
          {/if}
        </button>
      </div>

      <div class="credential-item">
        <div class="credential-info">
          <span class="credential-label">Database</span>
          <code class="credential-value">{connectionInfo.database}</code>
        </div>
        <button
          class="copy-btn"
          class:copied={copiedField === 'database'}
          onclick={() => copyToClipboard(connectionInfo.database, 'database')}
        >
          {#if copiedField === 'database'}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          {:else}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
              <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
            </svg>
          {/if}
        </button>
      </div>

      <div class="credential-item">
        <div class="credential-info">
          <span class="credential-label">Username</span>
          <code class="credential-value">{connectionInfo.username}</code>
        </div>
        <button
          class="copy-btn"
          class:copied={copiedField === 'username'}
          onclick={() => copyToClipboard(connectionInfo.username, 'username')}
        >
          {#if copiedField === 'username'}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          {:else}
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
              <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
            </svg>
          {/if}
        </button>
      </div>

      <div class="credential-item password-item">
        <div class="credential-info">
          <span class="credential-label">Password</span>
          <code class="credential-value password">
            {showPassword ? connectionInfo.password : maskPassword(connectionInfo.password)}
          </code>
        </div>
        <div class="password-actions">
          <button
            class="icon-btn"
            onclick={togglePassword}
            title={showPassword ? 'Hide password' : 'Show password'}
          >
            {#if showPassword}
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M2 2l12 12M6.5 6.5a2 2 0 002.8 2.8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                <path d="M4.5 4.5C3 5.5 2 7 2 8s2 4 6 4c1 0 2-.2 2.8-.5M8 4c4 0 6 2.5 6 4 0 .8-.5 1.7-1.3 2.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
              </svg>
            {:else}
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <ellipse cx="8" cy="8" rx="6" ry="4" stroke="currentColor" stroke-width="1.5"/>
                <circle cx="8" cy="8" r="2" stroke="currentColor" stroke-width="1.5"/>
              </svg>
            {/if}
          </button>
          <button
            class="copy-btn"
            class:copied={copiedField === 'password'}
            onclick={() => copyToClipboard(connectionInfo.password, 'password')}
          >
            {#if copiedField === 'password'}
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M3 8l3 3 7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            {:else}
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <rect x="5" y="5" width="9" height="9" rx="2" stroke="currentColor" stroke-width="1.5"/>
                <path d="M11 5V3.5A1.5 1.5 0 009.5 2h-6A1.5 1.5 0 002 3.5v6A1.5 1.5 0 003.5 11H5" stroke="currentColor" stroke-width="1.5"/>
              </svg>
            {/if}
          </button>
        </div>
      </div>
    </div>

    <div class="quick-actions">
      <span class="section-label">Quick Copy</span>
      <div class="action-buttons">
        <button
          class="action-btn"
          class:copied={copiedField === 'connStr'}
          onclick={copyConnectionString}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <path d="M4 8h8M8 4v8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            <rect x="2" y="2" width="12" height="12" rx="3" stroke="currentColor" stroke-width="1.5"/>
          </svg>
          <span>{copiedField === 'connStr' ? 'Copied!' : 'Connection URL'}</span>
        </button>
        <button
          class="action-btn"
          class:copied={copiedField === 'psql'}
          onclick={copyPsqlCommand}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
            <rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" stroke-width="1.5"/>
            <path d="M5 7l2 2-2 2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            <path d="M9 11h2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
          <span>{copiedField === 'psql' ? 'Copied!' : 'psql Command'}</span>
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .credentials-card {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 20px;
    padding: 24px;
    backdrop-filter: blur(10px);
    animation: fadeIn 0.3s ease-out;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 20px;
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
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .credentials-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 12px;
    overflow: hidden;
  }

  .credential-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: rgba(255, 255, 255, 0.02);
    transition: background 0.2s;
  }

  .credential-item:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .credential-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow: hidden;
  }

  .credential-label {
    font-size: 0.7rem;
    font-weight: 500;
    color: #71717a;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .credential-value {
    font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Fira Code', monospace;
    font-size: 0.875rem;
    color: #a5b4fc;
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .credential-value.password {
    color: #fbbf24;
    letter-spacing: 0.1em;
  }

  .password-actions {
    display: flex;
    gap: 4px;
  }

  .copy-btn, .icon-btn {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    color: #71717a;
    cursor: pointer;
    transition: all 0.2s;
  }

  .copy-btn:hover, .icon-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.15);
    color: #a1a1aa;
  }

  .copy-btn.copied {
    background: rgba(52, 211, 153, 0.1);
    border-color: rgba(52, 211, 153, 0.3);
    color: #34d399;
  }

  .quick-actions {
    margin-top: 20px;
    padding-top: 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
  }

  .section-label {
    display: block;
    font-size: 0.7rem;
    font-weight: 500;
    color: #71717a;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 12px;
  }

  .action-buttons {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }

  .action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 12px 16px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    color: #a1a1aa;
    font-size: 0.8rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s;
  }

  .action-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    border-color: rgba(255, 255, 255, 0.12);
    color: #e4e4e7;
  }

  .action-btn.copied {
    background: rgba(52, 211, 153, 0.1);
    border-color: rgba(52, 211, 153, 0.2);
    color: #34d399;
  }
</style>
