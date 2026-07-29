<script lang="ts">
  import type { McpTool } from "$lib/types";
  import { makePublicToolName } from "$lib/utils/namespace";

  interface Props {
    serverName: string;
    tools: McpTool[];
    onClose: () => void;
  }

  let { serverName, tools, onClose }: Props = $props();
</script>

<div class="fixed inset-0 z-60 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
  <div class="max-h-[85vh] w-full max-w-xl overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-bg)] p-6 shadow-2xl">
    <div class="flex items-center justify-between border-b border-[var(--color-border)] pb-3">
      <h3 class="text-base font-semibold text-[var(--color-text)]">
        外部 MCP [{serverName}] 已发现的工具 ({tools.length})
      </h3>
      <button class="text-xl text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={onClose}>×</button>
    </div>

    <div class="mt-4 flex flex-col gap-3">
      {#if tools.length === 0}
        <div class="p-4 text-center text-xs text-[var(--color-text-muted)]">
          暂未发现任何工具（可能服务未启动或尚未完成初始化）
        </div>
      {:else}
        {#each tools as tool}
          <div class="rounded-lg border border-[var(--color-border)] bg-white/5 p-3">
            <div class="flex items-center justify-between">
              <span class="font-mono text-xs font-semibold text-emerald-400">
                {tool.name}
              </span>
              <span class="font-mono text-[11px] text-[var(--color-text-muted)]">
                公网暴露名: {makePublicToolName(serverName, tool.name)}
              </span>
            </div>
            {#if tool.description}
              <p class="mt-1 text-xs text-[var(--color-text-muted)]">{tool.description}</p>
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    <div class="mt-5 flex justify-end border-t border-[var(--color-border)] pt-3">
      <button class="rounded-md border border-[var(--color-border)] px-4 py-1.5 text-xs font-medium hover:bg-white/5" onclick={onClose}>
        关闭
      </button>
    </div>
  </div>
</div>
