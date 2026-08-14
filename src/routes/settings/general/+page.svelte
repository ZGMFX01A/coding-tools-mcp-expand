<script lang="ts">
  import { onMount } from "svelte";
  import { ask, message } from "@tauri-apps/plugin-dialog";
  import { getProxy, setProxy, type ProxyConfigDto } from "$lib/api/settings";
  import { getWebviewMemorySample } from "$lib/api/ui-memory";
  import { reloadUiOnly } from "$lib/ui-memory-guard";
  import { showToast } from "$lib/stores/toast";

  let proxy = $state<ProxyConfigDto>({ mode: "none", url: "" });
  let changed = $state(false);
  let saving = $state(false);
  let releasingUi = $state(false);
  let memoryHint = $state<string | null>(null);

  async function refresh() {
    try {
      proxy = await getProxy();
      changed = false;
    } catch (e) {
      await message(String(e), { title: "加载失败", kind: "error" });
    }
  }

  async function save() {
    saving = true;
    try {
      await setProxy(proxy);
      changed = false;
      await message("代理设置已保存。", { title: "已保存", kind: "info" });
    } catch (e) {
      await message(String(e), { title: "保存失败", kind: "error" });
    } finally {
      saving = false;
    }
  }

  function handleChange() {
    changed = true;
  }

  async function refreshMemoryHint() {
    try {
      const sample = await getWebviewMemorySample();
      if (!sample.supported) {
        memoryHint = "当前平台暂不支持界面内存采样。";
        return;
      }
      memoryHint = `界面约 ${Math.round(sample.webviewMb)} MB（${sample.webviewProcessCount} 个 WebView 进程），主进程约 ${Math.round(sample.mainMb)} MB。`;
    } catch {
      memoryHint = null;
    }
  }

  async function handleReleaseUiMemory() {
    if (releasingUi) return;
    const ok = await ask(
      "将重建界面进程（WebView）以释放内存。MCP、Actions 与 FRP 隧道会继续在后台运行，不会被停止。",
      { title: "释放界面内存", kind: "info", okLabel: "立即释放", cancelLabel: "取消" },
    );
    if (!ok) return;
    releasingUi = true;
    showToast("正在重建界面进程…", { title: "释放界面内存", kind: "info", duration: 2000 });
    await reloadUiOnly("settings-manual");
  }

  onMount(() => {
    void refresh();
    void refreshMemoryHint();
  });
</script>

<section class="page-scroll">
  <header class="page-header">
    <p class="page-kicker">全局设置</p>
    <h2 class="page-title">通用</h2>
    <p class="mt-2 max-w-2xl text-sm text-[var(--color-text-muted)]">
      配置全局网络代理与界面内存释放。网络代理将应用于 Cloudflare 隧道连接，不影响软件下载代理。
    </p>
  </header>

  <div class="page-body flex flex-col gap-6">
    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">界面内存</h3>
      <p class="mt-1 text-xs text-[var(--color-text-muted)]">
        长时间运行后 WebView 可能占用较高内存。释放会重建界面进程，不会停止 MCP、External MCP 或隧道。
      </p>
      {#if memoryHint}
        <p class="mt-2 text-xs text-[var(--color-text-muted)]">{memoryHint}</p>
      {/if}
      <div class="mt-4 flex flex-wrap gap-2">
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-1.5 text-sm"
          onclick={() => void refreshMemoryHint()}
        >
          刷新占用
        </button>
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-white disabled:opacity-50"
          disabled={releasingUi}
          onclick={() => void handleReleaseUiMemory()}
        >
          <svg
            class="h-3.5 w-3.5 {releasingUi ? 'animate-spin' : ''}"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
          </svg>
          {releasingUi ? "刷新中…" : "释放界面内存"}
        </button>
      </div>
    </div>

    <div class="tx-card p-4">
      <h3 class="text-sm font-semibold">网络代理</h3>
      <form
        class="mt-4 grid gap-3"
        onsubmit={(e) => { e.preventDefault(); void save(); }}
      >
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">代理模式</span>
          <select
            class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-sm"
            bind:value={proxy.mode}
            onchange={handleChange}
          >
            <option value="none">无代理</option>
            <option value="system">系统代理</option>
            <option value="manual">手动代理地址</option>
          </select>
        </label>

        {#if proxy.mode === "manual"}
          <label class="grid gap-1">
            <span class="text-xs text-[var(--color-text-muted)]">代理地址</span>
            <input
              type="text"
              class="tx-input tx-mono"
              placeholder="http://127.0.0.1:7890"
              bind:value={proxy.url}
              oninput={handleChange}
            />
            <span class="text-xs text-[var(--color-text-muted)]">
              支持 HTTP/HTTPS/SOCKS 代理，如 http://127.0.0.1:7890
            </span>
          </label>
        {/if}

        <div class="flex justify-end pt-1">
          <button
            type="submit"
            class="rounded-md bg-[var(--color-accent)] px-3 py-1.5 text-sm font-medium text-white disabled:opacity-50"
            disabled={!changed || saving}
          >
            {saving ? "保存中…" : "保存设置"}
          </button>
        </div>
      </form>
    </div>
  </div>
</section>
