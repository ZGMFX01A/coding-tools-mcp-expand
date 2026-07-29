<script lang="ts">
  import type { ExternalMcpConfig, TestConnectionResult, FastContextDetectionResult } from "$lib/types";
  import { testExternalMcpConnection, detectFastContextEnv } from "$lib/api/external_mcp";
  import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
  import ExternalMcpTestModal from "./ExternalMcpTestModal.svelte";

  interface Props {
    workspaceId: string;
    config: ExternalMcpConfig | null;
    onSave: (config: ExternalMcpConfig) => void | Promise<void>;
    onClose: () => void;
  }

  let { workspaceId, config, onSave, onClose }: Props = $props();

  let name = $state(config?.name ?? "fast-context");
  let enabled = $state(config?.enabled ?? true);
  let command = $state(config?.command ?? "");
  let argsList = $state<string[]>(config?.args ? [...config.args] : []);
  let envPairs = $state<Array<{ key: string; value: string; isSecret: boolean }>>(
    config?.env
      ? Object.entries(config.env).map(([k, v]) => ({
          key: k,
          value: v,
          isSecret: isSecretKey(k),
        }))
      : [
          { key: "FC_INCLUDE_SNIPPETS", value: "true", isSecret: false },
          { key: "WINDSURF_API_KEY", value: "", isSecret: true },
        ],
  );
  let allowedToolsText = $state((config?.allowedTools ?? ["extract_windsurf_key", "fast_context_search"]).join(", "));
  let autoRestart = $state(config?.autoRestart ?? true);
  let initializeTimeoutSeconds = $state(config?.initializeTimeoutSeconds ?? 30);
  let callTimeoutSeconds = $state(config?.callTimeoutSeconds ?? 120);

  let presetMode = $state<"local_cmd" | "local_file" | "npx" | "unselected">("unselected");
  let detectionInfo = $state<FastContextDetectionResult | null>(null);
  let detecting = $state(false);

  let newArg = $state("");
  let newEnvKey = $state("");
  let newEnvVal = $state("");
  let testing = $state(false);
  let testResult = $state<TestConnectionResult | null>(null);
  let showTestModal = $state(false);
  let errorMsg = $state("");

  const actualCommandDisplay = $derived(
    command.trim()
      ? `${command.trim()} ${argsList.join(" ")}`.trim()
      : "(尚未指定启动命令)"
  );

  function isSecretKey(key: string): boolean {
    const k = key.toUpperCase();
    return ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASS", "AUTH", "CREDENTIAL"].some((kw) => k.includes(kw));
  }

  $effect(() => {
    if (!config && name === "fast-context") {
      void runDetection();
    } else if (config) {
      if (config.command === "npx") {
        presetMode = "npx";
      } else if (config.command === "node") {
        presetMode = "local_file";
      } else if (config.command) {
        presetMode = "local_cmd";
      }
    }
  });

  async function runDetection() {
    detecting = true;
    try {
      const res = await detectFastContextEnv();
      detectionInfo = res;
      if (res.detected) {
        if (res.mode === "local_cmd" && res.command) {
          presetMode = "local_cmd";
          command = res.command;
          argsList = res.args;
        } else if (res.mode === "local_file" && res.entryPath) {
          presetMode = "local_file";
          command = "node";
          argsList = [res.entryPath];
        }
      } else {
        presetMode = "unselected";
        command = "";
        argsList = [];
      }
    } catch (e) {
      console.error("环境检测失败:", e);
    } finally {
      detecting = false;
    }
  }

  async function selectScriptFile() {
    try {
      const selected = await openFileDialog({
        multiple: false,
        filters: [{ name: "Node.js 脚本入口", extensions: ["js", "mjs", "cjs"] }],
      });
      if (selected && typeof selected === "string") {
        command = "node";
        argsList = [selected];
        presetMode = "local_file";
        errorMsg = "";
      }
    } catch (e: any) {
      errorMsg = `选择文件失败: ${e?.message || e}`;
    }
  }

  function applyPreset(mode: "local_cmd" | "local_file" | "npx") {
    errorMsg = "";
    if (mode === "local_cmd") {
      presetMode = "local_cmd";
      command = "fast-context-mcp";
      argsList = [];
    } else if (mode === "local_file") {
      presetMode = "local_file";
      command = "node";
      if (argsList.length === 0 || !argsList[0].endsWith(".js")) {
        argsList = [];
      }
      void selectScriptFile();
    } else if (mode === "npx") {
      presetMode = "npx";
      command = "npx";
      argsList = ["-y", "--prefer-offline", "fast-context-mcp@1.3.0"];
    }
  }

  function addArg() {
    if (!newArg.trim()) return;
    argsList = [...argsList, newArg.trim()];
    newArg = "";
  }

  function removeArg(index: number) {
    argsList = argsList.filter((_, i) => i !== index);
  }

  function moveArg(index: number, direction: -1 | 1) {
    const target = index + direction;
    if (target < 0 || target >= argsList.length) return;
    const copy = [...argsList];
    const temp = copy[index];
    copy[index] = copy[target];
    copy[target] = temp;
    argsList = copy;
  }

  function addEnvPair() {
    if (!newEnvKey.trim()) return;
    const key = newEnvKey.trim();
    if (envPairs.some((p) => p.key === key)) {
      errorMsg = `环境变量 Key '${key}' 已存在`;
      return;
    }
    errorMsg = "";
    envPairs = [...envPairs, { key, value: newEnvVal, isSecret: isSecretKey(key) }];
    newEnvKey = "";
    newEnvVal = "";
  }

  function removeEnvPair(index: number) {
    envPairs = envPairs.filter((_, i) => i !== index);
  }

  function buildConfig(): ExternalMcpConfig {
    const envRecord: Record<string, string> = {};
    for (const p of envPairs) {
      if (p.key.trim()) {
        envRecord[p.key.trim()] = p.value;
      }
    }
    const allowedTools = allowedToolsText
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);

    return {
      id: config?.id ?? `mcp-${Date.now()}`,
      name: name.trim(),
      enabled,
      command: command.trim(),
      args: argsList,
      env: envRecord,
      allowedTools,
      autoRestart,
      initializeTimeoutSeconds,
      callTimeoutSeconds,
    };
  }

  async function handleTest() {
    if (!command.trim() || !name.trim()) {
      errorMsg = "未找到本地命令，请指定 MCP 入口文件，或选择通过 npx 运行。";
      return;
    }
    if (command.trim() === "node" && argsList.length === 0) {
      errorMsg = "使用 node 运行必须指定 .js / .mjs / .cjs 入口文件";
      return;
    }
    errorMsg = "";
    testing = true;
    try {
      const cfg = buildConfig();
      const res = await testExternalMcpConnection(workspaceId, cfg);
      testResult = res;
      showTestModal = true;
    } catch (e: any) {
      errorMsg = e?.message || String(e);
    } finally {
      testing = false;
    }
  }

  async function handleSubmit() {
    if (!name.trim()) {
      errorMsg = "名称不能为空";
      return;
    }
    if (!command.trim()) {
      errorMsg = "未找到本地命令，请指定 MCP 入口文件，或选择通过 npx 运行。";
      return;
    }
    if (command.trim() === "node" && argsList.length === 0) {
      errorMsg = "使用 node 运行必须指定 .js / .mjs / .cjs 入口文件";
      return;
    }
    errorMsg = "";
    const cfg = buildConfig();
    await onSave(cfg);
    onClose();
  }

  function applyWhitelistFromTools(tools: string[]) {
    allowedToolsText = tools.join(", ");
  }
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-xs">
  <div class="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-[var(--color-border)] bg-[var(--color-bg)] p-6 shadow-2xl">
    <div class="flex items-center justify-between border-b border-[var(--color-border)] pb-3">
      <h3 class="text-lg font-semibold text-[var(--color-text)]">
        {config ? "编辑外部 stdio MCP" : "配置外部 stdio MCP"}
      </h3>
      <button class="text-xl text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={onClose}>×</button>
    </div>

    <!-- 模式选择与环境检测通知 -->
    <div class="mt-4 rounded-lg border border-[var(--color-border)] bg-white/5 p-3 flex flex-col gap-2.5">
      <div class="flex items-center justify-between">
        <span class="text-xs font-semibold text-[var(--color-text)]">启动方式预设:</span>
        {#if detecting}
          <span class="text-xs text-[var(--color-text-muted)]">正在检测本机环境...</span>
        {:else if detectionInfo}
          <span class="text-xs {detectionInfo.detected ? 'text-emerald-400 font-medium' : 'text-amber-400'}">
            {detectionInfo.message}
          </span>
        {/if}
      </div>

      <div class="flex flex-wrap gap-2">
        <button
          type="button"
          class="rounded border px-2.5 py-1 text-xs font-medium transition-colors {presetMode === 'local_cmd' ? 'border-emerald-500 bg-emerald-500/10 text-emerald-400' : 'border-[var(--color-border)] hover:bg-white/10'}"
          onclick={() => applyPreset("local_cmd")}
        >
          1. 本机已有命令 {detectionInfo?.mode === 'local_cmd' ? '(推荐)' : ''}
        </button>
        <button
          type="button"
          class="rounded border px-2.5 py-1 text-xs font-medium transition-colors {presetMode === 'local_file' ? 'border-emerald-500 bg-emerald-500/10 text-emerald-400' : 'border-[var(--color-border)] hover:bg-white/10'}"
          onclick={() => applyPreset("local_file")}
        >
          2. 指定本地入口文件 (node)
        </button>
        <button
          type="button"
          class="rounded border px-2.5 py-1 text-xs font-medium transition-colors {presetMode === 'npx' ? 'border-amber-500 bg-amber-500/10 text-amber-400' : 'border-[var(--color-border)] hover:bg-white/10'}"
          onclick={() => applyPreset("npx")}
        >
          3. 通过 npx 运行 (兼容)
        </button>
      </div>

      {#if !detectionInfo?.detected && presetMode === 'unselected'}
        <div class="rounded-md bg-amber-500/10 p-2.5 text-xs text-amber-300">
          未检测到本机 fast-context，请选择本地入口文件，或通过 npx 运行。
        </div>
      {/if}
    </div>

    {#if errorMsg}
      <div class="mt-3 rounded-md bg-red-500/10 p-2.5 text-xs text-red-400">
        {errorMsg}
      </div>
    {/if}

    <form class="mt-4 grid gap-4 text-sm" onsubmit={(e) => { e.preventDefault(); void handleSubmit(); }}>
      <!-- 名称与启用状态 -->
      <div class="grid grid-cols-2 gap-3">
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">显示名称 / 命名空间 *</span>
          <input type="text" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm" placeholder="fast-context" bind:value={name} />
        </label>
        <label class="flex items-center gap-2 pt-5">
          <input type="checkbox" bind:checked={enabled} />
          <span class="text-sm font-medium">启用此服务</span>
        </label>
      </div>

      <!-- 启动命令与本地文件选择器 -->
      <div class="grid gap-2">
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">启动命令 *</span>
          <div class="flex gap-2">
            <input type="text" class="flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-sm" placeholder="fast-context-mcp / node / npx" bind:value={command} />
            {#if command === "node" || presetMode === "local_file"}
              <button
                type="button"
                class="rounded-md border border-[var(--color-border)] bg-white/5 px-3 py-1.5 text-xs font-medium hover:bg-white/10"
                onclick={selectScriptFile}
              >
                浏览选择入口文件...
              </button>
            {/if}
          </div>
        </label>

        <!-- 实际执行命令预览 (Requirement 10) -->
        <div class="rounded-md border border-[var(--color-border)] bg-black/20 p-2.5 text-xs">
          <span class="text-[var(--color-text-muted)] font-medium">实际执行命令行: </span>
          <span class="font-mono text-emerald-400">{actualCommandDisplay}</span>
        </div>
      </div>

      <!-- 参数列表 -->
      <div class="grid gap-1.5">
        <span class="text-xs text-[var(--color-text-muted)]">命令参数列表 (args)</span>
        <div class="flex flex-col gap-1.5">
          {#each argsList as arg, i}
            <div class="flex items-center gap-2">
              <input type="text" class="flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" bind:value={argsList[i]} />
              <button type="button" class="px-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={() => moveArg(i, -1)}>↑</button>
              <button type="button" class="px-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={() => moveArg(i, 1)}>↓</button>
              <button type="button" class="px-2 text-xs text-red-400 hover:text-red-300" onclick={() => removeArg(i)}>删除</button>
            </div>
          {/each}
        </div>
        <div class="flex gap-2 pt-1">
          <input type="text" class="flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="输入参数（如 .js 脚本文件路径或参数标志）" bind:value={newArg} onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); addArg(); } }} />
          <button type="button" class="rounded-md border border-[var(--color-border)] px-3 py-1.5 text-xs font-medium hover:bg-white/5" onclick={addArg}>添加参数</button>
        </div>
      </div>

      <!-- 环境变量 -->
      <div class="grid gap-1.5">
        <span class="text-xs text-[var(--color-text-muted)]">环境变量 (env)</span>
        <div class="flex flex-col gap-1.5">
          {#each envPairs as pair, i}
            <div class="flex items-center gap-2">
              <input type="text" class="w-1/3 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="KEY" bind:value={pair.key} />
              <input type={pair.isSecret ? "password" : "text"} class="flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="VALUE" bind:value={pair.value} />
              <button type="button" class="px-1 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text)]" onclick={() => { pair.isSecret = !pair.isSecret; }}>{pair.isSecret ? "显" : "隐"}</button>
              <button type="button" class="px-2 text-xs text-red-400 hover:text-red-300" onclick={() => removeEnvPair(i)}>删除</button>
            </div>
          {/each}
        </div>
        <div class="flex gap-2 pt-1">
          <input type="text" class="w-1/3 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="KEY" bind:value={newEnvKey} />
          <input type="text" class="flex-1 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="VALUE" bind:value={newEnvVal} />
          <button type="button" class="rounded-md border border-[var(--color-border)] px-3 py-1.5 text-xs font-medium hover:bg-white/5" onclick={addEnvPair}>添加变量</button>
        </div>
      </div>

      <!-- 工具白名单 -->
      <label class="grid gap-1">
        <div class="flex items-center justify-between">
          <span class="text-xs text-[var(--color-text-muted)]">工具白名单 (allowedTools，逗号分隔，留空允许全部)</span>
          <button type="button" class="text-xs text-[var(--color-primary,theme(colors.blue.400))] hover:underline" onclick={() => { allowedToolsText = ""; }}>清空</button>
        </div>
        <input type="text" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 font-mono text-xs" placeholder="extract_windsurf_key, fast_context_search" bind:value={allowedToolsText} />
      </label>

      <!-- 超时与自动重启 -->
      <div class="grid grid-cols-3 gap-3">
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">初始化超时 (秒)</span>
          <input type="number" min="5" max="300" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-xs" bind:value={initializeTimeoutSeconds} />
        </label>
        <label class="grid gap-1">
          <span class="text-xs text-[var(--color-text-muted)]">调用超时 (秒)</span>
          <input type="number" min="5" max="600" class="rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] px-2.5 py-1.5 text-xs" bind:value={callTimeoutSeconds} />
        </label>
        <label class="flex items-center gap-2 pt-5">
          <input type="checkbox" bind:checked={autoRestart} />
          <span class="text-xs">异常退出自动重启</span>
        </label>
      </div>

      <!-- 底部按钮 -->
      <div class="mt-4 flex items-center justify-between border-t border-[var(--color-border)] pt-4">
        <button type="button" class="rounded-md border border-[var(--color-border)] px-4 py-1.5 text-xs font-medium hover:bg-white/5" disabled={testing} onclick={handleTest}>
          {testing ? "正在测试..." : "测试连接"}
        </button>
        <div class="flex gap-2">
          <button type="button" class="rounded-md border border-[var(--color-border)] px-4 py-1.5 text-xs font-medium hover:bg-white/5" onclick={onClose}>取消</button>
          <button type="submit" class="rounded-md bg-blue-600 px-4 py-1.5 text-xs font-medium text-white hover:bg-blue-500">保存</button>
        </div>
      </div>
    </form>
  </div>
</div>

{#if showTestModal && testResult}
  <ExternalMcpTestModal
    result={testResult}
    onApplyWhitelist={(tools) => applyWhitelistFromTools(tools)}
    onClose={() => { showTestModal = false; }}
  />
{/if}
