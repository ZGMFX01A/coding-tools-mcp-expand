<script lang="ts">
  import CopyButton from "$lib/components/CopyButton.svelte";
  import StatusOrb from "$lib/components/StatusOrb.svelte";
  import type { RuntimeState } from "$lib/types";

  interface Props {
    status: RuntimeState;
    statusMessage?: string;
    port: number;
    busy?: boolean;
    tunnelType?: string;
    authType?: string;
    toolProfile?: string;
    permissionMode?: string;
    localEndpoint: string;
    publicEndpoint?: string;
    onToggle: () => void | Promise<void>;
    onPortChange?: (port: number) => void | Promise<void>;
    onConfigure?: () => void;
  }

  let {
    status,
    statusMessage = "",
    port,
    busy = false,
    tunnelType = "none",
    authType = "none",
    toolProfile = "core",
    permissionMode = "safe",
    localEndpoint,
    publicEndpoint = "",
    onToggle,
    onPortChange,
    onConfigure,
  }: Props = $props();

  let draftPort = $state<number>(0);

  $effect(() => {
    draftPort = port;
  });

  const running = $derived(status === "running");
  const canEditPort = $derived(!busy && status !== "running" && status !== "starting");

  function tunnelText(value: string) {
    if (value === "frp") return "FRP 公网隧道";
    if (value === "cloudflared") return "Cloudflare 隧道";
    return "仅本机";
  }

  function authText(value: string) {
    if (value === "oauth") return "OAuth";
    if (value === "bearer") return "Bearer Token";
    return "无认证";
  }

  function profileText(value: string) {
    if (value === "advanced") return "高级工具";
    if (value === "read-only") return "只读工具";
    return "核心工具";
  }

  function permissionText(value: string) {
    if (value === "trusted") return "信任模式";
    if (value === "dangerous") return "危险模式";
    return "安全模式";
  }

  async function commitPort() {
    const nextPort = Number(draftPort);
    if (!Number.isInteger(nextPort) || nextPort < 1 || nextPort > 65535) {
      draftPort = port;
      return;
    }
    if (nextPort !== port) await onPortChange?.(nextPort);
  }
</script>

<article class="tx-card p-5 sm:p-6">
  <div class="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
    <div class="min-w-0">
      <div class="flex flex-wrap items-center gap-2">
        <StatusOrb state={status} />
        <h3 class="text-base font-semibold text-[var(--color-text)]">MCP 连接</h3>
        <span class="tx-badge">Streamable HTTP</span>
      </div>
      <p class="mt-1.5 max-w-2xl text-sm leading-6 text-[var(--color-text-muted)]">
        AI 客户端通过下面的地址访问当前工作区。通常只需要启动服务并复制地址，不需要填写命令行参数。
      </p>
    </div>
    <div class="flex shrink-0 flex-wrap items-center gap-2">
      {#if onConfigure}
        <button type="button" class="tx-btn-ghost" onclick={onConfigure}>配置连接</button>
      {/if}
      <button
        type="button"
        class="tx-btn-primary"
        class:tx-btn-danger={running}
        disabled={busy}
        onclick={() => void onToggle()}
      >
        {busy ? "处理中…" : running ? "停止服务" : "启动服务"}
      </button>
    </div>
  </div>

  {#if statusMessage && status === "error"}
    <div class="tx-alert tx-alert--error mt-4" role="alert">{statusMessage}</div>
  {/if}

  <div class="mt-5 grid gap-3 lg:grid-cols-2">
    <div class="tx-info-block min-w-0 border border-[var(--primary)]/20 bg-[var(--primary-soft)]/45">
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="tx-section-label">本机地址</p>
          <p class="mt-1 text-xs text-[var(--color-text-muted)]">本机客户端使用</p>
        </div>
        <CopyButton value={localEndpoint} />
      </div>
      <p class="mt-3 break-all font-mono text-sm text-[var(--color-text)]">{localEndpoint}</p>
    </div>

    <div class="tx-info-block min-w-0">
      <div class="flex items-center justify-between gap-3">
        <div>
          <p class="tx-section-label">公网地址</p>
          <p class="mt-1 text-xs text-[var(--color-text-muted)]">远程客户端 / ChatGPT 使用</p>
        </div>
        {#if publicEndpoint}<CopyButton value={publicEndpoint} />{/if}
      </div>
      <p class="mt-3 break-all font-mono text-sm text-[var(--color-text)]">
        {publicEndpoint || "配置公网隧道后生成"}
      </p>
    </div>
  </div>

  <div class="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 border-t border-[var(--color-border)] pt-4 text-xs text-[var(--color-text-muted)]">
    <label class="inline-flex items-center gap-2">
      <span>端口</span>
      <input
        class="w-16 rounded-md border border-[var(--color-border)] bg-transparent px-2 py-1 text-right font-mono text-[var(--color-text)] outline-none focus:border-[var(--primary)]"
        type="number"
        min="1024"
        max="65535"
        bind:value={draftPort}
        disabled={!canEditPort}
        onchange={() => void commitPort()}
        aria-label="MCP 端口"
      />
    </label>
    <span>认证：<strong class="font-medium text-[var(--color-text)]">{authText(authType)}</strong></span>
    <span>隧道：<strong class="font-medium text-[var(--color-text)]">{tunnelText(tunnelType)}</strong></span>
    <span>运行策略：<strong class="font-medium text-[var(--color-text)]">{profileText(toolProfile)} · {permissionText(permissionMode)}</strong></span>
  </div>
</article>
