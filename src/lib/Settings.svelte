<script>
  let {
    onClose,
    invoke
  } = $props()

  let activeTab = $state('profiles')
  let awsProfiles = $state([])
  let rawConfig = $state('')
  let loading = $state(true)
  let saving = $state(false)
  let error = $state('')
  let success = $state('')

  // Edit modal state
  let editingProfile = $state(null)
  let editName = $state('')
  let editContent = $state('')

  $effect(() => {
    loadData()
  })

  async function loadData() {
    loading = true
    error = ''
    try {
      const [profiles, config] = await Promise.all([
        invoke('read_aws_config'),
        invoke('get_raw_aws_config')
      ])
      awsProfiles = profiles
      rawConfig = config
    } catch (err) {
      error = `Failed to load AWS config: ${err}`
    } finally {
      loading = false
    }
  }

  function openAddProfile() {
    editingProfile = { isNew: true }
    editName = ''
    editContent = 'region = us-east-1\n'
  }

  function openEditProfile(profile) {
    editingProfile = profile
    editName = profile.name
    editContent = profile.rawContent
  }

  function closeEditModal() {
    editingProfile = null
    editName = ''
    editContent = ''
  }

  async function saveProfile() {
    if (!editName.trim()) {
      error = 'Profile name is required'
      return
    }

    saving = true
    error = ''
    try {
      await invoke('save_aws_profile', {
        profile: {
          name: editName.trim(),
          rawContent: editContent,
          region: null,
          sourceProfile: null,
          roleArn: null,
          mfaSerial: null,
          ssoStartUrl: null,
          ssoRegion: null,
          ssoAccountId: null,
          ssoRoleName: null
        }
      })
      success = 'Profile saved successfully'
      setTimeout(() => { success = '' }, 3000)
      closeEditModal()
      await loadData()
    } catch (err) {
      error = `Failed to save profile: ${err}`
    } finally {
      saving = false
    }
  }

  async function deleteProfile(profileName) {
    if (!confirm(`Delete profile "${profileName}"?`)) return

    saving = true
    error = ''
    try {
      await invoke('delete_aws_profile', { profileName })
      success = 'Profile deleted'
      setTimeout(() => { success = '' }, 3000)
      await loadData()
    } catch (err) {
      error = `Failed to delete profile: ${err}`
    } finally {
      saving = false
    }
  }

  async function saveRawConfig() {
    saving = true
    error = ''
    try {
      await invoke('save_raw_aws_config', { content: rawConfig })
      success = 'Config saved successfully'
      setTimeout(() => { success = '' }, 3000)
      await loadData()
    } catch (err) {
      error = `Failed to save config: ${err}`
    } finally {
      saving = false
    }
  }
</script>

<div class="settings-modal">
  <div class="modal-content">
    <div class="modal-header">
      <h2>Settings</h2>
      <button class="close-btn" onclick={onClose}>
        <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
          <path d="M5 5l10 10M15 5l-10 10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
        </svg>
      </button>
    </div>

    <div class="tabs">
      <button
        class="tab"
        class:active={activeTab === 'profiles'}
        onclick={() => activeTab = 'profiles'}
      >
        AWS Profiles
      </button>
      <button
        class="tab"
        class:active={activeTab === 'raw'}
        onclick={() => activeTab = 'raw'}
      >
        Raw Config
      </button>
    </div>

    {#if error}
      <div class="message error">{error}</div>
    {/if}
    {#if success}
      <div class="message success">{success}</div>
    {/if}

    <div class="tab-content">
      {#if loading}
        <div class="loading">Loading...</div>
      {:else if activeTab === 'profiles'}
        <div class="profiles-tab">
          <div class="profiles-header">
            <span class="profiles-path">~/.aws/config</span>
            <button class="btn-add" onclick={openAddProfile}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
                <path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
              </svg>
              Add Profile
            </button>
          </div>

          {#if awsProfiles.length === 0}
            <div class="empty-state">
              <p>No AWS profiles found</p>
              <p class="hint">Click "Add Profile" to create one</p>
            </div>
          {:else}
            <div class="profiles-list">
              {#each awsProfiles as profile}
                <div class="profile-card">
                  <div class="profile-header">
                    <span class="profile-name">{profile.name}</span>
                    <div class="profile-actions">
                      <button class="btn-icon" onclick={() => openEditProfile(profile)} title="Edit">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M10.5 1.5l2 2-8 8H2.5v-2l8-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                      <button class="btn-icon delete" onclick={() => deleteProfile(profile.name)} title="Delete">
                        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
                          <path d="M2 4h10M5 4V2.5a.5.5 0 01.5-.5h3a.5.5 0 01.5.5V4M11 4v8a1 1 0 01-1 1H4a1 1 0 01-1-1V4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                      </button>
                    </div>
                  </div>
                  <div class="profile-details">
                    {#if profile.region}
                      <span class="detail">Region: {profile.region}</span>
                    {/if}
                    {#if profile.sourceProfile}
                      <span class="detail">Source: {profile.sourceProfile}</span>
                    {/if}
                    {#if profile.roleArn}
                      <span class="detail">Role: {profile.roleArn.split('/').pop()}</span>
                    {/if}
                    {#if profile.ssoStartUrl}
                      <span class="detail">SSO</span>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {:else}
        <div class="raw-tab">
          <textarea
            class="raw-editor"
            bind:value={rawConfig}
            placeholder="# AWS Config file contents..."
            spellcheck="false"
          ></textarea>
          <div class="raw-actions">
            <button class="btn-save" onclick={saveRawConfig} disabled={saving}>
              {saving ? 'Saving...' : 'Save Config'}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <!-- Edit Profile Modal -->
  {#if editingProfile}
    <div class="edit-modal-overlay" onclick={closeEditModal}>
      <div class="edit-modal" onclick={(e) => e.stopPropagation()}>
        <h3>{editingProfile.isNew ? 'Add Profile' : 'Edit Profile'}</h3>

        <div class="form-group">
          <label for="profile-name">Profile Name</label>
          <input
            id="profile-name"
            type="text"
            bind:value={editName}
            placeholder="my-profile"
            disabled={!editingProfile.isNew}
          />
        </div>

        <div class="form-group">
          <label for="profile-content">Configuration</label>
          <textarea
            id="profile-content"
            bind:value={editContent}
            placeholder="region = us-east-1&#10;source_profile = default&#10;role_arn = arn:aws:iam::..."
            spellcheck="false"
          ></textarea>
        </div>

        <div class="edit-actions">
          <button class="btn-cancel" onclick={closeEditModal}>Cancel</button>
          <button class="btn-save" onclick={saveProfile} disabled={saving}>
            {saving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-modal {
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
    background: linear-gradient(145deg, #1a1a2e 0%, #16162a 100%);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 20px;
    padding: 24px;
    max-width: 560px;
    width: 100%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    animation: slideUp 0.3s ease-out;
  }

  @keyframes slideUp {
    from { opacity: 0; transform: translateY(20px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 20px;
  }

  .modal-header h2 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
    color: #e4e4e7;
  }

  .close-btn {
    background: none;
    border: none;
    color: #71717a;
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
    transition: all 0.2s;
  }

  .close-btn:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #e4e4e7;
  }

  .tabs {
    display: flex;
    gap: 4px;
    background: rgba(255, 255, 255, 0.03);
    padding: 4px;
    border-radius: 10px;
    margin-bottom: 16px;
  }

  .tab {
    flex: 1;
    padding: 10px 16px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #71717a;
    background: none;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .tab:hover {
    color: #a1a1aa;
  }

  .tab.active {
    background: rgba(99, 102, 241, 0.15);
    color: #a5b4fc;
  }

  .message {
    padding: 10px 14px;
    border-radius: 8px;
    font-size: 0.875rem;
    margin-bottom: 12px;
  }

  .message.error {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
    border: 1px solid rgba(239, 68, 68, 0.2);
  }

  .message.success {
    background: rgba(34, 197, 94, 0.1);
    color: #4ade80;
    border: 1px solid rgba(34, 197, 94, 0.2);
  }

  .tab-content {
    flex: 1;
    overflow-y: auto;
    min-height: 300px;
  }

  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 200px;
    color: #71717a;
  }

  .profiles-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .profiles-path {
    font-size: 0.75rem;
    color: #52525b;
    font-family: ui-monospace, monospace;
  }

  .btn-add {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    font-size: 0.8rem;
    font-weight: 500;
    color: #a5b4fc;
    background: rgba(99, 102, 241, 0.1);
    border: 1px solid rgba(99, 102, 241, 0.2);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-add:hover {
    background: rgba(99, 102, 241, 0.15);
  }

  .empty-state {
    text-align: center;
    padding: 40px 20px;
    color: #71717a;
  }

  .empty-state p {
    margin: 0 0 8px;
  }

  .empty-state .hint {
    font-size: 0.875rem;
    color: #52525b;
  }

  .profiles-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .profile-card {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 12px;
    padding: 14px;
  }

  .profile-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .profile-name {
    font-weight: 500;
    color: #e4e4e7;
  }

  .profile-actions {
    display: flex;
    gap: 4px;
  }

  .btn-icon {
    padding: 6px;
    background: none;
    border: none;
    color: #71717a;
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-icon:hover {
    background: rgba(255, 255, 255, 0.1);
    color: #a1a1aa;
  }

  .btn-icon.delete:hover {
    background: rgba(239, 68, 68, 0.1);
    color: #f87171;
  }

  .profile-details {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .detail {
    font-size: 0.75rem;
    color: #71717a;
    background: rgba(255, 255, 255, 0.05);
    padding: 4px 8px;
    border-radius: 4px;
  }

  .raw-tab {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .raw-editor {
    flex: 1;
    min-height: 280px;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 10px;
    padding: 14px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    color: #e4e4e7;
    resize: none;
    outline: none;
  }

  .raw-editor:focus {
    border-color: #6366f1;
  }

  .raw-actions {
    margin-top: 12px;
    display: flex;
    justify-content: flex-end;
  }

  .btn-save {
    padding: 10px 20px;
    font-size: 0.875rem;
    font-weight: 500;
    color: white;
    background: linear-gradient(135deg, #6366f1 0%, #8b5cf6 100%);
    border: none;
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-save:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.3);
  }

  .btn-save:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Edit Modal */
  .edit-modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 110;
  }

  .edit-modal {
    background: #1e1e32;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 16px;
    padding: 24px;
    width: 400px;
    max-width: 90%;
  }

  .edit-modal h3 {
    margin: 0 0 20px;
    font-size: 1.1rem;
    color: #e4e4e7;
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    font-size: 0.8rem;
    font-weight: 500;
    color: #a1a1aa;
    margin-bottom: 8px;
  }

  .form-group input,
  .form-group textarea {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    padding: 12px;
    font-size: 0.875rem;
    color: #e4e4e7;
    outline: none;
  }

  .form-group input:focus,
  .form-group textarea:focus {
    border-color: #6366f1;
  }

  .form-group input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .form-group textarea {
    min-height: 150px;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    resize: vertical;
  }

  .edit-actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
    margin-top: 20px;
  }

  .btn-cancel {
    padding: 10px 20px;
    font-size: 0.875rem;
    font-weight: 500;
    color: #71717a;
    background: none;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .btn-cancel:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #a1a1aa;
  }
</style>
