<script lang="ts">
  // OPDS catalog access — a global on/off toggle plus per-user account
  // management. Self-contained: seeded from the page's load data, then
  // maintained from each mutation's response (the pull-list pattern).
  import { ApiError } from '$lib/api/client';
  import {
    createOpdsUser,
    deleteOpdsUser,
    disableOpdsUser,
    enableOpdsUser,
    saveOpds,
    type Opds,
    type OpdsUser
  } from '$lib/api/opds';
  import { formatDateTime } from '$lib/format';
  import { toast } from '$lib/stores/toast.svelte';
  import Button from './Button.svelte';
  import ErrorBanner from './ErrorBanner.svelte';
  import Modal from './Modal.svelte';

  let { opds, users }: { opds: Opds; users: OpdsUser[] } = $props();

  let current = $state<Opds>(opds);
  let accounts = $state<OpdsUser[]>([...users]);
  let busy = $state(false);
  let error = $state<ApiError | null>(null);

  // Add-user inline form.
  let showAddForm = $state(false);
  let newUsername = $state('');
  let newPassword = $state('');
  let confirmPassword = $state('');
  let addError = $state<string | null>(null);

  // Delete confirmation target.
  let deleteTarget = $state<OpdsUser | null>(null);

  // The copyable catalog URL. When a public base URL is configured (reverse
  // proxy), it's authoritative; otherwise compose from the browser's host +
  // the dedicated OPDS port.
  const catalogUrl = $derived.by(() => {
    if (current.base_url) return `${current.base_url}/opds/v1`;
    if (typeof window === 'undefined') return `:${current.opds_port}/opds/v1`;
    return `${window.location.protocol}//${window.location.hostname}:${current.opds_port}/opds/v1`;
  });

  // No accounts while enabled means every reader gets 401 — surface it.
  const enabledButNoUsers = $derived(current.enabled && accounts.length === 0);

  async function run(fn: () => Promise<void>): Promise<void> {
    busy = true;
    error = null;
    try {
      await fn();
    } catch (e) {
      error = e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e));
    } finally {
      busy = false;
    }
  }

  function handleToggle(enabled: boolean): Promise<void> {
    return run(async () => {
      current = await saveOpds({ enabled });
      toast.success(enabled ? 'OPDS catalog enabled.' : 'OPDS catalog disabled.');
    });
  }

  function resetAddForm(): void {
    showAddForm = false;
    newUsername = '';
    newPassword = '';
    confirmPassword = '';
    addError = null;
  }

  async function handleCreate(): Promise<void> {
    addError = null;
    const username = newUsername.trim();
    if (!username) {
      addError = 'Username is required.';
      return;
    }
    if (newPassword.length < 8) {
      addError = 'Password must be at least 8 characters.';
      return;
    }
    if (newPassword !== confirmPassword) {
      addError = 'Passwords do not match.';
      return;
    }
    busy = true;
    try {
      const created = await createOpdsUser({ username, password: newPassword });
      accounts = [...accounts, created].sort((a, b) =>
        a.username.localeCompare(b.username, undefined, { sensitivity: 'base' })
      );
      resetAddForm();
      toast.success(`Created OPDS user "${created.username}".`);
    } catch (e) {
      addError = e instanceof ApiError ? e.message : 'Could not create the user.';
    } finally {
      busy = false;
    }
  }

  function handleToggleUser(user: OpdsUser): Promise<void> {
    return run(async () => {
      const result = user.enabled ? await disableOpdsUser(user.id) : await enableOpdsUser(user.id);
      accounts = accounts.map((u) =>
        u.id === user.id ? { ...u, enabled: result.enabled } : u
      );
      toast.success(`${user.username} ${result.enabled ? 'enabled' : 'disabled'}.`);
    });
  }

  async function confirmDelete(): Promise<void> {
    if (!deleteTarget) return;
    const target = deleteTarget;
    await run(async () => {
      await deleteOpdsUser(target.id);
      accounts = accounts.filter((u) => u.id !== target.id);
      toast.success(`Deleted OPDS user "${target.username}".`);
    });
    deleteTarget = null;
  }

  async function copy(value: string, label: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(`${label} copied.`);
    } catch {
      toast.warning('Could not copy to clipboard.');
    }
  }
</script>

<section class="rounded-lg border border-slate-200 bg-white p-4">
  <h2 class="mb-2 text-base font-semibold">OPDS</h2>
  <p class="mb-3 text-sm text-slate-600">
    Serve your library to OPDS comic readers (Chunky, Panels, KyBook). Each reader connects with
    its own username and password — create an account per person below.
  </p>

  {#if error}
    <div class="mb-3"><ErrorBanner {error} onDismiss={() => (error = null)} /></div>
  {/if}

  <label class="flex items-center gap-2 text-sm text-slate-700">
    <input
      type="checkbox"
      checked={current.enabled}
      disabled={busy}
      onchange={(e) => handleToggle(e.currentTarget.checked)}
    />
    Enabled
  </label>

  {#if enabledButNoUsers}
    <p class="mt-2 text-xs text-amber-700">
      No accounts yet — readers can't connect until you add at least one user below.
    </p>
  {/if}

  <hr class="my-4 border-slate-100" />

  <!-- ===================== Accounts ===================== -->
  <div class="mb-3 flex items-center justify-between">
    <h3 class="text-sm font-semibold text-slate-700">Users</h3>
    {#if !showAddForm}
      <Button type="button" size="sm" onclick={() => (showAddForm = true)} disabled={busy}>
        Add user
      </Button>
    {/if}
  </div>

  {#if showAddForm}
    <form
      class="mb-4 space-y-2 rounded-md border border-slate-200 bg-slate-50 p-3"
      onsubmit={(e) => {
        e.preventDefault();
        void handleCreate();
      }}
    >
      {#if addError}
        <p class="rounded bg-red-50 px-2 py-1 text-xs text-red-700">{addError}</p>
      {/if}
      <label class="block text-xs font-medium text-slate-600">
        Username
        <input
          type="text"
          autocomplete="off"
          bind:value={newUsername}
          disabled={busy}
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
      <label class="block text-xs font-medium text-slate-600">
        Password
        <input
          type="password"
          autocomplete="new-password"
          bind:value={newPassword}
          disabled={busy}
          placeholder="At least 8 characters"
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
      <label class="block text-xs font-medium text-slate-600">
        Confirm password
        <input
          type="password"
          autocomplete="new-password"
          bind:value={confirmPassword}
          disabled={busy}
          class="mt-0.5 w-full rounded-md border border-slate-300 px-2 py-1 text-sm font-normal text-slate-900 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
        />
      </label>
      <div class="flex gap-2">
        <Button type="submit" size="sm" disabled={busy}>Save</Button>
        <Button type="button" variant="ghost" size="sm" onclick={resetAddForm} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  {/if}

  {#if accounts.length === 0}
    <p class="text-sm text-slate-500">No OPDS users yet.</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-slate-200 text-left text-xs text-slate-500">
            <th class="py-1.5 pr-3 font-medium">Username</th>
            <th class="py-1.5 pr-3 font-medium">Status</th>
            <th class="py-1.5 pr-3 font-medium">Last seen</th>
            <th class="py-1.5 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {#each accounts as u (u.id)}
            <tr class="border-b border-slate-100">
              <td class="py-1.5 pr-3 font-medium text-slate-900">{u.username}</td>
              <td class="py-1.5 pr-3">
                {#if u.enabled}
                  <span class="rounded bg-emerald-100 px-1.5 py-0.5 text-xs text-emerald-700">
                    Active
                  </span>
                {:else}
                  <span class="rounded bg-slate-100 px-1.5 py-0.5 text-xs text-slate-600">
                    Disabled
                  </span>
                {/if}
              </td>
              <td class="py-1.5 pr-3 text-xs text-slate-500">
                {u.last_seen_at ? formatDateTime(u.last_seen_at) : 'Never'}
              </td>
              <td class="py-1.5">
                <div class="flex gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onclick={() => handleToggleUser(u)}
                    disabled={busy}
                  >
                    {u.enabled ? 'Disable' : 'Enable'}
                  </Button>
                  <Button
                    type="button"
                    variant="danger"
                    size="sm"
                    onclick={() => (deleteTarget = u)}
                    disabled={busy}
                  >
                    Delete
                  </Button>
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <hr class="my-4 border-slate-100" />

  <div>
    <span class="block text-xs font-medium text-slate-600">Catalog URL</span>
    <div class="mt-0.5 flex gap-2">
      <input
        type="text"
        readonly
        value={catalogUrl}
        class="w-full rounded-md border border-slate-200 bg-slate-50 px-2 py-1 font-mono text-xs text-slate-700 focus:outline-none"
      />
      <Button
        type="button"
        variant="secondary"
        size="sm"
        onclick={() => copy(catalogUrl, 'Catalog URL')}
      >
        Copy
      </Button>
    </div>
    <p class="mt-1 text-xs text-slate-500">
      Add this URL in your OPDS reader. It points at the dedicated OPDS port ({current.opds_port}).
    </p>
  </div>
</section>

{#if deleteTarget}
  {@const target = deleteTarget}
  <Modal open={true} title="Delete OPDS user?" onClose={() => (deleteTarget = null)}>
    <p class="text-sm text-slate-600">
      Remove <strong>{target.username}</strong>. Their reader apps will stop working immediately.
      This cannot be undone.
    </p>
    <div class="mt-4 flex justify-end gap-2">
      <Button type="button" variant="ghost" onclick={() => (deleteTarget = null)} disabled={busy}>
        Cancel
      </Button>
      <Button type="button" variant="danger" onclick={confirmDelete} disabled={busy}>
        Delete
      </Button>
    </div>
  </Modal>
{/if}
