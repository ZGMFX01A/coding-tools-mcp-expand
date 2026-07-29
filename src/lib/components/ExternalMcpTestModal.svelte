<script lang="ts">
  import type { TestConnectionResult } from "$lib/types";

  interface Props {
    result: TestConnectionResult;
    onApplyWhitelist: (tools: string[]) => void;
    onClose: () => void;
  }

  let { result, onApplyWhitelist, onClose }: Props = $props();

  function applyWhitelist() {
    const toolNames = result.discoveredTools.map((t) => t.name);
    onApplyWhitelist(toolNames);
    onClose();
  }

  function errorKindLabel(kind: string | undefined | null): { label: string; tagClass: string } {
    switch (kind) {
      case "command_not_found":
        return { label: "命令不存在", tagClass: "bg-red-500/20 text-red-300 border-red-500/30" };
      case "entry_file_not_found":
        return { label: "入口文件不存在", tagClass: "bg-amber-500/20 text-amber-300 border-amber-500/30" };
      case "process_spawn_failed":
        return { label: "进程启动失败", tagClass: "bg-red-500/20 text-red-300 border-red-500/30" };
      case "initialize_timeout":
        return { label: "初始化超时", tagClass: "bg-amber-500/20 text-amber-300 border-amber-500/30" };
      case "initialize_failed":
        return { label: "MCP 初始化失败", tagClass: "bg-red-500/20 text-red-300 border-red-500/30" };
      default:
        return { label: "测试失败", tagClass: "bg-red-500/20 text-red-300 border-red-500/30" };
    }
  }
</script>

<div class="fixed inset-0 z-60 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
  <div class="max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-bg)] p-6 shadow-2xl">
    <div class="flex items-center justify-between border-b border-[var(--color-border)] pb-3">
      <h3 class="text-base font-semibold text-[var(--color-text)]">
        测试连接结果
      </h3>
      <button class="text-xl text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={onClose}>×</button>
    </div>

    <div class="mt-4 grid gap-3 text-sm">
      <div class="flex items-center justify-between rounded-md p-3 font-medium text-xs {result.success ? 'bg-emerald-500/10 text-emerald-400' : 'bg-red-500/10 text-red-400'}">
        <span>{result.success ? "✓ 启动与初始化握手成功" : "✗ 连接失败"}</span>
        <span>耗时: {result.durationMs} ms</span>
      </div>

      {#if result.resolvedCommandDisplay}
        <div class="rounded-md border border-[var(--color-border)] bg-black/20 p-2.5 text-xs font-mono">
          <span class="text-[var(--color-text-muted)] font-sans">实际尝试执行命令: </span>
          <span class="text-blue-400">{result.resolvedCommandDisplay}</span>
        </div>
      {/if}

      {#if result.protocolVersion}
        <div class="text-xs text-[var(--color-text-muted)]">
          协议版本: <span class="font-mono text-[var(--color-text)]">{result.protocolVersion}</span>
        </div>
      {/if}

      {#if !result.success && result.errorMessage}
        {@const errTag = errorKindLabel(result.errorKind)}
        <div class="rounded-md bg-red-500/10 p-3.5 text-xs text-red-400 grid gap-2">
          <div class="flex items-center gap-2">
            <span class="rounded border px-2 py-0.5 text-[10px] font-semibold {errTag.tagClass}">
              {errTag.label}
            </span>
          </div>
          <p class="font-mono text-xs leading-relaxed">{result.errorMessage}</p>
        </div>
      {/if}

      {#if result.success}
        <div class="mt-2 grid gap-2">
          <div class="flex items-center justify-between">
            <span class="text-xs font-semibold text-[var(--color-text)]">
              已发现工具数量 ({result.discoveredTools.length})
            </span>
            {#if result.discoveredTools.length > 0}
              <button class="text-xs text-blue-400 hover:underline" onclick={applyWhitelist}>
                全部设为授权白名单
              </button>
            {/if}
          </div>

          <div class="max-h-48 overflow-y-auto rounded-md border border-[var(--color-border)] p-2">
            {#if result.discoveredTools.length === 0}
              <span class="text-xs text-[var(--color-text-muted)]">未发现任何工具</span>
            {:else}
              <div class="flex flex-col gap-1.5">
                {#each result.discoveredTools as tool}
                  <div class="rounded bg-white/5 p-2">
                    <div class="font-mono text-xs font-semibold text-[var(--color-text)]">{tool.name}</div>
                    {#if tool.description}
                      <div class="text-[11px] text-[var(--color-text-muted)]">{tool.description}</div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {/if}
    </div>

    <div class="mt-5 flex justify-end gap-2 border-t border-[var(--color-border)] pt-3">
      <button class="rounded-md border border-[var(--color-border)] px-4 py-1.5 text-xs font-medium hover:bg-white/5" onclick={onClose}>
        关闭
      </button>
    </div>
  </div>
</div>
