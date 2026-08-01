<script lang="ts">
  import { FolderInput, FolderOpen } from "@lucide/svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { openWorkspaceDirectory } from "$lib/api/workspaces";
  import { showToast } from "$lib/stores/toast";

  interface Props {
    id: string;
    name: string;
    path: string;
    onSave: (name: string) => void | Promise<void>;
    onUpdatePath: (path: string) => void | Promise<void>;
    onSaveId: (id: string) => void | Promise<void>;
  }

  let { id, name, path, onSave, onUpdatePath, onSaveId }: Props = $props();

  let draftId = $state("");
  let draftName = $state("");
  let savingId = $state(false);
  let saving = $state(false);
  let opening = $state(false);
  let updatingPath = $state(false);

  const dirty = $derived(draftName.trim() !== name && draftName.trim().length > 0);
  const idDirty = $derived(draftId.trim() !== id && draftId.trim().length > 0);

  $effect(() => {
    draftId = id;
    draftName = name;
  });

  async function saveId() {
    if (savingId || !idDirty) return;
    savingId = true;
    try {
      await onSaveId(draftId.trim());
    } finally {
      savingId = false;
    }
  }

  async function save() {
    if (saving || !dirty) return;
    saving = true;
    try {
      await onSave(draftName.trim());
    } finally {
      saving = false;
    }
  }

  async function openDirectory() {
    if (opening || !path.trim()) return;
    opening = true;
    try {
      await openWorkspaceDirectory(path);
    } catch (error) {
      showToast(String(error), {
        kind: "error",
        title: "无法打开目录",
      });
    } finally {
      opening = false;
    }
  }

  function normalizePath(value: string): string {
    return value.trim().replace(/[\\/]+$/, "");
  }

  async function updateDirectory() {
    if (updatingPath) return;
    updatingPath = true;
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        defaultPath: path.trim() || undefined,
      });
      if (!selected || Array.isArray(selected)) return;
      const nextPath = normalizePath(selected);
      if (!nextPath || nextPath === normalizePath(path)) return;
      await onUpdatePath(nextPath);
    } catch (error) {
      showToast(String(error), {
        kind: "error",
        title: "无法更新目录",
      });
    } finally {
      updatingPath = false;
    }
  }
</script>

<form class="flex flex-col gap-4" onsubmit={(event) => {
    event.preventDefault();
    void save();
  }}
>
  <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
    <div class="tx-field min-w-0 flex-1">
      <span class="tx-label">工作区 ID</span>
      <div class="flex min-w-0 items-center gap-2">
        <input
          type="text"
          class="tx-input min-w-0 flex-1 font-mono"
          spellcheck="false"
          bind:value={draftId}
        />
        <button
          type="button"
          class="tx-btn-primary shrink-0"
          disabled={savingId || !idDirty}
          onclick={() => void saveId()}
        >
          {savingId ? "保存中…" : "保存 ID"}
        </button>
      </div>
      {#if idDirty}
        <p class="mt-1 text-[11px] text-[var(--color-text-muted)]">
          修改后将改变工作区访问地址，且需先停止 MCP 和 Actions 服务；ID 仅允许字母、数字、连字符（-）和下划线（_）。
        </p>
      {/if}
    </div>
    <label class="tx-field min-w-0 flex-1">
      <span class="tx-label">工作区名称</span>
      <input type="text" class="tx-input" bind:value={draftName} />
    </label>
  </div>
  <div class="flex flex-col gap-3 sm:flex-row sm:items-end">
    <div class="tx-field min-w-0 flex-1">
      <span class="tx-label">路径</span>
      <div class="flex min-w-0 items-center gap-2">
        <p
          class="tx-mono min-w-0 flex-1 truncate rounded-[10px] border border-transparent px-2.5 py-2 text-[var(--color-text-secondary)]"
          title={path}
        >
          {path}
        </p>
        <button
          type="button"
          class="tx-btn-ghost shrink-0 px-2.5 py-1.5 text-xs"
          disabled={opening || !path.trim()}
          onclick={() => void openDirectory()}
        >
          <FolderOpen size={14} class="inline-block" />
          <span class="ml-1">{opening ? "打开中…" : "打开目录"}</span>
        </button>
        <button
          type="button"
          class="tx-btn-ghost shrink-0 px-2.5 py-1.5 text-xs"
          disabled={updatingPath}
          onclick={() => void updateDirectory()}
        >
          <FolderInput size={14} class="inline-block" />
          <span class="ml-1">{updatingPath ? "选择中…" : "更新目录"}</span>
        </button>
      </div>
    </div>
    <button type="submit" class="tx-btn-primary shrink-0" disabled={saving || !dirty}>
      {saving ? "保存中…" : "保存名称"}
    </button>
  </div>
</form>
