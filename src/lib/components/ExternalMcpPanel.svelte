<script lang="ts">
  import type { ExternalMcpConfig, ExternalMcpStatus, McpTool } from "$lib/types";
  import {
    deleteExternalMcp,
    getExternalMcpDiscoveredTools,
    listExternalMcps,
    reconnectExternalMcp,
    saveExternalMcp,
  } from "$lib/api/external_mcp";
  import ExternalMcpFormModal from "./ExternalMcpFormModal.svelte";
  import ExternalMcpToolsModal from "./ExternalMcpToolsModal.svelte";
  import { showToast } from "$lib/stores/toast";

  interface Props {
    workspaceId: string;
    configs?: ExternalMcpConfig[];
    onRefreshProfile: () => Promise<void>;
  }

  let { workspaceId, configs = [], onRefreshProfile }: Props = $props();

  let statuses = $state<ExternalMcpStatus[]>([]);
  let loading = $state(false);
  let editingConfig = $state<ExternalMcpConfig | null>(null);
  let showFormModal = $state(false);
  let viewingTools = $state<{ serverName: string; tools: McpTool[] } | null>(null);

  async function loadStatuses() {
    if (!workspaceId) return;
    loading = true;
    try {
      statuses = await listExternalMcps(workspaceId);
    } catch (e) {
      console.error("加载外部 MCP 状态失败:", e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (workspaceId) {
      void loadStatuses();
    }
  });

  function getStatus(configId: string): ExternalMcpStatus | undefined {
    return statuses.find((s) => s.configId === configId);
  }

  function stateLabel(stateStr: string | undefined): { label: string; color: string } {
    switch (stateStr) {
      case "ready":
        return { label: "运行中", color: "bg-emerald-500/10 text-emerald-400 border-emerald-500/20" };
      case "starting":
        return { label: "正在启动", color: "bg-blue-500/10 text-blue-400 border-blue-500/20" };
      case "initializing":
        return { label: "正在初始化", color: "bg-amber-500/10 text-amber-400 border-amber-500/20" };
      case "restarting":
        return { label: "正在重启", color: "bg-amber-500/10 text-amber-400 border-amber-500/20" };
      case "stopping":
        return { label: "正在停止", color: "bg-slate-500/10 text-slate-400 border-slate-500/20" };
      case "error":
        return { label: "异常", color: "bg-red-500/10 text-red-400 border-red-500/20" };
      case "disabled":
        return { label: "未启用", color: "bg-slate-500/10 text-slate-400 border-slate-500/20" };
      default:
        return { label: "未启动", color: "bg-slate-500/10 text-slate-400 border-slate-500/20" };
    }
  }

  function openCreateModal() {
    editingConfig = null;
    showFormModal = true;
  }

  function openFastContextTemplate() {
    // 默认使用本机已有命令模板
    editingConfig = {
      id: `mcp-fc-${Date.now()}`,
      name: "fast-context",
      enabled: true,
      command: "fast-context-mcp",
      args: [],
      env: {
        FC_INCLUDE_SNIPPETS: "true",
      },
      allowedTools: ["extract_windsurf_key", "fast_context_search"],
      autoRestart: true,
      initializeTimeoutSeconds: 30,
      callTimeoutSeconds: 120,
    };
    showFormModal = true;
  }

  function openEditModal(cfg: ExternalMcpConfig) {
    editingConfig = cfg;
    showFormModal = true;
  }

  async function handleSaveConfig(cfg: ExternalMcpConfig) {
    try {
      await saveExternalMcp(workspaceId, cfg);
      showToast(cfg.id ? "外部 MCP 配置已更新" : "外部 MCP 配置已添加", { kind: "success" });
      await onRefreshProfile();
      await loadStatuses();
    } catch (e: any) {
      showToast(`保存失败: ${e?.message || e}`, { kind: "error" });
    }
  }

  async function handleToggleEnable(cfg: ExternalMcpConfig) {
    try {
      const updated = { ...cfg, enabled: !cfg.enabled };
      await saveExternalMcp(workspaceId, updated);
      showToast(updated.enabled ? "已启用" : "已禁用", { kind: "info" });
      await onRefreshProfile();
      await loadStatuses();
    } catch (e: any) {
      showToast(`更新失败: ${e?.message || e}`, { kind: "error" });
    }
  }

  async function handleReconnect(configId: string) {
    try {
      await reconnectExternalMcp(workspaceId, configId);
      showToast("正在重新连接...", { kind: "info" });
      await loadStatuses();
    } catch (e: any) {
      showToast(`重连失败: ${e?.message || e}`, { kind: "error" });
    }
  }

  async function handleDelete(configId: string) {
    if (!confirm("确定要删除此外部 MCP 配置吗？")) return;
    try {
      await deleteExternalMcp(workspaceId, configId);
      showToast("外部 MCP 已删除", { kind: "success" });
      await onRefreshProfile();
      await loadStatuses();
    } catch (e: any) {
      showToast(`删除失败: ${e?.message || e}`, { kind: "error" });
    }
  }

  async function handleViewTools(cfg: ExternalMcpConfig) {
    try {
      const tools = await getExternalMcpDiscoveredTools(workspaceId, cfg.id);
      viewingTools = { serverName: cfg.name, tools };
    } catch (e: any) {
      showToast(`获取工具失败: ${e?.message || e}`, { kind: "error" });
    }
  }
</script>

<div class="rounded-xl border border-[var(--color-border)] bg-[var(--color-bg)] p-5 shadow-xs">
  <div class="flex items-center justify-between border-b border-[var(--color-border)] pb-4">
    <div>
      <h3 class="text-base font-semibold text-[var(--color-text)]">外部 stdio MCP 聚合</h3>
      <p class="mt-0.5 text-xs text-[var(--color-text-muted)]">
        聚合与启动本机已安装的 stdio MCP 服务（如代码检索 fast-context、数据库 MCP 等）
      </p>
    </div>
    <div class="flex items-center gap-2">
      <button
        class="rounded-md border border-[var(--color-border)] px-3 py-1.5 text-xs font-medium hover:bg-white/5"
        onclick={openFastContextTemplate}
      >
        + 导入 fast-context 模板
      </button>
      <button
        class="rounded-md bg-blue-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-blue-500"
        onclick={openCreateModal}
      >
        + 新增外部 MCP
      </button>
    </div>
  </div>

  <div class="mt-4">
    {#if configs.length === 0}
      <div class="flex flex-col items-center justify-center rounded-lg border border-dashed border-[var(--color-border)] p-8 text-center">
        <p class="text-xs text-[var(--color-text-muted)]">尚未配置任何外部 stdio MCP 服务</p>
        <p class="mt-1 text-[11px] text-[var(--color-text-muted)]">
          配置本机已安装的 <span class="font-mono text-blue-400">fast-context-mcp</span> 可开启项目级代码语义搜索能力
        </p>
        <button
          class="mt-3 rounded-md border border-[var(--color-border)] bg-white/5 px-4 py-1.5 text-xs font-medium hover:bg-white/10"
          onclick={openFastContextTemplate}
        >
          导入 fast-context 模板
        </button>
      </div>
    {:else}
      <div class="flex flex-col gap-3">
        {#each configs as cfg (cfg.id)}
          {@const st = getStatus(cfg.id)}
          {@const stateInfo = stateLabel(st?.state)}
          <div class="flex items-center justify-between rounded-lg border border-[var(--color-border)] bg-white/5 p-4">
            <div class="grid gap-1">
              <div class="flex items-center gap-2">
                <span class="font-mono text-sm font-semibold text-[var(--color-text)]">{cfg.name}</span>
                <span class="rounded-full border px-2 py-0.5 text-[10px] font-medium {stateInfo.color}">
                  {stateInfo.label}
                </span>
                {#if st?.pid}
                  <span class="font-mono text-[10px] text-[var(--color-text-muted)]">PID: {st.pid}</span>
                {/if}
              </div>
              <div class="font-mono text-xs text-[var(--color-text-muted)]">
                命令: {cfg.command} {cfg.args.join(" ")}
              </div>
              {#if st?.errorMessage}
                <div class="text-[11px] text-red-400 font-mono">
                  最近错误: {st.errorMessage}
                </div>
              {/if}
              <div class="flex gap-3 text-[11px] text-[var(--color-text-muted)] pt-0.5">
                <span>已发现工具: {st?.discoveredToolsCount ?? 0} 个</span>
                <span>白名单: {cfg.allowedTools.length === 0 ? "全部允许" : `${cfg.allowedTools.length} 个`}</span>
              </div>
            </div>

            <!-- 操作按钮群 -->
            <div class="flex items-center gap-2">
              <button
                class="rounded-md border border-[var(--color-border)] px-2.5 py-1 text-xs hover:bg-white/5"
                onclick={() => void handleViewTools(cfg)}
              >
                查看工具
              </button>
              <button
                class="rounded-md border border-[var(--color-border)] px-2.5 py-1 text-xs hover:bg-white/5"
                onclick={() => void handleReconnect(cfg.id)}
              >
                重连
              </button>
              <button
                class="rounded-md border border-[var(--color-border)] px-2.5 py-1 text-xs hover:bg-white/5"
                onclick={() => void handleToggleEnable(cfg)}
              >
                {cfg.enabled ? "禁用" : "启用"}
              </button>
              <button
                class="rounded-md border border-[var(--color-border)] px-2.5 py-1 text-xs hover:bg-white/5"
                onclick={() => openEditModal(cfg)}
              >
                编辑
              </button>
              <button
                class="rounded-md border border-[var(--color-border)] px-2.5 py-1 text-xs text-red-400 hover:bg-red-500/10"
                onclick={() => void handleDelete(cfg.id)}
              >
                删除
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if showFormModal}
  <ExternalMcpFormModal
    {workspaceId}
    config={editingConfig}
    onSave={handleSaveConfig}
    onClose={() => { showFormModal = false; }}
  />
{/if}

{#if viewingTools}
  <ExternalMcpToolsModal
    serverName={viewingTools.serverName}
    tools={viewingTools.tools}
    onClose={() => { viewingTools = null; }}
  />
{/if}
