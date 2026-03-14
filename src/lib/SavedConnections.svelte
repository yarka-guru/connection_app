<script>
import CopyButton from './CopyButton.svelte'
import { maskPassword, buildConnectionString } from './utils.js'

const {
  savedConnections = [],
  activeConnections = [],
  projects = [],
  connectingId = null,
  activeTab = 'rds',
  connectionHealth = {},
  onConnect,
  onDisconnect,
  onDelete,
  onUpdate,
  onReorder,
  onMoveToGroup,
  onRenameGroup,
  onDeleteGroup,
} = $props()

// Filter saved connections by active tab
const filteredConnections = $derived(
  savedConnections.filter((c) => {
    const project = projects.find((p) => p.key === c.projectKey)
    const ct = project?.connectionType || 'rds'
    return activeTab === 'rds' ? ct === 'rds' : ct === 'service'
  }),
)

// Group connections: derive group names (ordered by first appearance) and grouped map
const groupNames = $derived(() => {
  const seen = new Set()
  const names = []
  for (const c of filteredConnections) {
    const g = c.group || null
    if (g && !seen.has(g)) {
      seen.add(g)
      names.push(g)
    }
  }
  return names
})

const ungroupedConnections = $derived(
  filteredConnections.filter((c) => !c.group),
)

const groupedConnectionsMap = $derived(() => {
  const map = {}
  for (const c of filteredConnections) {
    if (c.group) {
      if (!map[c.group]) map[c.group] = []
      map[c.group].push(c)
    }
  }
  return map
})

let expandedId = $state(null)
let editingId = $state(null)
let editName = $state('')
let collapsedGroups = $state(new Set())
let showNewGroupInput = $state(false)
let newGroupName = $state('')
let editingGroupName = $state(null)
let editGroupNameValue = $state('')
let movingConnectionId = $state(null)
let dragOverGroup = $state(null)

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

function toggleGroupCollapse(groupName) {
  const next = new Set(collapsedGroups)
  if (next.has(groupName)) {
    next.delete(groupName)
  } else {
    next.add(groupName)
  }
  collapsedGroups = next
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

function startEdit(connection, e) {
  e.stopPropagation()
  editingId = connection.id
  editName = connection.name
}

function cancelEdit() {
  editingId = null
  editName = ''
}

function saveEdit(e) {
  e?.preventDefault()
  if (!editingId || !editName.trim()) return
  onUpdate?.(editingId, editName.trim())
  editingId = null
  editName = ''
}

function handleEditKeydown(e) {
  if (e.key === 'Escape') {
    e.stopPropagation()
    cancelEdit()
  }
}

function moveUp(index, list, e) {
  e.stopPropagation()
  if (index === 0) return
  const fullIds = savedConnections.map((c) => c.id)
  const aId = list[index].id
  const bId = list[index - 1].id
  const aIdx = fullIds.indexOf(aId)
  const bIdx = fullIds.indexOf(bId)
  ;[fullIds[bIdx], fullIds[aIdx]] = [fullIds[aIdx], fullIds[bIdx]]
  onReorder?.(fullIds)
}

function moveDown(index, list, e) {
  e.stopPropagation()
  if (index >= list.length - 1) return
  const fullIds = savedConnections.map((c) => c.id)
  const aId = list[index].id
  const bId = list[index + 1].id
  const aIdx = fullIds.indexOf(aId)
  const bIdx = fullIds.indexOf(bId)
  ;[fullIds[aIdx], fullIds[bIdx]] = [fullIds[bIdx], fullIds[aIdx]]
  onReorder?.(fullIds)
}

function getHealthStatus(activeConn) {
  if (!activeConn) return null
  const health = connectionHealth[activeConn.id]
  if (!health) return { status: 'healthy', tooltip: 'Connected' }
  const now = Date.now()
  const elapsed = Math.floor((now - health.lastCheck) / 1000)
  let tooltip
  if (elapsed < 5) {
    tooltip = 'Checked just now'
  } else if (elapsed < 60) {
    tooltip = `Last checked ${elapsed}s ago`
  } else {
    tooltip = `Last checked ${Math.floor(elapsed / 60)}m ago`
  }
  return { status: health.status, tooltip }
}

function handleHeaderKeydown(e, activeConn, connectionId) {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault()
    if (activeConn) toggleExpand(connectionId)
  }
}

// Group management
function handleCreateGroup(e) {
  e?.preventDefault()
  const name = newGroupName.trim()
  if (!name || !movingConnectionId) return
  onMoveToGroup?.(movingConnectionId, name)
  newGroupName = ''
  showNewGroupInput = false
  movingConnectionId = null
}

function handleNewGroupKeydown(e) {
  if (e.key === 'Escape') {
    e.stopPropagation()
    showNewGroupInput = false
    newGroupName = ''
    movingConnectionId = null
  }
}

function startRenameGroup(groupName, e) {
  e.stopPropagation()
  editingGroupName = groupName
  editGroupNameValue = groupName
}

function saveGroupRename(e) {
  e?.preventDefault()
  const newName = editGroupNameValue.trim()
  if (!editingGroupName || !newName) return
  if (newName !== editingGroupName) {
    onRenameGroup?.(editingGroupName, newName)
  }
  editingGroupName = null
  editGroupNameValue = ''
}

function cancelGroupRename() {
  editingGroupName = null
  editGroupNameValue = ''
}

function handleGroupRenameKeydown(e) {
  if (e.key === 'Escape') {
    e.stopPropagation()
    cancelGroupRename()
  }
}

function handleDeleteGroup(groupName, e) {
  e.stopPropagation()
  onDeleteGroup?.(groupName)
}

function startMoveToGroup(connectionId, e) {
  e.stopPropagation()
  movingConnectionId = movingConnectionId === connectionId ? null : connectionId
  showNewGroupInput = false
}

function moveToExistingGroup(groupName, e) {
  e?.stopPropagation()
  if (!movingConnectionId) return
  onMoveToGroup?.(movingConnectionId, groupName)
  movingConnectionId = null
}

function moveToUngrouped(e) {
  e?.stopPropagation()
  if (!movingConnectionId) return
  onMoveToGroup?.(movingConnectionId, null)
  movingConnectionId = null
}

function showCreateNewGroup(e) {
  e?.stopPropagation()
  showNewGroupInput = true
  newGroupName = ''
}

// Drag and drop
function handleDragStart(e, connectionId) {
  e.dataTransfer.setData('text/plain', connectionId)
  e.dataTransfer.effectAllowed = 'move'
}

function handleDragOver(e, groupKey) {
  e.preventDefault()
  e.dataTransfer.dropEffect = 'move'
  dragOverGroup = groupKey
}

function handleDragLeave() {
  dragOverGroup = null
}

function handleDrop(e, groupKey) {
  e.preventDefault()
  dragOverGroup = null
  const connectionId = e.dataTransfer.getData('text/plain')
  if (!connectionId) return
  // groupKey is null for ungrouped, or the group name string
  onMoveToGroup?.(connectionId, groupKey)
}

function handleGroupHeaderKeydown(e, groupName) {
  if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault()
    toggleGroupCollapse(groupName)
  }
}
</script>

{#if filteredConnections.length > 0}
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

    <!-- Move-to-group picker (shown when movingConnectionId is set) -->
    {#if movingConnectionId}
      <div class="move-picker">
        <span class="move-picker-label">Move to group:</span>
        <div class="move-picker-options">
          <button class="move-option" onclick={moveToUngrouped}>General (ungrouped)</button>
          {#each groupNames() as gName}
            <button class="move-option" onclick={(e) => moveToExistingGroup(gName, e)}>{gName}</button>
          {/each}
          {#if showNewGroupInput}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <form class="new-group-form" onsubmit={handleCreateGroup} onkeydown={handleNewGroupKeydown}>
              <input
                type="text"
                class="new-group-input"
                placeholder="New group name..."
                bind:value={newGroupName}
                autofocus
              />
              <button type="submit" class="new-group-btn" disabled={!newGroupName.trim()}>
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                  <path d="M2 6h8M6 2v8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                </svg>
              </button>
            </form>
          {:else}
            <button class="move-option move-option-new" onclick={showCreateNewGroup}>+ New group</button>
          {/if}
        </div>
        <button class="move-cancel" onclick={() => { movingConnectionId = null; showNewGroupInput = false }}>Cancel</button>
      </div>
    {/if}

    <div class="connections-list">
      <!-- Ungrouped connections (General section) -->
      {#if ungroupedConnections.length > 0}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="group-section"
          class:drag-over={dragOverGroup === '__ungrouped__'}
          ondragover={(e) => handleDragOver(e, '__ungrouped__')}
          ondragleave={handleDragLeave}
          ondrop={(e) => handleDrop(e, null)}
        >
          <div
            class="group-header"
            role="button"
            tabindex="0"
            onclick={() => toggleGroupCollapse('__ungrouped__')}
            onkeydown={(e) => handleGroupHeaderKeydown(e, '__ungrouped__')}
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" class="group-chevron" class:collapsed={collapsedGroups.has('__ungrouped__')}>
              <path d="M4 5l3 3 3-3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <span class="group-name">General</span>
            <span class="group-count">{ungroupedConnections.length}</span>
          </div>
          {#if !collapsedGroups.has('__ungrouped__')}
            <div class="group-connections">
              {#each ungroupedConnections as connection, index (connection.id)}
                {@const activeConn = getActiveConnection(connection)}
                {@const isConnecting = connectingId === connection.id}
                {@render connectionItem(connection, activeConn, isConnecting, index, ungroupedConnections)}
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      <!-- Named groups -->
      {#each groupNames() as groupName (groupName)}
        {@const groupConns = groupedConnectionsMap()[groupName] || []}
        {#if groupConns.length > 0}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="group-section"
            class:drag-over={dragOverGroup === groupName}
            ondragover={(e) => handleDragOver(e, groupName)}
            ondragleave={handleDragLeave}
            ondrop={(e) => handleDrop(e, groupName)}
          >
            <div
              class="group-header"
              role="button"
              tabindex="0"
              onclick={() => toggleGroupCollapse(groupName)}
              onkeydown={(e) => handleGroupHeaderKeydown(e, groupName)}
            >
              <svg width="14" height="14" viewBox="0 0 14 14" fill="none" class="group-chevron" class:collapsed={collapsedGroups.has(groupName)}>
                <path d="M4 5l3 3 3-3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              {#if editingGroupName === groupName}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <form class="edit-group-form" onsubmit={saveGroupRename} onclick={(e) => e.stopPropagation()} onkeydown={handleGroupRenameKeydown}>
                  <input
                    type="text"
                    class="edit-group-input"
                    bind:value={editGroupNameValue}
                    autofocus
                    onblur={saveGroupRename}
                  />
                </form>
              {:else}
                <span class="group-name">{groupName}</span>
              {/if}
              <span class="group-count">{groupConns.length}</span>
              <div class="group-actions">
                <button
                  class="btn-group-action"
                  onclick={(e) => startRenameGroup(groupName, e)}
                  aria-label="Rename group {groupName}"
                >
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                    <path d="M9 1l2 2-7 7H2V8l7-7z" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </button>
                <button
                  class="btn-group-action btn-group-delete"
                  onclick={(e) => handleDeleteGroup(groupName, e)}
                  aria-label="Delete group {groupName}"
                >
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                    <path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                  </svg>
                </button>
              </div>
            </div>
            {#if !collapsedGroups.has(groupName)}
              <div class="group-connections">
                {#each groupConns as connection, index (connection.id)}
                  {@const activeConn = getActiveConnection(connection)}
                  {@const isConnecting = connectingId === connection.id}
                  {@render connectionItem(connection, activeConn, isConnecting, index, groupConns)}
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      {/each}
    </div>
  </div>
{/if}

{#snippet connectionItem(connection, activeConn, isConnecting, index, list)}
  <div
    class="connection-item"
    class:active={activeConn}
    class:connecting={isConnecting}
    class:expanded={expandedId === connection.id}
    draggable="true"
    ondragstart={(e) => handleDragStart(e, connection.id)}
  >
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
        {@const health = getHealthStatus(activeConn)}
        <div class="connection-status" title={health?.tooltip}>
          <span class="status-dot" class:healthy={health?.status === 'healthy'} class:degraded={health?.status === 'degraded'} class:unhealthy={health?.status === 'unhealthy'}></span>
        </div>
      {/if}
      <div class="connection-info">
        {#if editingId === connection.id}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <form class="edit-name-form" onsubmit={saveEdit} onclick={(e) => e.stopPropagation()} onkeydown={handleEditKeydown}>
            <input
              type="text"
              class="edit-name-input"
              bind:value={editName}
              autofocus
              onblur={saveEdit}
            />
          </form>
        {:else}
          <div class="connection-name-row">
            <span class="connection-name">{connection.name}</span>
            {#if activeConn}
              <span class="connection-port">:{activeConn.localPort}</span>
            {/if}
          </div>
        {/if}
        {#if isConnecting}
          <span class="connecting-text">Connecting...</span>
        {:else if editingId !== connection.id}
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
          {#if list.length > 1}
            <div class="reorder-buttons">
              <button
                class="btn-reorder"
                disabled={index === 0 || !!connectingId}
                onclick={(e) => moveUp(index, list, e)}
                aria-label="Move up"
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                  <path d="M6 2.5v7M3 5.5l3-3 3 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              </button>
              <button
                class="btn-reorder"
                disabled={index === list.length - 1 || !!connectingId}
                onclick={(e) => moveDown(index, list, e)}
                aria-label="Move down"
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                  <path d="M6 9.5v-7M3 6.5l3 3 3-3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              </button>
            </div>
          {/if}
          <button
            class="btn-move-group"
            disabled={!!connectingId}
            onclick={(e) => startMoveToGroup(connection.id, e)}
            aria-label="Move to group"
            class:active-move={movingConnectionId === connection.id}
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <path d="M2 4h4l1.5-2H12a1 1 0 011 1v8a1 1 0 01-1 1H2a1 1 0 01-1-1V5a1 1 0 011-1z" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
          <button
            class="btn-edit"
            disabled={!!connectingId}
            onclick={(e) => startEdit(connection, e)}
            aria-label="Edit {connection.name}"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <path d="M10.5 1.5l2 2-8 8H2.5v-2l8-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
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
      {@const info = activeConn.connectionInfo}
      <div class="connection-details">
        <div class="detail-row">
          <span class="detail-label">Host</span>
          <code class="detail-value">{info.host}</code>
          <CopyButton value={info.host} label="Copy host" />
        </div>
        <div class="detail-row">
          <span class="detail-label">Port</span>
          <code class="detail-value">{info.port}</code>
          <CopyButton value={String(info.port)} label="Copy port" />
        </div>
        {#if info.connectionType === 'service'}
          {#if info.sshCommand}
            <div class="detail-row ssh-command-row">
              <span class="detail-label">SSH</span>
              <code class="detail-value ssh-command">{info.sshCommand}</code>
              <CopyButton value={info.sshCommand} label="Copy SSH command" />
            </div>
          {/if}
          {#if info.remoteHost}
            <div class="detail-row">
              <span class="detail-label">Remote</span>
              <code class="detail-value">{info.remoteHost}</code>
              <CopyButton value={info.remoteHost} label="Copy remote host" />
            </div>
          {/if}
          {#if info.serviceType}
            <div class="detail-row">
              <span class="detail-label">Service</span>
              <code class="detail-value">{info.serviceType.toUpperCase()}</code>
            </div>
          {/if}
          {#if info.targetType}
            <div class="detail-row">
              <span class="detail-label">Target</span>
              <code class="detail-value">{info.targetType}</code>
            </div>
          {/if}
        {:else}
          {#if info.username}
            <div class="detail-row">
              <span class="detail-label">User</span>
              <code class="detail-value">{info.username}</code>
              <CopyButton value={info.username} label="Copy username" />
            </div>
          {/if}
          {#if info.password}
            <div class="detail-row">
              <span class="detail-label">Password</span>
              <code class="detail-value password">{maskPassword(info.password)}</code>
              <CopyButton value={info.password} label="Copy password" />
            </div>
          {/if}
          {#if info.database}
            <div class="detail-row">
              <span class="detail-label">Database</span>
              <code class="detail-value">{info.database}</code>
              <CopyButton value={info.database} label="Copy database" />
            </div>
          {/if}
          {#if info.username && info.password && info.database}
            <div class="conn-string-row">
              <span class="detail-label">Connect</span>
              <div class="conn-string-formats">
                {#if info.engine === 'mysql'}
                  <div class="format-btn-group">
                    <span class="format-label">mysql</span>
                    <CopyButton value={buildConnectionString({ ...info, localPort: info.port }, 'mysql')} label="Copy mysql command" />
                  </div>
                {:else}
                  <div class="format-btn-group">
                    <span class="format-label">psql</span>
                    <CopyButton value={buildConnectionString({ ...info, localPort: info.port }, 'psql')} label="Copy psql command" />
                  </div>
                {/if}
                <div class="format-btn-group">
                  <span class="format-label">URI</span>
                  <CopyButton value={buildConnectionString({ ...info, localPort: info.port }, 'uri')} label="Copy connection URI" />
                </div>
                <div class="format-btn-group">
                  <span class="format-label">JDBC</span>
                  <CopyButton value={buildConnectionString({ ...info, localPort: info.port }, 'jdbc')} label="Copy JDBC string" />
                </div>
              </div>
            </div>
          {/if}
        {/if}
      </div>
    {/if}
  </div>
{/snippet}

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
    background: linear-gradient(135deg, rgba(var(--accent-primary-rgb), 0.2) 0%, rgba(var(--accent-primary-rgb), 0.15) 100%);
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-primary);
  }

  .card-title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
  }

  .active-count {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--accent-secondary);
    background: rgba(var(--accent-secondary-rgb), 0.1);
    padding: 4px 10px;
    border-radius: 12px;
  }

  .connections-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* Group sections */
  .group-section {
    border-radius: 12px;
    transition: border-color 0.2s, background-color 0.2s;
    border: 1px solid transparent;
  }

  .group-section.drag-over {
    border-color: rgba(var(--accent-primary-rgb), 0.4);
    background: rgba(var(--accent-primary-rgb), 0.05);
  }

  .group-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    cursor: pointer;
    border-radius: 8px;
    transition: background-color 0.2s;
    user-select: none;
  }

  .group-header:hover {
    background: rgba(255, 255, 255, 0.03);
  }

  .group-chevron {
    color: var(--text-muted);
    transition: transform 0.2s;
    flex-shrink: 0;
  }

  .group-chevron.collapsed {
    transform: rotate(-90deg);
  }

  .group-name {
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .group-count {
    font-size: 0.7rem;
    color: var(--text-muted);
    background: rgba(255, 255, 255, 0.05);
    padding: 1px 7px;
    border-radius: 8px;
    margin-left: 2px;
  }

  .group-actions {
    display: flex;
    gap: 2px;
    margin-left: auto;
    opacity: 0;
    transition: opacity 0.2s;
  }

  .group-header:hover .group-actions {
    opacity: 1;
  }

  .btn-group-action {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    color: var(--text-muted);
    padding: 0;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-group-action:hover {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-hover);
  }

  .btn-group-delete:hover {
    color: var(--color-error-soft);
    background: rgba(var(--color-error-rgb), 0.1);
  }

  .edit-group-form {
    display: flex;
    flex: 1;
  }

  .edit-group-input {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--accent-primary);
    border-radius: 6px;
    padding: 2px 8px;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--text-primary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    outline: none;
  }

  .group-connections {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px 0 4px 4px;
    animation: slideDown 0.15s ease-out;
  }

  /* Move-to-group picker */
  .move-picker {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    margin-bottom: 12px;
    background: rgba(var(--accent-primary-rgb), 0.06);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.15);
    border-radius: 10px;
    flex-wrap: wrap;
  }

  .move-picker-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--accent-primary-light);
    white-space: nowrap;
  }

  .move-picker-options {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    flex: 1;
    align-items: center;
  }

  .move-option {
    font-size: 0.72rem;
    padding: 3px 10px;
    border-radius: 6px;
    border: 1px solid rgba(var(--glass-rgb), 0.15);
    background: rgba(0, 0, 0, 0.2);
    color: var(--text-primary);
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s;
    white-space: nowrap;
  }

  .move-option:hover {
    background: rgba(var(--accent-primary-rgb), 0.1);
    border-color: rgba(var(--accent-primary-rgb), 0.3);
  }

  .move-option-new {
    color: var(--accent-primary-light);
    border-style: dashed;
  }

  .new-group-form {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .new-group-input {
    width: 120px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--accent-primary);
    border-radius: 6px;
    padding: 3px 8px;
    font-size: 0.72rem;
    color: var(--text-primary);
    outline: none;
  }

  .new-group-btn {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(var(--accent-primary-rgb), 0.15);
    border: 1px solid rgba(var(--accent-primary-rgb), 0.3);
    border-radius: 6px;
    cursor: pointer;
    color: var(--accent-primary);
    transition: background-color 0.2s;
  }

  .new-group-btn:hover:not(:disabled) {
    background: rgba(var(--accent-primary-rgb), 0.25);
  }

  .new-group-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .move-cancel {
    font-size: 0.72rem;
    padding: 3px 10px;
    border-radius: 6px;
    border: 1px solid rgba(var(--color-error-rgb), 0.2);
    background: transparent;
    color: var(--color-error-soft);
    cursor: pointer;
    transition: background-color 0.2s;
    white-space: nowrap;
  }

  .move-cancel:hover {
    background: rgba(var(--color-error-rgb), 0.1);
  }

  /* Connection items */
  .connection-item {
    background: rgba(0, 0, 0, 0.2);
    border-radius: 12px;
    overflow: hidden;
    transition: border-color 0.2s;
    cursor: grab;
  }

  .connection-item:active {
    cursor: grabbing;
  }

  .connection-item.active {
    border: 1px solid rgba(var(--accent-secondary-rgb), 0.2);
    cursor: default;
  }

  .connection-item.connecting {
    border: 1px solid rgba(var(--accent-primary-rgb), 0.3);
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
    border-radius: 50%;
    will-change: opacity;
  }

  .status-dot.healthy {
    background: var(--accent-secondary);
    box-shadow: 0 0 8px rgba(var(--accent-secondary-rgb), 0.5);
    animation: pulse-healthy 2s ease-in-out infinite;
  }

  .status-dot.degraded {
    background: #d4a853;
    box-shadow: 0 0 8px rgba(212, 168, 83, 0.5);
    animation: pulse-degraded 1.5s ease-in-out infinite;
  }

  .status-dot.unhealthy {
    background: var(--color-error);
    box-shadow: 0 0 8px rgba(var(--color-error-rgb), 0.5);
    animation: none;
  }

  @keyframes pulse-healthy {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.6; }
  }

  @keyframes pulse-degraded {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
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
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .connection-port {
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
    font-size: 0.85rem;
    color: var(--accent-secondary);
    font-weight: 500;
  }

  .connection-meta {
    font-size: 0.75rem;
    color: var(--accent-primary-light);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .connection-last-used {
    font-size: 0.7rem;
    color: var(--text-secondary);
  }

  .connection-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
    align-items: center;
  }

  .btn-connect, .btn-delete, .btn-disconnect, .btn-expand, .btn-edit, .btn-move-group {
    width: 32px;
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid rgba(var(--glass-rgb), 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: background-color 0.2s, border-color 0.2s, color 0.2s;
  }

  .btn-connect {
    color: var(--accent-secondary);
  }

  .btn-connect:hover {
    background: rgba(var(--accent-secondary-rgb), 0.1);
    border-color: rgba(var(--accent-secondary-rgb), 0.3);
  }

  .btn-connect:active {
    transform: var(--press-scale);
  }

  .btn-edit {
    color: var(--text-muted);
  }

  .btn-edit:hover {
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border-color: rgba(var(--accent-primary-rgb), 0.2);
  }

  .btn-move-group {
    color: var(--text-muted);
  }

  .btn-move-group:hover,
  .btn-move-group.active-move {
    color: var(--accent-primary-light);
    background: rgba(var(--accent-primary-rgb), 0.1);
    border-color: rgba(var(--accent-primary-rgb), 0.2);
  }

  .btn-delete {
    color: var(--text-muted);
  }

  .btn-delete:hover {
    color: var(--color-error-soft);
    background: rgba(var(--color-error-rgb), 0.1);
    border-color: rgba(var(--color-error-rgb), 0.3);
  }

  .btn-disconnect {
    color: var(--color-error-soft);
  }

  .btn-disconnect:hover {
    background: rgba(var(--color-error-rgb), 0.1);
    border-color: rgba(var(--color-error-rgb), 0.3);
  }

  .btn-expand {
    color: var(--text-muted);
    border: none;
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

  .btn-connect:disabled, .btn-delete:disabled, .btn-disconnect:disabled, .btn-expand:disabled, .btn-edit:disabled, .btn-move-group:disabled {
    opacity: 0.4;
    cursor: not-allowed;
    pointer-events: none;
  }

  .reorder-buttons {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .btn-reorder {
    width: 22px;
    height: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    color: var(--text-muted);
    padding: 0;
    transition: background-color 0.2s, color 0.2s;
  }

  .btn-reorder:hover:not(:disabled) {
    background: rgba(var(--glass-rgb), 0.1);
    color: var(--text-hover);
  }

  .btn-reorder:disabled {
    opacity: 0.2;
    cursor: not-allowed;
  }

  .edit-name-form {
    display: flex;
  }

  .edit-name-input {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--accent-primary);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-primary);
    outline: none;
  }

  .connecting-spinner {
    display: block;
    width: 8px;
    height: 8px;
    border: 1.5px solid rgba(var(--accent-primary-rgb), 0.3);
    border-top-color: var(--accent-primary);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    will-change: transform;
  }

  .connecting-text {
    font-size: 0.75rem;
    color: var(--accent-primary-light);
    animation: fadeIn 0.3s ease-out;
  }

  .btn-spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 1.5px solid rgba(var(--accent-secondary-rgb), 0.3);
    border-top-color: var(--accent-secondary);
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

  .conn-string-row {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.02);
    border-radius: 6px;
    margin-bottom: 4px;
  }

  .conn-string-formats {
    display: flex;
    gap: 8px;
    flex: 1;
  }

  .format-btn-group {
    display: flex;
    align-items: center;
    gap: 2px;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 6px;
    padding: 2px 6px 2px 8px;
  }

  .format-label {
    font-size: 0.7rem;
    font-weight: 500;
    color: var(--text-secondary);
    font-family: 'SF Mono', 'Cascadia Code', 'Consolas', 'Liberation Mono', monospace;
  }

  .ssh-command-row {
    background: rgba(var(--accent-secondary-rgb), 0.05);
    border: 1px solid rgba(var(--accent-secondary-rgb), 0.1);
  }

  .detail-value.ssh-command {
    color: var(--accent-secondary);
    font-size: 0.75rem;
  }
</style>
